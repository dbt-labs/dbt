import abc
import os
from typing import Dict, Any

import dbt.exceptions
import dbt.flags
import dbt.include
import dbt.utils
import dbt.hooks
import dbt.clients.jinja
import dbt.context.parser

from dbt.include.global_project import PROJECT_NAME as GLOBAL_PROJECT_NAME
from dbt.utils import coalesce
from dbt.logger import GLOBAL_LOGGER as logger
from dbt.contracts.project import ProjectList
from dbt.parser.source_config import SourceConfig
from dbt import deprecations
from dbt import hooks


class BaseParser(metaclass=abc.ABCMeta):
    def __init__(self, root_project_config, all_projects: ProjectList):
        self.root_project_config = root_project_config
        self.all_projects = all_projects
        if dbt.flags.STRICT_MODE:
            dct = {
                'projects': {
                    name: project.to_project_config(with_packages=True)
                    for name, project in all_projects.items()
                }
            }
            ProjectList.from_dict(dct, validate=True)

    @property
    def default_schema(self):
        return getattr(self.root_project_config.credentials, 'schema',
                       'public')

    @property
    def default_database(self):
        return getattr(self.root_project_config.credentials, 'database', 'dbt')

    def load_and_parse(self, *args, **kwargs):
        raise dbt.exceptions.NotImplementedException("Not implemented")

    @classmethod
    def get_path(cls, resource_type, package_name, resource_name):
        """Returns a unique identifier for a resource"""

        return "{}.{}.{}".format(resource_type, package_name, resource_name)

    @classmethod
    def get_fqn(cls, node, package_project_config, extra=[]):
        parts = dbt.utils.split_path(node.path)
        name, _ = os.path.splitext(parts[-1])
        fqn = ([package_project_config.project_name] +
               parts[:-1] +
               extra +
               [name])

        return fqn


class MacrosKnownParser(BaseParser):
    def __init__(self, root_project_config, all_projects, macro_manifest):
        super().__init__(
            root_project_config=root_project_config,
            all_projects=all_projects
        )
        self.macro_manifest = macro_manifest
        self._get_schema_func = None
        self._get_alias_func = None

    def get_schema_func(self):
        """The get_schema function is set by a few different things:
            - if there is a 'generate_schema_name' macro in the root project,
                it will be used.
            - if that does not exist but there is a 'generate_schema_name'
                macro in the 'dbt' internal project, that will be used
            - if neither of those exist (unit tests?), a function that returns
                the 'default schema' as set in the root project's 'credentials'
                is used
        """
        if self._get_schema_func is not None:
            return self._get_schema_func

        get_schema_macro = self.macro_manifest.find_macro_by_name(
            'generate_schema_name',
            self.root_project_config.project_name
        )
        if get_schema_macro is None:
            get_schema_macro = self.macro_manifest.find_macro_by_name(
                'generate_schema_name',
                GLOBAL_PROJECT_NAME
            )
        # this is only true in tests!
        if get_schema_macro is None:
            def get_schema(custom_schema_name=None, node=None):
                return self.default_schema
        else:
            root_context = dbt.context.parser.generate_macro(
                get_schema_macro, self.root_project_config,
                self.macro_manifest
            )
            get_schema = get_schema_macro.generator(root_context)

        self._get_schema_func = get_schema
        return self._get_schema_func

    def get_alias_func(self):
        """The get_alias function is set by a few different things:
            - if there is a 'generate_alias_name' macro in the root project,
                it will be used.
            - if that does not exist but there is a 'generate_alias_name'
                macro in the 'dbt' internal project, that will be used
            - if neither of those exist (unit tests?), a function that returns
                the 'default alias' as set in the model's filename or alias
                configuration.
        """
        if self._get_alias_func is not None:
            return self._get_alias_func

        get_alias_macro = self.macro_manifest.find_macro_by_name(
            'generate_alias_name',
            self.root_project_config.project_name
        )
        if get_alias_macro is None:
            get_alias_macro = self.macro_manifest.find_macro_by_name(
                'generate_alias_name',
                GLOBAL_PROJECT_NAME
            )

        # the generate_alias_name macro might not exist
        if get_alias_macro is None:
            def get_alias(custom_alias_name, node):
                if custom_alias_name is None:
                    return node.name
                else:
                    return custom_alias_name
        else:
            root_context = dbt.context.parser.generate_macro(
                get_alias_macro, self.root_project_config,
                self.macro_manifest
            )
            get_alias = get_alias_macro.generator(root_context)

        self._get_alias_func = get_alias
        return self._get_alias_func

    def _mangle_hooks(self, config):
        """Given a config dict that may have `pre-hook`/`post-hook` keys,
        convert it from the yucky maybe-a-string, maybe-a-dict to a dict.
        """
        # Like most of parsing, this is a horrible hack :(
        for key in hooks.ModelHookType:
            if key in config:
                config[key] = [hooks.get_hook_dict(h) for h in config[key]]

    def _build_intermediate_node_dict(self, config, node_dict, node_path,
                                      package_project_config, tags, fqn,
                                      snapshot_config, column_name):
        """Update the unparsed node dictionary and build the basis for an
        intermediate ParsedNode that will be passed into the renderer
        """
        # Set this temporarily. Not the full config yet (as config() hasn't
        # been called from jinja yet). But the Var() call below needs info
        # about project level configs b/c they might contain refs.
        # TODO: Restructure this?
        config_dict = coalesce(snapshot_config, {})
        config_dict.update(config.config)
        self._mangle_hooks(config_dict)

        node_dict.update({
            'refs': [],
            'sources': [],
            'depends_on': {
                'nodes': [],
                'macros': [],
            },
            'unique_id': node_path,
            'fqn': fqn,
            'tags': tags,
            'config': config_dict,
            # Set these temporarily so get_rendered() has access to a schema,
            # database, and alias.
            'schema': self.default_schema,
            'database': self.default_database,
            'alias': node_dict.get('name'),
        })

        # if there's a column, it should end up part of the ParsedNode
        if column_name is not None:
            node_dict['column_name'] = column_name

        return node_dict

    def _render_with_context(self, parsed_node, config):
        """Given the parsed node and a SourceConfig to use during parsing,
        render the node's sql wtih macro capture enabled.

        Note: this mutates the config object when config() calls are rendered.
        """
        context = dbt.context.parser.generate(
            parsed_node,
            self.root_project_config,
            self.macro_manifest,
            config)

        dbt.clients.jinja.get_rendered(
            parsed_node.raw_sql, context, parsed_node,
            capture_macros=True)

    def _update_parsed_node_info(self, parsed_node, config):
        """Given the SourceConfig used for parsing and the parsed node,
        generate and set the true values to use, overriding the temporary parse
        values set in _build_intermediate_parsed_node.
        """
        # Special macro defined in the global project. Use the root project's
        # definition, not the current package
        schema_override = config.config.get('schema')
        get_schema = self.get_schema_func()
        try:
            schema = get_schema(schema_override, parsed_node)
        except dbt.exceptions.CompilationException as exc:
            too_many_args = (
                "macro 'dbt_macro__generate_schema_name' takes not more than "
                "1 argument(s)"
            )
            if too_many_args not in str(exc):
                raise
            deprecations.warn('generate-schema-name-single-arg')
            schema = get_schema(schema_override)
        parsed_node.schema = schema.strip()

        alias_override = config.config.get('alias')
        get_alias = self.get_alias_func()
        parsed_node.alias = get_alias(alias_override, parsed_node).strip()

        parsed_node.database = config.config.get(
            'database', self.default_database
        ).strip()

        # Set tags on node provided in config blocks
        model_tags = config.config.get('tags', [])
        parsed_node.tags.extend(model_tags)

        # Overwrite node config
        config_dict = parsed_node.config.to_dict()
        config_dict.update(config.config)
        # re-mangle hooks, in case we got new ones
        self._mangle_hooks(config_dict)
        parsed_node.config = parsed_node.config.from_dict(config_dict)

    @abc.abstractmethod
    def parse_from_dict(self, parsed_dict: Dict[str, Any]) -> Any:
        """Given a dictionary, return the parsed entity for this parser"""

    def parse_node(self, node, node_path, package_project_config, tags=None,
                   fqn_extra=None, fqn=None, snapshot_config=None,
                   column_name=None):
        """Parse a node, given an UnparsedNode and any other required information.

        snapshot_config should be set if the node is an Snapshot node.
        column_name should be set if the node is a Test node associated with a
        particular column.
        """
        logger.debug("Parsing {}".format(node_path))

        tags = coalesce(tags, [])
        fqn_extra = coalesce(fqn_extra, [])

        if fqn is None:
            fqn = self.get_fqn(node, package_project_config, fqn_extra)

        config = SourceConfig(
            self.root_project_config,
            package_project_config,
            fqn,
            node.resource_type)

        parsed_dict = self._build_intermediate_node_dict(
            config, node.to_dict(), node_path, config, tags, fqn,
            snapshot_config, column_name
        )
        parsed_node = self.parse_from_dict(parsed_dict)

        self._render_with_context(parsed_node, config)
        self._update_parsed_node_info(parsed_node, config)

        parsed_node.to_dict(validate=True)

        return parsed_node

    def check_block_parsing(self, name, path, contents):
        """Check if we were able to extract toplevel blocks from the given
        contents. Return True if extraction was successful (no exceptions),
        False if it fails.
        """
        if not dbt.flags.TEST_NEW_PARSER:
            return True
        try:
            dbt.clients.jinja.extract_toplevel_blocks(contents)
        except Exception:
            return False
        return True
