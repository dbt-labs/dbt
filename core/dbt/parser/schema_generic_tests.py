import pathlib
import itertools
import os

from typing import List, Dict, Optional, Union, Any
from dbt.parser.base import SimpleParser
from dbt.parser.generic_test_builders import TestBuilder
from dbt.parser.search import FileBlock
from dbt.context.providers import RefArgs, generate_test_context
from dbt.parser.common import (
    TestBlock,
    Testable,
    TestDef,
    GenericTestBlock,
    VersionedTestBlock,
    trimmed,
)
from dbt.contracts.graph.unparsed import UnparsedNodeUpdate, NodeVersion, UnparsedColumn
from dbt.contracts.graph.nodes import (
    GenericTestNode,
    UnpatchedSourceDefinition,
    ManifestNode,
    GraphMemberNode,
)
from dbt.context.context_config import ContextConfig
from dbt.context.configured import generate_schema_yml_context, SchemaYamlVars
from dbt.common.dataclass_schema import ValidationError
from dbt.exceptions import SchemaConfigError, CompilationError, ParsingError, TestConfigError
from dbt.contracts.files import FileHash
from dbt.utils import md5, get_pseudo_test_path
from dbt.clients.jinja import get_rendered, add_rendered_test_kwargs
from dbt.clients.adapter import get_adapter, get_adapter_package_names
from dbt.node_types import NodeType
from dbt.context.macro_resolver import MacroResolver


# This parser handles the tests that are defined in "schema" (yaml) files, on models,
# sources, etc. The base generic test is handled by the GenericTestParser
class SchemaGenericTestParser(SimpleParser):
    def __init__(
        self,
        project,
        manifest,
        root_project,
    ) -> None:
        super().__init__(project, manifest, root_project)
        self.schema_yaml_vars = SchemaYamlVars()
        self.render_ctx = generate_schema_yml_context(
            self.root_project, self.project.project_name, self.schema_yaml_vars
        )
        internal_package_names = get_adapter_package_names(self.root_project.credentials.type)
        self.macro_resolver = MacroResolver(
            self.manifest.macros, self.root_project.project_name, internal_package_names
        )

    @property
    def resource_type(self) -> NodeType:
        return NodeType.Test

    @classmethod
    def get_compiled_path(cls, block: FileBlock) -> str:
        return block.path.relative_path

    def parse_file(self, block: FileBlock, dct: Optional[Dict] = None) -> None:
        pass

    def parse_from_dict(self, dct, validate=True) -> GenericTestNode:
        if validate:
            GenericTestNode.validate(dct)
        return GenericTestNode.from_dict(dct)

    def parse_column_tests(
        self, block: TestBlock, column: UnparsedColumn, version: Optional[NodeVersion]
    ) -> None:
        if not column.tests:
            return

        for test in column.tests:
            self.parse_test(block, test, column, version)

    def create_test_node(
        self,
        target: Union[UnpatchedSourceDefinition, UnparsedNodeUpdate],
        path: str,
        config: ContextConfig,
        tags: List[str],
        fqn: List[str],
        name: str,
        raw_code: str,
        test_metadata: Dict[str, Any],
        file_key_name: str,
        column_name: Optional[str],
    ) -> GenericTestNode:

        HASH_LENGTH = 10

        # N.B: This function builds a hashable string from any given test_metadata dict.
        #   it's a bit fragile for general use (only supports str, int, float, List, Dict)
        #   but it gets the job done here without the overhead of complete ser(de).
        def get_hashable_md(data: Union[str, int, float, List, Dict]) -> Union[str, List, Dict]:
            if type(data) == dict:
                return {k: get_hashable_md(data[k]) for k in sorted(data.keys())}  # type: ignore
            elif type(data) == list:
                return [get_hashable_md(val) for val in data]  # type: ignore
            else:
                return str(data)

        hashable_metadata = repr(get_hashable_md(test_metadata))
        hash_string = "".join([name, hashable_metadata])
        test_hash = md5(hash_string)[-HASH_LENGTH:]

        dct = {
            "alias": name,
            "schema": self.default_schema,
            "database": self.default_database,
            "fqn": fqn,
            "name": name,
            "resource_type": self.resource_type,
            "tags": tags,
            "path": path,
            "original_file_path": target.original_file_path,
            "package_name": self.project.project_name,
            "raw_code": raw_code,
            "language": "sql",
            "unique_id": self.generate_unique_id(name, test_hash),
            "config": self.config_dict(config),
            "test_metadata": test_metadata,
            "column_name": column_name,
            "checksum": FileHash.empty().to_dict(omit_none=True),
            "file_key_name": file_key_name,
        }
        try:
            GenericTestNode.validate(dct)
            return GenericTestNode.from_dict(dct)
        except ValidationError as exc:
            # this is a bit silly, but build an UnparsedNode just for error
            # message reasons
            node = self._create_error_node(
                name=target.name,
                path=path,
                original_file_path=target.original_file_path,
                raw_code=raw_code,
            )
            raise TestConfigError(exc, node)

    # This is called directly in the SourcePatcher and by the "parse_node"
    # command which is called by the SchemaParser.
    def parse_generic_test(
        self,
        target: Testable,
        test: Dict[str, Any],
        tags: List[str],
        column_name: Optional[str],
        schema_file_id: str,
        version: Optional[NodeVersion],
    ) -> GenericTestNode:
        try:
            builder = TestBuilder(
                test=test,
                target=target,
                column_name=column_name,
                version=version,
                package_name=target.package_name,
                render_ctx=self.render_ctx,
            )
            if self.schema_yaml_vars.env_vars:
                self.store_env_vars(target, schema_file_id, self.schema_yaml_vars.env_vars)
                self.schema_yaml_vars.env_vars = {}

        except ParsingError as exc:
            context = trimmed(str(target))
            msg = "Invalid test config given in {}:\n\t{}\n\t@: {}".format(
                target.original_file_path, exc.msg, context
            )
            raise ParsingError(msg) from exc

        except CompilationError as exc:
            context = trimmed(str(target))
            msg = (
                "Invalid generic test configuration given in "
                f"{target.original_file_path}: \n{exc.msg}\n\t@: {context}"
            )
            raise CompilationError(msg) from exc

        original_name = os.path.basename(target.original_file_path)
        compiled_path = get_pseudo_test_path(builder.compiled_name, original_name)

        # fqn is the relative path of the yaml file where this generic test is defined,
        # minus the project-level directory and the file name itself
        # TODO pass a consistent path object from both UnparsedNode and UnpatchedSourceDefinition
        path = pathlib.Path(target.original_file_path)
        relative_path = str(path.relative_to(*path.parts[:1]))
        fqn = self.get_fqn(relative_path, builder.fqn_name)

        # this is the ContextConfig that is used in render_update
        config: ContextConfig = self.initial_config(fqn)

        # builder.args contains keyword args for the test macro,
        # not configs which have been separated out in the builder.
        # The keyword args are not completely rendered until compilation.
        metadata = {
            "namespace": builder.namespace,
            "name": builder.name,
            "kwargs": builder.args,
        }
        tags = sorted(set(itertools.chain(tags, builder.tags())))

        if isinstance(target, UnpatchedSourceDefinition):
            file_key_name = f"{target.source.yaml_key}.{target.source.name}"
        else:
            file_key_name = f"{target.yaml_key}.{target.name}"

        node = self.create_test_node(
            target=target,
            path=compiled_path,
            config=config,
            fqn=fqn,
            tags=tags,
            name=builder.fqn_name,
            raw_code=builder.build_raw_code(),
            column_name=column_name,
            test_metadata=metadata,
            file_key_name=file_key_name,
        )
        self.render_test_update(node, config, builder, schema_file_id)

        return node

    def _lookup_attached_node(
        self, target: Testable, version: Optional[NodeVersion]
    ) -> Optional[Union[ManifestNode, GraphMemberNode]]:
        """Look up attached node for Testable target nodes other than sources. Can be None if generic test attached to SQL node with no corresponding .sql file."""
        attached_node = None  # type: Optional[Union[ManifestNode, GraphMemberNode]]
        if not isinstance(target, UnpatchedSourceDefinition):
            attached_node_unique_id = self.manifest.ref_lookup.get_unique_id(
                target.name, target.package_name, version
            )
            if attached_node_unique_id:
                attached_node = self.manifest.nodes[attached_node_unique_id]
            else:
                disabled_node = self.manifest.disabled_lookup.find(
                    target.name, None
                ) or self.manifest.disabled_lookup.find(target.name.upper(), None)
                if disabled_node:
                    attached_node = self.manifest.disabled[disabled_node[0].unique_id][0]
        return attached_node

    def store_env_vars(self, target, schema_file_id, env_vars):
        self.manifest.env_vars.update(env_vars)
        if schema_file_id in self.manifest.files:
            schema_file = self.manifest.files[schema_file_id]
            if isinstance(target, UnpatchedSourceDefinition):
                search_name = target.source.name
                yaml_key = target.source.yaml_key
                if "." in search_name:  # source file definitions
                    (search_name, _) = search_name.split(".")
            else:
                search_name = target.name
                yaml_key = target.yaml_key
            for var in env_vars.keys():
                schema_file.add_env_var(var, yaml_key, search_name)

    # This does special shortcut processing for the two
    # most common internal macros, not_null and unique,
    # which avoids the jinja rendering to resolve config
    # and variables, etc, which might be in the macro.
    # In the future we will look at generalizing this
    # more to handle additional macros or to use static
    # parsing to avoid jinja overhead.
    def render_test_update(self, node, config, builder, schema_file_id):
        macro_unique_id = self.macro_resolver.get_macro_id(
            node.package_name, "test_" + builder.name
        )
        # Add the depends_on here so we can limit the macros added
        # to the context in rendering processing
        node.depends_on.add_macro(macro_unique_id)
        if macro_unique_id in ["macro.dbt.test_not_null", "macro.dbt.test_unique"]:
            config_call_dict = builder.get_static_config()
            config._config_call_dict = config_call_dict
            # This sets the config from dbt_project
            self.update_parsed_node_config(node, config)
            # source node tests are processed at patch_source time
            if isinstance(builder.target, UnpatchedSourceDefinition):
                sources = [builder.target.fqn[-2], builder.target.fqn[-1]]
                node.sources.append(sources)
            else:  # all other nodes
                node.refs.append(RefArgs(name=builder.target.name, version=builder.version))
        else:
            try:
                # make a base context that doesn't have the magic kwargs field
                context = generate_test_context(
                    node,
                    self.root_project,
                    self.manifest,
                    config,
                    self.macro_resolver,
                )
                # update with rendered test kwargs (which collects any refs)
                # Note: This does not actually update the kwargs with the rendered
                # values. That happens in compilation.
                add_rendered_test_kwargs(context, node, capture_macros=True)
                # the parsed node is not rendered in the native context.
                get_rendered(node.raw_code, context, node, capture_macros=True)
                self.update_parsed_node_config(node, config)
                # env_vars should have been updated in the context env_var method
            except ValidationError as exc:
                # we got a ValidationError - probably bad types in config()
                raise SchemaConfigError(exc, node=node) from exc

        # Set attached_node for generic test nodes, if available.
        # Generic test node inherits attached node's group config value.
        attached_node = self._lookup_attached_node(builder.target, builder.version)
        if attached_node:
            node.attached_node = attached_node.unique_id
            node.group, node.group = attached_node.group, attached_node.group

    def parse_node(self, block: GenericTestBlock) -> GenericTestNode:
        """In schema parsing, we rewrite most of the part of parse_node that
        builds the initial node to be parsed, but rendering is basically the
        same
        """
        node = self.parse_generic_test(
            target=block.target,
            test=block.test,
            tags=block.tags,
            column_name=block.column_name,
            schema_file_id=block.file.file_id,
            version=block.version,
        )
        self.add_test_node(block, node)
        return node

    def add_test_node(self, block: GenericTestBlock, node: GenericTestNode):
        test_from = {"key": block.target.yaml_key, "name": block.target.name}
        if node.config.enabled:
            self.manifest.add_node(block.file, node, test_from)
        else:
            self.manifest.add_disabled(block.file, node, test_from)

    def render_with_context(
        self,
        node: GenericTestNode,
        config: ContextConfig,
    ) -> None:
        """Given the parsed node and a ContextConfig to use during
        parsing, collect all the refs that might be squirreled away in the test
        arguments. This includes the implicit "model" argument.
        """
        # make a base context that doesn't have the magic kwargs field
        context = self._context_for(node, config)
        # update it with the rendered test kwargs (which collects any refs)
        add_rendered_test_kwargs(context, node, capture_macros=True)

        # the parsed node is not rendered in the native context.
        get_rendered(node.raw_code, context, node, capture_macros=True)

    def parse_test(
        self,
        target_block: TestBlock,
        test: TestDef,
        column: Optional[UnparsedColumn],
        version: Optional[NodeVersion],
    ) -> None:
        if isinstance(test, str):
            test = {test: {}}

        if column is None:
            column_name: Optional[str] = None
            column_tags: List[str] = []
        else:
            column_name = column.name
            should_quote = column.quote or (column.quote is None and target_block.quote_columns)
            if should_quote:
                column_name = get_adapter(self.root_project).quote(column_name)
            column_tags = column.tags

        block = GenericTestBlock.from_test_block(
            src=target_block,
            test=test,
            column_name=column_name,
            tags=column_tags,
            version=version,
        )
        self.parse_node(block)

    def parse_tests(self, block: TestBlock) -> None:
        for column in block.columns:
            self.parse_column_tests(block, column, None)

        for test in block.tests:
            self.parse_test(block, test, None, None)

    def parse_versioned_tests(self, block: VersionedTestBlock) -> None:
        if not block.target.versions:
            self.parse_tests(block)
        else:
            for version in block.target.versions:
                for column in block.target.get_columns_for_version(version.v):
                    self.parse_column_tests(block, column, version.v)

                for test in block.target.get_tests_for_version(version.v):
                    self.parse_test(block, test, None, version.v)

    def generate_unique_id(self, resource_name: str, hash: Optional[str] = None) -> str:
        return ".".join(
            filter(None, [self.resource_type, self.project.project_name, resource_name, hash])
        )
