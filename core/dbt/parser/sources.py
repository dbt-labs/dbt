import itertools
from dataclasses import replace
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Set

from dbt.adapters.capability import Capability
from dbt.adapters.factory import get_adapter
from dbt.artifacts.resources import FreshnessThreshold, SourceConfig, Time
from dbt.config import RuntimeConfig
from dbt.context.context_config import (
    BaseContextConfigGenerator,
    ContextConfigGenerator,
    UnrenderedConfigGenerator,
)
from dbt.contracts.files import SchemaSourceFile
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
from dbt.events.types import FreshnessConfigProblem, UnusedTables
from dbt.node_types import NodeType
from dbt.parser.common import ParserRef
from dbt.parser.schema_generic_tests import SchemaGenericTestParser
from dbt_common.events.functions import fire_event, warn_or_error
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

        # We need to be able to tell the difference between explicitly setting the loaded_at_field to None/null
        # and when it's simply not set.  This allows a user to override the source level loaded_at_field so that
        # specific table can default to metadata-based freshness.
        if table.loaded_at_field_present or table.loaded_at_field is not None:
            loaded_at_field = table.loaded_at_field
        else:
            loaded_at_field = source.loaded_at_field  # may be None, that's okay

        freshness = merge_freshness(source.freshness, table.freshness)
        quoting = source.quoting.merged(table.quoting)
        # path = block.path.original_file_path
        table_meta = table.meta or {}
        source_meta = source.meta or {}
        meta = {**source_meta, **table_meta}

        # make sure we don't do duplicate tags from source + table
        tags = sorted(set(itertools.chain(source.tags, table.tags)))

        config = self._generate_source_config(
            target=target,
            rendered=True,
        )

        config = config.finalize_and_validate()

        unrendered_config = self._generate_source_config(
            target=target,
            rendered=False,
        )

        schema_file = self.manifest.files[target.file_id]
        assert isinstance(schema_file, SchemaSourceFile)
        source_vars = schema_file.get_vars("sources", source.name)

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
            meta=meta,
            loader=source.loader,
            loaded_at_field=loaded_at_field,
            freshness=freshness,
            quoting=quoting,
            resource_type=NodeType.Source,
            fqn=target.fqn,
            tags=tags,
            config=config,
            unrendered_config=unrendered_config,
            vars=source_vars,
        )

        if (
            parsed_source.freshness
            and not parsed_source.loaded_at_field
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
            warn_or_error(UnusedTables(unused_tables=unused_tables_formatted))

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


def merge_freshness_time_thresholds(
    base: Optional[Time], update: Optional[Time]
) -> Optional[Time]:
    if base and update:
        return base.merged(update)
    elif update is None:
        return None
    else:
        return update or base


def merge_freshness(
    base: Optional[FreshnessThreshold], update: Optional[FreshnessThreshold]
) -> Optional[FreshnessThreshold]:
    if base is not None and update is not None:
        merged_freshness = base.merged(update)
        # merge one level deeper the error_after and warn_after thresholds
        merged_error_after = merge_freshness_time_thresholds(base.error_after, update.error_after)
        merged_warn_after = merge_freshness_time_thresholds(base.warn_after, update.warn_after)

        merged_freshness.error_after = merged_error_after
        merged_freshness.warn_after = merged_warn_after
        return merged_freshness
    elif base is None and update is not None:
        return update
    else:
        return None
