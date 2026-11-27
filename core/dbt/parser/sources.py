import itertools
from dataclasses import replace
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Set, Tuple

from dbt.adapters.capability import Capability
from dbt.adapters.factory import get_adapter
from dbt.artifacts.resources import FreshnessThreshold, SourceConfig, Time
from dbt.config import RuntimeConfig
from dbt.context.context_config import (
    BaseContextConfigGenerator,
    ContextConfigGenerator,
    UnrenderedConfigGenerator,
)
from dbt.contracts.graph.manifest import Manifest, SourceKey
from dbt.contracts.graph.nodes import (
    GenericTestNode,
    SourceDefinition,
    UnpatchedSourceDefinition,
)
from dbt.contracts.graph.unparsed import (
    SourcePatch,
    SourceTablePatch,
    UnparsedColumn,
    UnparsedSourceDefinition,
    UnparsedSourceTableDefinition,
)
from dbt.events.types import FreshnessConfigProblem, UnusedTables, ValidationWarning
from dbt.exceptions import ParsingError
from dbt.node_types import NodeType
from dbt.parser.common import ParserRef
from dbt.parser.schema_generic_tests import SchemaGenericTestParser
from dbt_common.events.base_types import EventGroupType
from dbt_common.events.functions import fire_event, fire_or_defer_event
from dbt_common.exceptions import DbtInternalError


# An UnparsedSourceDefinition is taken directly from the yaml
# file. It can affect multiple tables, all of which will eventually
# have their own source node. An UnparsedSourceDefinition will
# generate multiple UnpatchedSourceDefinition nodes (one per
# table) in the SourceParser.add_source_definitions. The
# SourcePatcher takes an UnparsedSourceDefinition and the
# SourcePatch and produces a SourceDefinition. Each
# SourcePatch can be applied to multiple UnpatchedSourceDefinitions.
class SourcePatcher:
    def __init__(
        self,
        root_project: RuntimeConfig,
        manifest: Manifest,
    ) -> None:
        self.root_project = root_project
        self.manifest = manifest
        self.generic_test_parsers: Dict[str, SchemaGenericTestParser] = {}
        self.patches_used: Dict[SourceKey, Set[str]] = {}
        self.sources: Dict[str, SourceDefinition] = {}
        self._deprecations: Set[Any] = set()

    # This method calls the 'parse_source' method which takes
    # the UnpatchedSourceDefinitions in the manifest and combines them
    # with SourcePatches to produce SourceDefinitions.
    def construct_sources(self) -> None:
        for unique_id, unpatched in self.manifest.sources.items():
            schema_file = self.manifest.files[unpatched.file_id]
            if isinstance(unpatched, SourceDefinition):
                # In partial parsing, there will be SourceDefinitions
                # which must be retained.
                self.sources[unpatched.unique_id] = unpatched
                continue
            # returns None if there is no patch
            patch = self.get_patch_for(unpatched)

            # returns unpatched if there is no patch
            patched = self.patch_source(unpatched, patch)

            # now use the patched UnpatchedSourceDefinition to extract test data.
            for test in self.get_source_tests(patched):
                if test.config.enabled:
                    self.manifest.add_node_nofile(test)
                else:
                    self.manifest.add_disabled_nofile(test)
                # save the test unique_id in the schema_file, so we can
                # process in partial parsing
                test_from = {"key": "sources", "name": patched.source.name}
                schema_file.add_test(test.unique_id, test_from)

            # Convert UnpatchedSourceDefinition to a SourceDefinition
            parsed = self.parse_source(patched)
            if parsed.config.enabled:
                self.sources[unique_id] = parsed
            else:
                self.manifest.add_disabled_nofile(parsed)

        self.warn_unused()

    def patch_source(
        self,
        unpatched: UnpatchedSourceDefinition,
        patch: Optional[SourcePatch],
    ) -> UnpatchedSourceDefinition:

        # This skips patching if no patch exists because of the
        # performance overhead of converting to and from dicts
        if patch is None:
            return unpatched

        source_dct = unpatched.source.to_dict(omit_none=True)
        table_dct = unpatched.table.to_dict(omit_none=True)
        patch_path: Optional[Path] = None

        source_table_patch: Optional[SourceTablePatch] = None

        if patch is not None:
            source_table_patch = patch.get_table_named(unpatched.table.name)
            source_dct.update(patch.to_patch_dict())
            patch_path = patch.path

        if source_table_patch is not None:
            table_dct.update(source_table_patch.to_patch_dict())

        source = UnparsedSourceDefinition.from_dict(source_dct)
        table = UnparsedSourceTableDefinition.from_dict(table_dct)
        return replace(unpatched, source=source, table=table, patch_path=patch_path)

    # This converts an UnpatchedSourceDefinition to a SourceDefinition
    def parse_source(self, target: UnpatchedSourceDefinition) -> SourceDefinition:
        source = target.source
        table = target.table
        refs = ParserRef.from_target(table)
        unique_id = target.unique_id
        description = table.description or ""
        source_description = source.description or ""

        quoting = source.quoting.merged(table.quoting)
        # Retain original source meta prior to merge with table meta
        source_meta = {**source.meta, **source.config.get("meta", {})}

        config = self._generate_source_config(
            target=target,
            rendered=True,
        )

        config = config.finalize_and_validate()

        unrendered_config = self._generate_source_config(
            target=target,
            rendered=False,
        )

        if not isinstance(config, SourceConfig):
            raise DbtInternalError(
                f"Calculated a {type(config)} for a source, but expected a SourceConfig"
            )

        default_database = self.root_project.credentials.database

        parsed_source = SourceDefinition(
            package_name=target.package_name,
            database=(source.database or default_database),
            unrendered_database=source.unrendered_database,
            schema=(source.schema or source.name),
            unrendered_schema=source.unrendered_schema,
            identifier=(table.identifier or table.name),
            path=target.path,
            original_file_path=target.original_file_path,
            columns=refs.column_info,
            unique_id=unique_id,
            name=table.name,
            description=description,
            external=table.external,
            source_name=source.name,
            source_description=source_description,
            source_meta=source_meta,
            meta=config.meta,
            loader=source.loader,
            loaded_at_field=config.loaded_at_field,
            loaded_at_query=config.loaded_at_query,
            freshness=config.freshness,
            quoting=quoting,
            resource_type=NodeType.Source,
            fqn=target.fqn,
            tags=config.tags,
            config=config,
            unrendered_config=unrendered_config,
        )

        if (
            parsed_source.freshness
            and not parsed_source.loaded_at_field
            and not parsed_source.loaded_at_query
            and not get_adapter(self.root_project).supports(Capability.TableLastModifiedMetadata)
        ):
            # Metadata-based freshness is being used by default for this node,
            # but is not available through the configured adapter, so warn the
            # user that freshness info will not be collected for this node at
            # runtime.
            fire_event(
                FreshnessConfigProblem(
                    msg=f"The configured adapter does not support metadata-based freshness. A loaded_at_field must be specified for source '{source.name}.{table.name}'."
                )
            )

        # relation name is added after instantiation because the adapter does
        # not provide the relation name for a UnpatchedSourceDefinition object
        parsed_source.relation_name = self._get_relation_name(parsed_source)
        return parsed_source

    # Use the SchemaGenericTestParser to parse the source tests
    def get_generic_test_parser_for(self, package_name: str) -> "SchemaGenericTestParser":
        if package_name in self.generic_test_parsers:
            generic_test_parser = self.generic_test_parsers[package_name]
        else:
            all_projects = self.root_project.load_dependencies()
            project = all_projects[package_name]
            generic_test_parser = SchemaGenericTestParser(
                project, self.manifest, self.root_project
            )
            self.generic_test_parsers[package_name] = generic_test_parser
        return generic_test_parser

    def get_source_tests(self, target: UnpatchedSourceDefinition) -> Iterable[GenericTestNode]:
        is_root_project = True if self.root_project.project_name == target.package_name else False
        target.validate_data_tests(is_root_project)
        for data_test, column in target.get_tests():
            yield self.parse_source_test(
                target=target,
                data_test=data_test,
                column=column,
            )

    def get_patch_for(
        self,
        unpatched: UnpatchedSourceDefinition,
    ) -> Optional[SourcePatch]:
        if isinstance(unpatched, SourceDefinition):
            return None
        key = (unpatched.package_name, unpatched.source.name)
        patch: Optional[SourcePatch] = self.manifest.source_patches.get(key)
        if patch is None:
            return None
        if key not in self.patches_used:
            # mark the key as used
            self.patches_used[key] = set()
        if patch.get_table_named(unpatched.table.name) is not None:
            self.patches_used[key].add(unpatched.table.name)
        return patch

    # This calls parse_generic_test in the SchemaGenericTestParser
    def parse_source_test(
        self,
        target: UnpatchedSourceDefinition,
        data_test: Dict[str, Any],
        column: Optional[UnparsedColumn],
    ) -> GenericTestNode:
        column_name: Optional[str]
        if column is None:
            column_name = None
        else:
            column_name = column.name
            should_quote = column.quote or (column.quote is None and target.quote_columns)
            if should_quote:
                column_name = get_adapter(self.root_project).quote(column_name)

        tags_sources = [target.source.tags, target.table.tags]
        if column is not None:
            tags_sources.append(column.tags)
            if column_config_tags := column.config.get("tags", []):
                if isinstance(column_config_tags, list):
                    tags_sources.append(column_config_tags)
                elif isinstance(column_config_tags, str):
                    tags_sources.append([column_config_tags])
        tags = list(itertools.chain.from_iterable(tags_sources))

        generic_test_parser = self.get_generic_test_parser_for(target.package_name)
        node = generic_test_parser.parse_generic_test(
            target=target,
            data_test=data_test,
            tags=tags,
            column_name=column_name,
            schema_file_id=target.file_id,
            version=None,
        )
        return node

    def _generate_source_config(self, target: UnpatchedSourceDefinition, rendered: bool):
        generator: BaseContextConfigGenerator
        if rendered:
            generator = ContextConfigGenerator(self.root_project)
        else:
            generator = UnrenderedConfigGenerator(self.root_project)

        # configs with precendence set
        precedence_configs = dict()
        # first apply source configs
        precedence_configs.update(target.source.config)
        # then overrite anything that is defined on source tables
        # this is not quite complex enough for configs that can be set as top-level node keys, but
        # it works while source configs can only include `enabled`.
        precedence_configs.update(target.table.config)

        precedence_freshness = self.calculate_freshness_from_raw_target(target)
        if precedence_freshness:
            precedence_configs["freshness"] = precedence_freshness.to_dict()
        elif precedence_freshness is None:
            precedence_configs["freshness"] = None
        else:
            # this means that the user did not set a freshness threshold in the source schema file, as such
            # there should be no freshness precedence
            precedence_configs.pop("freshness", None)

        precedence_loaded_at_field, precedence_loaded_at_query = (
            self.calculate_loaded_at_field_query_from_raw_target(target)
        )
        precedence_configs["loaded_at_field"] = precedence_loaded_at_field
        precedence_configs["loaded_at_query"] = precedence_loaded_at_query

        # Handle merges across source, table, and config for meta and tags
        precedence_meta = self.calculate_meta_from_raw_target(target)
        precedence_configs["meta"] = precedence_meta

        precedence_tags = self.calculate_tags_from_raw_target(target)
        precedence_configs["tags"] = precedence_tags

        # Because freshness is a "object" config, the freshness from the dbt_project.yml and the freshness
        # from the schema file _won't_ get merged by this process. The result will be that the freshness will
        # come from the schema file if provided, and if not, it'll fall back to the dbt_project.yml freshness.
        return generator.calculate_node_config(
            config_call_dict={},
            fqn=target.fqn,
            resource_type=NodeType.Source,
            project_name=target.package_name,
            base=False,
            patch_config_dict=precedence_configs,
        )

    def _get_relation_name(self, node: SourceDefinition):
        adapter = get_adapter(self.root_project)
        relation_cls = adapter.Relation
        return str(relation_cls.create_from(self.root_project, node))

    def warn_unused(self) -> None:
        unused_tables: Dict[SourceKey, Optional[Set[str]]] = {}
        for patch in self.manifest.source_patches.values():
            key = (patch.overrides, patch.name)
            if key not in self.patches_used:
                unused_tables[key] = None
            elif patch.tables is not None:
                table_patches = {t.name for t in patch.tables}
                unused = table_patches - self.patches_used[key]
                # don't add unused tables, the
                if unused:
                    # because patches are required to be unique, we can safely
                    # write without looking
                    unused_tables[key] = unused

        if unused_tables:
            unused_tables_formatted = self.get_unused_msg(unused_tables)
            fire_or_defer_event(
                UnusedTables(unused_tables=unused_tables_formatted),
                force_warn_or_error_handling=True,
                event_group_type=EventGroupType.PARSE,
            )

        self.manifest.source_patches = {}

    def get_unused_msg(
        self,
        unused_tables: Dict[SourceKey, Optional[Set[str]]],
    ) -> List:
        unused_tables_formatted = []
        for key, table_names in unused_tables.items():
            patch = self.manifest.source_patches[key]
            patch_name = f"{patch.overrides}.{patch.name}"
            if table_names is None:
                unused_tables_formatted.append(f"  - Source {patch_name} (in {patch.path})")
            else:
                for table_name in sorted(table_names):
                    unused_tables_formatted.append(
                        f"  - Source table {patch_name}.{table_name} " f"(in {patch.path})"
                    )
        return unused_tables_formatted

    def calculate_freshness_from_raw_target(
        self,
        target: UnpatchedSourceDefinition,
    ) -> Optional[FreshnessThreshold]:
        source: UnparsedSourceDefinition = target.source
        table: UnparsedSourceTableDefinition = target.table

        source_freshness = source.freshness

        source_config_freshness_raw: Optional[Dict] = source.config.get(
            "freshness", {}
        )  # Will only be None if the user explicitly set it to null
        warn_on_fractional_freshness_counts(
            source_config_freshness_raw, source.name, table.name
        )
        source_config_freshness: Optional[FreshnessThreshold] = (
            FreshnessThreshold.from_dict(source_config_freshness_raw)
            if source_config_freshness_raw is not None
            else None
        )

        table_freshness = table.freshness

        table_config_freshness_raw: Optional[Dict] = table.config.get(
            "freshness", {}
        )  # Will only be None if the user explicitly set it to null
        warn_on_fractional_freshness_counts(
            table_config_freshness_raw, source.name, table.name
        )
        table_config_freshness: Optional[FreshnessThreshold] = (
            FreshnessThreshold.from_dict(table_config_freshness_raw)
            if table_config_freshness_raw is not None
            else None
        )

        return merge_source_freshness(
            source_freshness,
            source_config_freshness,
            table_freshness,
            table_config_freshness,
        )

    def calculate_loaded_at_field_query_from_raw_target(
        self, target: UnpatchedSourceDefinition
    ) -> Tuple[Optional[str], Optional[str]]:
        # We need to be able to tell the difference between explicitly setting the loaded_at_field to None/null
        # and when it's simply not set.  This allows a user to override the source level loaded_at_field so that
        # specific table can default to metadata-based freshness.

        # loaded_at_field and loaded_at_query are supported both at top-level (deprecated) and config-level (preferred) on sources and tables.
        if target.table.loaded_at_field_present and (
            target.table.loaded_at_query or target.table.config.get("loaded_at_query")
        ):
            raise ParsingError(
                "Cannot specify both loaded_at_field and loaded_at_query at table level."
            )
        if (target.source.loaded_at_field or target.source.config.get("loaded_at_field")) and (
            target.source.loaded_at_query or target.source.config.get("loaded_at_query")
        ):
            raise ParsingError(
                "Cannot specify both loaded_at_field and loaded_at_query at source level."
            )

        if (
            target.table.loaded_at_field_present
            or target.table.loaded_at_field is not None
            or target.table.config.get("loaded_at_field") is not None
        ):
            loaded_at_field = target.table.loaded_at_field or target.table.config.get(
                "loaded_at_field"
            )
        else:
            loaded_at_field = target.source.loaded_at_field or target.source.config.get(
                "loaded_at_field"
            )  # may be None, that's okay

        loaded_at_query: Optional[str]
        if (
            target.table.loaded_at_query is not None
            or target.table.config.get("loaded_at_query") is not None
        ):
            loaded_at_query = target.table.loaded_at_query or target.table.config.get(
                "loaded_at_query"
            )
        else:
            if target.table.loaded_at_field_present:
                loaded_at_query = None
            else:
                loaded_at_query = target.source.loaded_at_query or target.source.config.get(
                    "loaded_at_query"
                )

        return loaded_at_field, loaded_at_query

    def calculate_meta_from_raw_target(self, target: UnpatchedSourceDefinition) -> Dict[str, Any]:
        source_meta = target.source.meta or {}
        source_config_meta = target.source.config.get("meta", {})
        source_config_meta = source_config_meta if isinstance(source_config_meta, dict) else {}

        table_meta = target.table.meta or {}
        table_config_meta = target.table.config.get("meta", {})
        table_config_meta = table_config_meta if isinstance(table_config_meta, dict) else {}

        return {**source_meta, **source_config_meta, **table_meta, **table_config_meta}

    def calculate_tags_from_raw_target(self, target: UnpatchedSourceDefinition) -> List[str]:
        source_tags = target.source.tags or []
        source_config_tags = self._get_config_tags(
            target.source.config.get("tags", []), target.source.name
        )

        table_tags = target.table.tags or []
        table_config_tags = self._get_config_tags(
            target.table.config.get("tags", []), target.table.name
        )

        return sorted(
            set(itertools.chain(source_tags, source_config_tags, table_tags, table_config_tags))
        )

    def _get_config_tags(self, tags: Any, source_name: str) -> List[str]:
        config_tags = tags if isinstance(tags, list) else [tags]

        config_tags_valid: List[str] = []
        for tag in config_tags:
            if not isinstance(tag, str):
                fire_or_defer_event(
                    ValidationWarning(
                        field_name=f"`config.tags`: {tags}",
                        resource_type=NodeType.Source.value,
                        node_name=source_name,
                    ),
                    force_warn_or_error_handling=True,
                    event_group_type=EventGroupType.PARSE,
                )
            else:
                config_tags_valid.append(tag)

        return config_tags_valid


def merge_freshness_time_thresholds(
    base: Optional[Time], update: Optional[Time]
) -> Optional[Time]:
    if base and update:
        return base.merged(update)
    elif update is None:
        return None
    else:
        return update or base


def warn_on_fractional_freshness_counts(
    freshness_raw: Optional[Dict],
    source_name: str,
    table_name: str,
) -> None:
    """Warn when a `config:` freshness count is fractional.

    `Time.count` is an int, so `FreshnessThreshold.from_dict` silently
    truncates e.g. 1.5 to 1. The same value under the top-level `freshness:`
    key is a hard parse error, so at least warn here. See #17291.
    """
    if not freshness_raw:
        return
    for field in ("warn_after", "error_after"):
        threshold = freshness_raw.get(field)
        if not isinstance(threshold, dict):
            continue
        count = threshold.get("count")
        if isinstance(count, float) and not count.is_integer():
            fire_event(
                FreshnessConfigProblem(
                    msg=(
                        f"Source {source_name}.{table_name}: freshness {field}.count "
                        f"must be a whole number, got {count}; "
                        f"dbt will use {int(count)}."
                    )
                )
            )


def merge_source_freshness(
    *thresholds: Optional[FreshnessThreshold],
) -> Optional[FreshnessThreshold]:
    if not thresholds:
        return None

    # Initialize with the first threshold.
    # If the first threshold is None, current_merged_value will be None,
    # and subsequent merges will correctly follow the original logic.
    current_merged_value: Optional[FreshnessThreshold] = thresholds[0]

    # Iterate through the rest of the thresholds, applying the original pairwise logic
    for i in range(1, len(thresholds)):
        base = current_merged_value
        update = thresholds[i]

        if base is not None and update is not None:
            merged_freshness_obj = base.merged(update)
            # merge one level deeper the error_after and warn_after thresholds
            merged_error_after = merge_freshness_time_thresholds(
                base.error_after, update.error_after
            )
            merged_warn_after = merge_freshness_time_thresholds(base.warn_after, update.warn_after)

            merged_freshness_obj.error_after = merged_error_after
            merged_freshness_obj.warn_after = merged_warn_after
            current_merged_value = merged_freshness_obj
        elif base is None and bool(update):
            # If current_merged_value (base) is None, the update becomes the new value
            current_merged_value = update
        else:  # This covers cases where 'update' is None, or both 'base' and 'update' are None.
            # Following original logic, if 'update' is None, the result of the pair-merge is None.
            current_merged_value = None

    return current_merged_value
