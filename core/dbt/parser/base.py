import abc
import itertools
import os
from typing import List, Dict, Any, Generic, Optional, TypeVar

from dbt.common.dataclass_schema import ValidationError

from dbt import utils
from dbt.clients.jinja import MacroGenerator
from dbt.context.providers import (
    generate_parser_model_context,
    generate_generate_name_macro_context,
)
from dbt.clients.adapter import get_adapter  # noqa: F401
from dbt.clients.jinja import get_rendered
from dbt.config import Project, RuntimeConfig
from dbt.context.context_config import ContextConfig
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.nodes import Contract, BaseNode, ManifestNode
from dbt.contracts.graph.unparsed import Docs, UnparsedNode
from dbt.exceptions import (
    DbtInternalError,
    ConfigUpdateError,
    DictParseError,
    InvalidAccessTypeError,
)
from dbt import hooks
from dbt.node_types import NodeType, ModelLanguage, AccessType
from dbt.parser.search import FileBlock

# internally, the parser may store a less-restrictive type that will be
# transformed into the final type. But it will have to be derived from
# ParsedNode to be operable.
FinalValue = TypeVar("FinalValue", bound=BaseNode)
IntermediateValue = TypeVar("IntermediateValue", bound=BaseNode)

IntermediateNode = TypeVar("IntermediateNode", bound=Any)
FinalNode = TypeVar("FinalNode", bound=ManifestNode)


ConfiguredBlockType = TypeVar("ConfiguredBlockType", bound=FileBlock)


class BaseParser(Generic[FinalValue]):
    def __init__(self, project: Project, manifest: Manifest) -> None:
        self.project = project
        self.manifest = manifest

    @abc.abstractmethod
    def parse_file(self, block: FileBlock) -> None:
        pass

    @abc.abstractproperty
    def resource_type(self) -> NodeType:
        pass

    def generate_unique_id(self, resource_name: str, hash: Optional[str] = None) -> str:
        """Returns a unique identifier for a resource
        An optional hash may be passed in to ensure uniqueness for edge cases"""

        return ".".join(
            filter(None, [self.resource_type, self.project.project_name, resource_name, hash])
        )


class Parser(BaseParser[FinalValue], Generic[FinalValue]):
    def __init__(
        self,
        project: Project,
        manifest: Manifest,
        root_project: RuntimeConfig,
    ) -> None:
        super().__init__(project, manifest)
        self.root_project = root_project


class RelationUpdate:
    def __init__(self, config: RuntimeConfig, manifest: Manifest, component: str) -> None:
        default_macro = manifest.find_generate_macro_by_name(
            component=component,
            root_project_name=config.project_name,
        )
        if default_macro is None:
            raise DbtInternalError(f"No macro with name generate_{component}_name found")

        default_macro_context = generate_generate_name_macro_context(
            default_macro, config, manifest
        )
        self.default_updater = MacroGenerator(default_macro, default_macro_context)

        package_names = config.dependencies.keys() if config.dependencies else {}
        package_updaters = {}
        for package_name in package_names:
            package_macro = manifest.find_generate_macro_by_name(
                component=component,
                root_project_name=config.project_name,
                imported_package=package_name,
            )
            if package_macro:
                imported_macro_context = generate_generate_name_macro_context(
                    package_macro, config, manifest
                )
                package_updaters[package_macro.package_name] = MacroGenerator(
                    package_macro, imported_macro_context
                )

        self.package_updaters = package_updaters
        self.component = component

    def __call__(self, parsed_node: Any, override: Optional[str]) -> None:
        if parsed_node.package_name in self.package_updaters:
            new_value = self.package_updaters[parsed_node.package_name](override, parsed_node)
        else:
            new_value = self.default_updater(override, parsed_node)

        if isinstance(new_value, str):
            new_value = new_value.strip()
        setattr(parsed_node, self.component, new_value)


class ConfiguredParser(
    Parser[FinalNode],
    Generic[ConfiguredBlockType, IntermediateNode, FinalNode],
):
    def __init__(
        self,
        project: Project,
        manifest: Manifest,
        root_project: RuntimeConfig,
    ) -> None:
        super().__init__(project, manifest, root_project)

        self._update_node_database = RelationUpdate(
            manifest=manifest, config=root_project, component="database"
        )
        self._update_node_schema = RelationUpdate(
            manifest=manifest, config=root_project, component="schema"
        )
        self._update_node_alias = RelationUpdate(
            manifest=manifest, config=root_project, component="alias"
        )

    @classmethod
    @abc.abstractmethod
    def get_compiled_path(cls, block: ConfiguredBlockType) -> str:
        pass

    @abc.abstractmethod
    def parse_from_dict(self, dict, validate=True) -> IntermediateNode:
        pass

    @abc.abstractproperty
    def resource_type(self) -> NodeType:
        pass

    @property
    def default_schema(self):
        return self.root_project.credentials.schema

    @property
    def default_database(self):
        return self.root_project.credentials.database

    def get_fqn_prefix(self, path: str) -> List[str]:
        no_ext = os.path.splitext(path)[0]
        fqn = [self.project.project_name]
        fqn.extend(utils.split_path(no_ext)[:-1])
        return fqn

    def get_fqn(self, path: str, name: str) -> List[str]:
        """Get the FQN for the node. This impacts node selection and config
        application.
        """
        fqn = self.get_fqn_prefix(path)
        fqn.append(name)
        return fqn

    def _mangle_hooks(self, config):
        """Given a config dict that may have `pre-hook`/`post-hook` keys,
        convert it from the yucky maybe-a-string, maybe-a-dict to a dict.
        """
        # Like most of parsing, this is a horrible hack :(
        for key in hooks.ModelHookType:
            if key in config:
                config[key] = [hooks.get_hook_dict(h) for h in config[key]]

    def _create_error_node(
        self, name: str, path: str, original_file_path: str, raw_code: str, language: str = "sql"
    ) -> UnparsedNode:
        """If we hit an error before we've actually parsed a node, provide some
        level of useful information by attaching this to the exception.
        """
        # this is a bit silly, but build an UnparsedNode just for error
        # message reasons
        return UnparsedNode(
            name=name,
            resource_type=self.resource_type,
            path=path,
            original_file_path=original_file_path,
            package_name=self.project.project_name,
            raw_code=raw_code,
            language=language,
        )

    def _create_parsetime_node(
        self,
        block: ConfiguredBlockType,
        path: str,
        config: ContextConfig,
        fqn: List[str],
        name=None,
        **kwargs,
    ) -> IntermediateNode:
        """Create the node that will be passed in to the parser context for
        "rendering". Some information may be partial, as it'll be updated by
        config() and any ref()/source() calls discovered during rendering.
        """
        if name is None:
            name = block.name
        if block.path.relative_path.endswith(".py"):
            language = ModelLanguage.python
            config.add_config_call({"materialized": "table"})
        else:
            # this is not ideal but we have a lot of tests to adjust if don't do it
            language = ModelLanguage.sql

        dct = {
            "alias": name,
            "schema": self.default_schema,
            "database": self.default_database,
            "fqn": fqn,
            "name": name,
            "resource_type": self.resource_type,
            "path": path,
            "original_file_path": block.path.original_file_path,
            "package_name": self.project.project_name,
            "raw_code": block.contents,
            "language": language,
            "unique_id": self.generate_unique_id(name),
            "config": self.config_dict(config),
            "checksum": block.file.checksum.to_dict(omit_none=True),
        }
        dct.update(kwargs)

        try:
            return self.parse_from_dict(dct, validate=True)
        except ValidationError as exc:
            # this is a bit silly, but build an UnparsedNode just for error
            # message reasons
            node = self._create_error_node(
                name=block.name,
                path=path,
                original_file_path=block.path.original_file_path,
                raw_code=block.contents,
            )
            raise DictParseError(exc, node=node)

    def _context_for(self, parsed_node: IntermediateNode, config: ContextConfig) -> Dict[str, Any]:
        return generate_parser_model_context(parsed_node, self.root_project, self.manifest, config)

    def render_with_context(self, parsed_node: IntermediateNode, config: ContextConfig):
        # Given the parsed node and a ContextConfig to use during parsing,
        # render the node's sql with macro capture enabled.
        # Note: this mutates the config object when config calls are rendered.
        context = self._context_for(parsed_node, config)

        # this goes through the process of rendering, but just throws away
        # the rendered result. The "macro capture" is the point?
        get_rendered(parsed_node.raw_code, context, parsed_node, capture_macros=True)
        return context

    # This is taking the original config for the node, converting it to a dict,
    # updating the config with new config passed in, then re-creating the
    # config from the dict in the node.
    def update_parsed_node_config_dict(
        self, parsed_node: IntermediateNode, config_dict: Dict[str, Any]
    ) -> None:
        # Overwrite node config
        final_config_dict = parsed_node.config.to_dict(omit_none=True)
        final_config_dict.update({k.strip(): v for (k, v) in config_dict.items()})
        # re-mangle hooks, in case we got new ones
        self._mangle_hooks(final_config_dict)
        parsed_node.config = parsed_node.config.from_dict(final_config_dict)

    def update_parsed_node_relation_names(
        self, parsed_node: IntermediateNode, config_dict: Dict[str, Any]
    ) -> None:

        # These call the RelationUpdate callable to go through generate_name macros
        self._update_node_database(parsed_node, config_dict.get("database"))
        self._update_node_schema(parsed_node, config_dict.get("schema"))
        self._update_node_alias(parsed_node, config_dict.get("alias"))

        # Snapshot nodes use special "target_database" and "target_schema" fields for some reason
        if parsed_node.resource_type == NodeType.Snapshot:
            if "target_database" in config_dict and config_dict["target_database"]:
                parsed_node.database = config_dict["target_database"]
            if "target_schema" in config_dict and config_dict["target_schema"]:
                parsed_node.schema = config_dict["target_schema"]

        self._update_node_relation_name(parsed_node)

    def update_parsed_node_config(
        self,
        parsed_node: IntermediateNode,
        config: ContextConfig,
        context=None,
        patch_config_dict=None,
    ) -> None:
        """Given the ContextConfig used for parsing and the parsed node,
        generate and set the true values to use, overriding the temporary parse
        values set in _build_intermediate_parsed_node.
        """

        # build_config_dict takes the config_call_dict in the ContextConfig object
        # and calls calculate_node_config to combine dbt_project configs and
        # config calls from SQL files, plus patch configs (from schema files)
        config_dict = config.build_config_dict(patch_config_dict=patch_config_dict)

        # Set tags on node provided in config blocks. Tags are additive, so even if
        # config has been built before, we don't have to reset tags in the parsed_node.
        model_tags = config_dict.get("tags", [])
        for tag in model_tags:
            if tag not in parsed_node.tags:
                parsed_node.tags.append(tag)

        # If we have meta in the config, copy to node level, for backwards
        # compatibility with earlier node-only config.
        if "meta" in config_dict and config_dict["meta"]:
            parsed_node.meta = config_dict["meta"]

        # If we have group in the config, copy to node level
        if "group" in config_dict and config_dict["group"]:
            parsed_node.group = config_dict["group"]

        # If we have access in the config, copy to node level
        if parsed_node.resource_type == NodeType.Model and config_dict.get("access", None):
            if AccessType.is_valid(config_dict["access"]):
                parsed_node.access = AccessType(config_dict["access"])
            else:
                raise InvalidAccessTypeError(
                    unique_id=parsed_node.unique_id, field_value=config_dict["access"]
                )

        # If we have docs in the config, merge with the node level, for backwards
        # compatibility with earlier node-only config.
        if "docs" in config_dict and config_dict["docs"]:
            # we set show at the value of the config if it is set, otherwise, inherit the value
            docs_show = (
                config_dict["docs"]["show"]
                if "show" in config_dict["docs"]
                else parsed_node.docs.show
            )
            if "node_color" in config_dict["docs"]:
                parsed_node.docs = Docs(
                    show=docs_show, node_color=config_dict["docs"]["node_color"]
                )
            else:
                parsed_node.docs = Docs(show=docs_show)

        # If we have contract in the config, copy to node level
        if "contract" in config_dict and config_dict["contract"]:
            contract_dct = config_dict["contract"]
            Contract.validate(contract_dct)
            parsed_node.contract = Contract.from_dict(contract_dct)

        # unrendered_config is used to compare the original database/schema/alias
        # values and to handle 'same_config' and 'same_contents' calls
        parsed_node.unrendered_config = config.build_config_dict(
            rendered=False, patch_config_dict=patch_config_dict
        )

        parsed_node.config_call_dict = config._config_call_dict

        # do this once before we parse the node database/schema/alias, so
        # parsed_node.config is what it would be if they did nothing
        self.update_parsed_node_config_dict(parsed_node, config_dict)
        # This updates the node database/schema/alias/relation_name
        self.update_parsed_node_relation_names(parsed_node, config_dict)

        # tests don't have hooks
        if parsed_node.resource_type == NodeType.Test:
            return

        # at this point, we've collected our hooks. Use the node context to
        # render each hook and collect refs/sources
        hooks = list(itertools.chain(parsed_node.config.pre_hook, parsed_node.config.post_hook))
        # skip context rebuilding if there aren't any hooks
        if not hooks:
            return
        if not context:
            context = self._context_for(parsed_node, config)
        for hook in hooks:
            get_rendered(hook.sql, context, parsed_node, capture_macros=True)

    def initial_config(self, fqn: List[str]) -> ContextConfig:
        config_version = min([self.project.config_version, self.root_project.config_version])
        if config_version == 2:
            return ContextConfig(
                self.root_project,
                fqn,
                self.resource_type,
                self.project.project_name,
            )
        else:
            raise DbtInternalError(
                f"Got an unexpected project version={config_version}, expected 2"
            )

    def config_dict(
        self,
        config: ContextConfig,
    ) -> Dict[str, Any]:
        config_dict = config.build_config_dict(base=True)
        self._mangle_hooks(config_dict)
        return config_dict

    def render_update(self, node: IntermediateNode, config: ContextConfig) -> None:
        try:
            context = self.render_with_context(node, config)
            self.update_parsed_node_config(node, config, context=context)
        except ValidationError as exc:
            # we got a ValidationError - probably bad types in config()
            raise ConfigUpdateError(exc, node=node) from exc

    def add_result_node(self, block: FileBlock, node: ManifestNode):
        if node.config.enabled:
            self.manifest.add_node(block.file, node)
        else:
            self.manifest.add_disabled(block.file, node)

    def parse_node(self, block: ConfiguredBlockType) -> FinalNode:
        compiled_path: str = self.get_compiled_path(block)
        fqn = self.get_fqn(compiled_path, block.name)

        config: ContextConfig = self.initial_config(fqn)

        node = self._create_parsetime_node(
            block=block,
            path=compiled_path,
            config=config,
            fqn=fqn,
        )
        self.render_update(node, config)
        result = self.transform(node)
        self.add_result_node(block, result)
        return result

    def _update_node_relation_name(self, node: ManifestNode):
        # Seed and Snapshot nodes and Models that are not ephemeral,
        # and TestNodes that store_failures.
        # TestNodes do not get a relation_name without store failures
        # because no schema is created.
        if node.is_relational and not node.is_ephemeral_model:
            adapter = get_adapter(self.root_project)
            relation_cls = adapter.Relation
            node.relation_name = str(relation_cls.create_from(self.root_project, node))
        else:
            # Set it to None in case it changed with a config update
            node.relation_name = None

    @abc.abstractmethod
    def parse_file(self, file_block: FileBlock) -> None:
        pass

    @abc.abstractmethod
    def transform(self, node: IntermediateNode) -> FinalNode:
        pass


class SimpleParser(
    ConfiguredParser[ConfiguredBlockType, FinalNode, FinalNode],
    Generic[ConfiguredBlockType, FinalNode],
):
    def transform(self, node):
        return node


class SQLParser(
    ConfiguredParser[FileBlock, IntermediateNode, FinalNode], Generic[IntermediateNode, FinalNode]
):
    def parse_file(self, file_block: FileBlock) -> None:
        self.parse_node(file_block)


class SimpleSQLParser(SQLParser[FinalNode, FinalNode]):
    def transform(self, node):
        return node
