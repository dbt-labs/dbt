import json
import os
import unittest
from copy import deepcopy
from typing import Any, Dict
from unittest import mock

import pytest

import dbt.config
import dbt.exceptions
from dbt import deprecations
from dbt.adapters.contracts.connection import DEFAULT_QUERY_COMMENT, QueryComment
from dbt.adapters.factory import load_plugin
from dbt.config.project import Project, _get_required_version, read_project_flags
from dbt.constants import DEPENDENCIES_FILE_NAME
from dbt.contracts.project import GitPackage, LocalPackage, PackageConfig, ProjectFlags
from dbt.deprecations import (
    GenericJSONSchemaValidationDeprecation as GenericJSONSchemaValidationDeprecationCore,
)
from dbt.events.types import GenericJSONSchemaValidationDeprecation, LogDbtProjectError
from dbt.flags import set_from_args
from dbt.jsonschemas.jsonschemas import project_schema
from dbt.node_types import NodeType
from dbt.tests.util import safe_set_invocation_context
from dbt_common.events.event_catcher import EventCatcher
from dbt_common.events.event_manager_client import get_event_manager
from dbt_common.events.types import Note
from dbt_common.exceptions import DbtRuntimeError
from dbt_common.semver import VersionSpecifier
from tests.unit.config import (
    BaseConfigTest,
    empty_project_renderer,
    project_from_config_norender,
    project_from_config_rendered,
)


class TestProjectMethods:
    def test_all_source_paths(self, project: Project):
        assert (
            project.all_source_paths.sort()
            == ["models", "seeds", "snapshots", "analyses", "macros", "tests"].sort()
        )

    def test_generic_test_paths(self, project: Project):
        assert project.generic_test_paths == ["tests/generic"]

    def test_fixture_paths(self, project: Project):
        assert project.fixture_paths == ["tests/fixtures"]

    def test__str__(self, project: Project):
        assert (
            str(project)
            == "{'name': 'test_project', 'version': 1.0, 'project-root': 'doesnt/actually/exist', 'profile': 'test_profile', 'model-paths': ['models'], 'macro-paths': ['macros'], 'seed-paths': ['seeds'], 'test-paths': ['tests'], 'analysis-paths': ['analyses'], 'docs-paths': ['docs'], 'asset-paths': ['assets'], 'target-path': 'target', 'snapshot-paths': ['snapshots'], 'osi-paths': ['osi', 'OSI'], 'clean-targets': ['target'], 'log-path': 'path/to/project/logs', 'quoting': {}, 'models': {}, 'on-run-start': [], 'on-run-end': [], 'dispatch': [{'macro_namespace': 'dbt_utils', 'search_order': ['test_project', 'dbt_utils']}], 'seeds': {}, 'snapshots': {}, 'sources': {}, 'data_tests': {}, 'unit_tests': {}, 'metrics': {}, 'semantic-models': {}, 'saved-queries': {}, 'analyses': {}, 'exposures': {}, 'functions': {}, 'vars': {}, 'require-dbt-version': ['=0.0.0'], 'restrict-access': False, 'dbt-cloud': {}, 'flags': {}, 'query-comment': {'comment': \"\\n{%- set comment_dict = {} -%}\\n{%- do comment_dict.update(\\n    app='dbt',\\n    dbt_version=dbt_version,\\n    profile_name=target.get('profile_name'),\\n    target_name=target.get('target_name'),\\n) -%}\\n{%- if node is not none -%}\\n  {%- do comment_dict.update(\\n    node_id=node.unique_id,\\n  ) -%}\\n{% else %}\\n  {# in the node context, the connection name is the node_id #}\\n  {%- do comment_dict.update(connection_name=connection_name) -%}\\n{%- endif -%}\\n{{ return(tojson(comment_dict)) }}\\n\", 'append': False, 'job-label': False}, 'packages': []}"
        )

    def test_get_selector(self, project: Project):
        selector = project.get_selector("my_selector")
        assert selector.raw == "give me cats"

        with pytest.raises(DbtRuntimeError):
            project.get_selector("doesnt_exist")

    def test_get_default_selector_name(self, project: Project):
        default_selector_name = project.get_default_selector_name()
        assert default_selector_name == "my_selector"

        project.selectors["my_selector"]["default"] = False
        default_selector_name = project.get_default_selector_name()
        assert default_selector_name is None

    def test_get_macro_search_order(self, project: Project):
        search_order = project.get_macro_search_order("dbt_utils")
        assert search_order == ["test_project", "dbt_utils"]

        search_order = project.get_macro_search_order("doesnt_exist")
        assert search_order is None

    def test_project_target_path(self, project: Project):
        assert project.project_target_path == "doesnt/actually/exist/target"

    def test_eq(self, project: Project):
        other = deepcopy(project)
        assert project == other

    def test_neq(self, project: Project):
        other = deepcopy(project)
        other.project_name = "other project"
        assert project != other

    def test_hashed_name(self, project: Project):
        assert project.hashed_name() == "6e72a69d5c5cca8f0400338441c022e4"


class TestProjectInitialization(BaseConfigTest):
    def test_defaults(self):
        project = project_from_config_norender(
            self.default_project_data, project_root=self.project_dir
        )
        self.assertEqual(project.project_name, "my_test_project")
        self.assertEqual(project.version, "0.0.1")
        self.assertEqual(project.profile_name, "default")
        self.assertEqual(project.project_root, self.project_dir)
        self.assertEqual(project.model_paths, ["models"])
        self.assertEqual(project.macro_paths, ["macros"])
        self.assertEqual(project.seed_paths, ["seeds"])
        self.assertEqual(project.test_paths, ["tests"])
        self.assertEqual(project.analysis_paths, ["analyses"])
        self.assertEqual(
            set(project.docs_paths),
            {"models", "seeds", "snapshots", "analyses", "macros", "tests", "functions"},
        )
        self.assertEqual(project.asset_paths, [])
        self.assertEqual(project.target_path, "target")
        self.assertEqual(project.clean_targets, ["target"])
        self.assertEqual(project.log_path, "logs")
        self.assertEqual(project.packages_install_path, "dbt_packages")
        self.assertEqual(project.quoting, {})
        self.assertEqual(project.models, {})
        self.assertEqual(project.on_run_start, [])
        self.assertEqual(project.on_run_end, [])
        self.assertEqual(project.seeds, {})
        self.assertEqual(project.dbt_version, [VersionSpecifier.from_version_string(">=0.0.0")])
        self.assertEqual(project.packages, PackageConfig(packages=[]))
        # just make sure str() doesn't crash anything, that's always
        # embarrassing
        str(project)

    def test_implicit_overrides(self):
        self.default_project_data.update(
            {
                "model-paths": ["other-models"],
            }
        )
        project = project_from_config_norender(
            self.default_project_data, project_root=self.project_dir
        )
        self.assertEqual(
            set(project.docs_paths),
            {"other-models", "seeds", "snapshots", "analyses", "macros", "tests", "functions"},
        )

    def test_all_overrides(self):
        # log-path is not tested because it is set exclusively from flags, not cfg
        self.default_project_data.update(
            {
                "model-paths": ["other-models"],
                "macro-paths": ["other-macros"],
                "seed-paths": ["other-seeds"],
                "test-paths": ["other-tests"],
                "analysis-paths": ["other-analyses"],
                "docs-paths": ["docs"],
                "asset-paths": ["other-assets"],
                "clean-targets": ["another-target"],
                "packages-install-path": "other-dbt_packages",
                "quoting": {"identifier": False, "snowflake_ignore_case": True},
                "models": {
                    "pre-hook": ["{{ logging.log_model_start_event() }}"],
                    "post-hook": ["{{ logging.log_model_end_event() }}"],
                    "my_test_project": {
                        "first": {
                            "enabled": False,
                            "sub": {
                                "enabled": True,
                            },
                        },
                        "second": {
                            "materialized": "table",
                        },
                    },
                    "third_party": {
                        "third": {
                            "materialized": "view",
                        },
                    },
                },
                "on-run-start": [
                    "{{ logging.log_run_start_event() }}",
                ],
                "on-run-end": [
                    "{{ logging.log_run_end_event() }}",
                ],
                "seeds": {
                    "my_test_project": {
                        "enabled": True,
                        "schema": "seed_data",
                        "post-hook": "grant select on {{ this }} to bi_user",
                    },
                },
                "data_tests": {"my_test_project": {"fail_calc": "sum(failures)"}},
                "require-dbt-version": ">=0.1.0",
            }
        )
        packages = {
            "packages": [
                {
                    "local": "foo",
                },
                {"git": "git@example.com:dbt-labs/dbt-utils.git", "revision": "test-rev"},
            ],
        }
        project = project_from_config_norender(
            self.default_project_data, project_root=self.project_dir, packages=packages
        )
        self.assertEqual(project.project_name, "my_test_project")
        self.assertEqual(project.version, "0.0.1")
        self.assertEqual(project.profile_name, "default")
        self.assertEqual(project.model_paths, ["other-models"])
        self.assertEqual(project.macro_paths, ["other-macros"])
        self.assertEqual(project.seed_paths, ["other-seeds"])
        self.assertEqual(project.test_paths, ["other-tests"])
        self.assertEqual(project.analysis_paths, ["other-analyses"])
        self.assertEqual(project.docs_paths, ["docs"])
        self.assertEqual(project.asset_paths, ["other-assets"])
        self.assertEqual(project.clean_targets, ["another-target"])
        self.assertEqual(project.packages_install_path, "other-dbt_packages")
        self.assertEqual(project.quoting, {"identifier": False, "snowflake_ignore_case": True})
        self.assertEqual(
            project.models,
            {
                "pre-hook": ["{{ logging.log_model_start_event() }}"],
                "post-hook": ["{{ logging.log_model_end_event() }}"],
                "my_test_project": {
                    "first": {
                        "enabled": False,
                        "sub": {
                            "enabled": True,
                        },
                    },
                    "second": {
                        "materialized": "table",
                    },
                },
                "third_party": {
                    "third": {
                        "materialized": "view",
                    },
                },
            },
        )
        self.assertEqual(project.on_run_start, ["{{ logging.log_run_start_event() }}"])
        self.assertEqual(project.on_run_end, ["{{ logging.log_run_end_event() }}"])
        self.assertEqual(
            project.seeds,
            {
                "my_test_project": {
                    "enabled": True,
                    "schema": "seed_data",
                    "post-hook": "grant select on {{ this }} to bi_user",
                },
            },
        )
        self.assertEqual(
            project.data_tests,
            {
                "my_test_project": {"fail_calc": "sum(failures)"},
            },
        )
        self.assertEqual(project.dbt_version, [VersionSpecifier.from_version_string(">=0.1.0")])
        self.assertEqual(
            project.packages,
            PackageConfig(
                packages=[
                    LocalPackage(local="foo", unrendered={"local": "foo"}),
                    GitPackage(
                        git="git@example.com:dbt-labs/dbt-utils.git",
                        revision="test-rev",
                        unrendered={
                            "git": "git@example.com:dbt-labs/dbt-utils.git",
                            "revision": "test-rev",
                        },
                    ),
                ]
            ),
        )
        str(project)  # this does the equivalent of project.to_project_config(with_packages=True)
        json.dumps(project.to_project_config())

    def test_string_run_hooks(self):
        self.default_project_data.update(
            {
                "on-run-start": "{{ logging.log_run_start_event() }}",
                "on-run-end": "{{ logging.log_run_end_event() }}",
            }
        )
        project = project_from_config_rendered(self.default_project_data)
        self.assertEqual(project.on_run_start, ["{{ logging.log_run_start_event() }}"])
        self.assertEqual(project.on_run_end, ["{{ logging.log_run_end_event() }}"])

    def test_invalid_project_name(self):
        self.default_project_data["name"] = "invalid-project-name"
        with self.assertRaises(dbt.exceptions.DbtProjectError) as exc:
            project_from_config_norender(self.default_project_data, project_root=self.project_dir)

        self.assertIn("invalid-project-name", str(exc.exception))

    def test_no_project(self):
        os.remove(os.path.join(self.project_dir, "dbt_project.yml"))
        renderer = empty_project_renderer()
        with self.assertRaises(dbt.exceptions.DbtProjectError) as exc:
            dbt.config.Project.from_project_root(self.project_dir, renderer)

        self.assertIn("No dbt_project.yml", str(exc.exception))

    def test_invalid_version(self):
        self.default_project_data["require-dbt-version"] = "hello!"
        with self.assertRaises(dbt.exceptions.DbtProjectError):
            project_from_config_norender(self.default_project_data, project_root=self.project_dir)

    def test_unsupported_version(self):
        self.default_project_data["require-dbt-version"] = ">99999.0.0"
        # allowed, because the RuntimeConfig checks, not the Project itself
        project_from_config_norender(self.default_project_data, project_root=self.project_dir)

    def test_none_values(self):
        self.default_project_data.update(
            {
                "models": None,
                "seeds": None,
                "on-run-end": None,
                "on-run-start": None,
            }
        )
        project = project_from_config_rendered(self.default_project_data)
        self.assertEqual(project.models, {})
        self.assertEqual(project.on_run_start, [])
        self.assertEqual(project.on_run_end, [])
        self.assertEqual(project.seeds, {})

    def test_nested_none_values(self):
        self.default_project_data.update(
            {
                "models": {"vars": None, "pre-hook": None, "post-hook": None},
                "seeds": {"vars": None, "pre-hook": None, "post-hook": None, "column_types": None},
            }
        )
        project = project_from_config_rendered(self.default_project_data)
        self.assertEqual(project.models, {"vars": {}, "pre-hook": [], "post-hook": []})
        self.assertEqual(
            project.seeds, {"vars": {}, "pre-hook": [], "post-hook": [], "column_types": {}}
        )

    @pytest.mark.skipif(os.name == "nt", reason="crashes CI for Windows")
    def test_cycle(self):
        models = {}
        models["models"] = models
        self.default_project_data.update(
            {
                "models": models,
            }
        )
        with self.assertRaises(dbt.exceptions.DbtProjectError) as exc:
            project_from_config_rendered(self.default_project_data)

        assert "Cycle detected" in str(exc.exception)

    def test_query_comment_empty(self):
        self.default_project_data.update(
            {
                "query-comment": None,
            }
        )
        project = project_from_config_norender(
            self.default_project_data, project_root=self.project_dir
        )
        self.assertEqual(project.query_comment.comment, "")
        self.assertEqual(project.query_comment.append, QueryComment().append)

        self.default_project_data.update(
            {
                "query-comment": "",
            }
        )
        project = project_from_config_norender(
            self.default_project_data, project_root=self.project_dir
        )
        self.assertEqual(project.query_comment.comment, "")
        self.assertEqual(project.query_comment.append, QueryComment().append)

    def test_default_query_comment(self):
        project = project_from_config_norender(
            self.default_project_data, project_root=self.project_dir
        )
        self.assertEqual(project.query_comment, QueryComment())

    def test_default_query_comment_append(self):
        self.default_project_data.update(
            {
                "query-comment": {"append": True},
            }
        )
        project = project_from_config_norender(
            self.default_project_data, project_root=self.project_dir
        )
        self.assertEqual(project.query_comment.comment, DEFAULT_QUERY_COMMENT)
        self.assertEqual(project.query_comment.append, True)

    def test_custom_query_comment_append(self):
        self.default_project_data.update(
            {
                "query-comment": {"comment": "run by user test", "append": True},
            }
        )
        project = project_from_config_norender(
            self.default_project_data, project_root=self.project_dir
        )
        self.assertEqual(project.query_comment.comment, "run by user test")
        self.assertEqual(project.query_comment.append, True)

    def test_default_query_comment_append_False(self):
        self.default_project_data.update(
            {
                "query-comment": {"append": False},
            }
        )
        project = project_from_config_norender(
            self.default_project_data, project_root=self.project_dir
        )
        self.assertEqual(project.query_comment.comment, DEFAULT_QUERY_COMMENT)
        self.assertEqual(project.query_comment.append, False)

    def test_custom_query_comment_append_false(self):
        self.default_project_data.update(
            {
                "query-comment": {"comment": "run by user test", "append": False},
            }
        )
        project = project_from_config_norender(
            self.default_project_data, project_root=self.project_dir
        )
        self.assertEqual(project.query_comment.comment, "run by user test")
        self.assertEqual(project.query_comment.append, False)

    def test_packages_from_dependencies(self):
        packages = {
            "packages": [
                {
                    "git": "{{ env_var('some_package') }}",
                    "warn-unpinned": True,
                }
            ],
        }

        project = project_from_config_rendered(
            self.default_project_data, packages, packages_specified_path=DEPENDENCIES_FILE_NAME
        )
        git_package = project.packages.packages[0]
        # packages did not render because packages_specified_path=DEPENDENCIES_FILE_NAME
        assert git_package.git == "{{ env_var('some_package') }}"


class TestProjectFile(BaseConfigTest):
    def test_from_project_root(self):
        renderer = empty_project_renderer()
        project = dbt.config.Project.from_project_root(self.project_dir, renderer)
        from_config = project_from_config_norender(
            self.default_project_data, project_root=self.project_dir
        )
        self.assertEqual(project, from_config)
        self.assertEqual(project.version, "0.0.1")
        self.assertEqual(project.project_name, "my_test_project")

    def test_with_invalid_package(self):
        renderer = empty_project_renderer()
        self.write_packages({"invalid": ["not a package of any kind"]})
        with self.assertRaises(dbt.exceptions.DbtProjectError):
            dbt.config.Project.from_project_root(self.project_dir, renderer)


class TestVariableProjectFile(BaseConfigTest):
    def setUp(self):
        super().setUp()
        self.default_project_data["version"] = "{{ var('cli_version') }}"
        self.default_project_data["name"] = "blah"
        self.default_project_data["profile"] = "{{ env_var('env_value_profile') }}"
        self.write_project(self.default_project_data)

    def test_cli_and_env_vars(self):
        renderer = dbt.config.renderer.DbtProjectYamlRenderer(None, {"cli_version": "0.1.2"})
        with mock.patch.dict(os.environ, self.env_override):
            safe_set_invocation_context()  # reset invocation context with new env
            project = dbt.config.Project.from_project_root(
                self.project_dir,
                renderer,
            )

        self.assertEqual(renderer.ctx_obj.env_vars, {"env_value_profile": "default"})
        self.assertEqual(project.version, "0.1.2")
        self.assertEqual(project.project_name, "blah")
        self.assertEqual(project.profile_name, "default")


class TestVarLookups(unittest.TestCase):
    def setUp(self):
        self.initial_src_vars = {
            # globals
            "foo": 123,
            "bar": "hello",
            # project-scoped
            "my_project": {
                "bar": "goodbye",
                "baz": True,
            },
            "other_project": {
                "foo": 456,
            },
        }
        self.src_vars = deepcopy(self.initial_src_vars)
        self.dst = {"vars": deepcopy(self.initial_src_vars)}

        self.projects = ["my_project", "other_project", "third_project"]
        load_plugin("postgres")
        self.local_var_search = mock.MagicMock(
            fqn=["my_project", "my_model"], resource_type=NodeType.Model, package_name="my_project"
        )
        self.other_var_search = mock.MagicMock(
            fqn=["other_project", "model"],
            resource_type=NodeType.Model,
            package_name="other_project",
        )
        self.third_var_search = mock.MagicMock(
            fqn=["third_project", "third_model"],
            resource_type=NodeType.Model,
            package_name="third_project",
        )

    def test_lookups(self):
        vars_provider = dbt.config.project.VarProvider(self.initial_src_vars)

        expected = [
            (self.local_var_search, "foo", 123),
            (self.other_var_search, "foo", 456),
            (self.third_var_search, "foo", 123),
            (self.local_var_search, "bar", "goodbye"),
            (self.other_var_search, "bar", "hello"),
            (self.third_var_search, "bar", "hello"),
            (self.local_var_search, "baz", True),
            (self.other_var_search, "baz", None),
            (self.third_var_search, "baz", None),
        ]
        for node, key, expected_value in expected:
            value = vars_provider.vars_for(node, "postgres").get(key)
            assert value == expected_value


class TestMultipleProjectFlags(BaseConfigTest):
    def setUp(self):
        super().setUp()

        self.default_project_data.update(
            {
                "flags": {
                    "send_anonymous_usage_data": False,
                }
            }
        )
        self.write_project(self.default_project_data)

        self.default_profile_data.update(
            {
                "config": {
                    "send_anonymous_usage_data": False,
                }
            }
        )
        self.write_profile(self.default_profile_data)

    def test_setting_multiple_flags(self):
        with pytest.raises(dbt.exceptions.DbtProjectError):
            set_from_args(self.args, None)


class TestReadProjectFlags:
    @pytest.fixture(autouse=True)
    def buffered_deprecations(self):
        deprecations.buffered_deprecations.clear()
        yield deprecations.buffered_deprecations
        deprecations.buffered_deprecations.clear()

    def read_flags(self, tmp_path, project_flags):
        (tmp_path / "dbt_project.yml").write_text(
            "name: test\nversion: 1.0\nconfig-version: 2\nprofile: test\n" + project_flags
        )
        return read_project_flags(str(tmp_path), str(tmp_path))

    def test_valid_flags(self, tmp_path, buffered_deprecations):
        project_flags = self.read_flags(tmp_path, "flags:\n  fail_fast: true\n")

        assert project_flags.fail_fast is True
        assert buffered_deprecations == []

    def test_invalid_flag_is_logged_once_logger_is_ready(self, tmp_path, buffered_deprecations):
        project_flags = self.read_flags(
            tmp_path,
            "flags:\n  fail_fast: true\n  require_yaml_configuration_for_mf_time_spines: bad\n",
        )
        catcher = EventCatcher(event_to_catch=LogDbtProjectError)
        get_event_manager().add_callback(catcher.catch)

        deprecations.fire_buffered_deprecations()

        assert project_flags == ProjectFlags()
        assert len(catcher.caught_events) == 1
        msg = catcher.caught_events[0].info.msg
        assert (
            "Invalid project flag 'require_yaml_configuration_for_mf_time_spines': "
            "'bad' is not of type 'boolean'"
        ) in msg
        assert "All project flags were ignored." in msg

    @pytest.mark.parametrize(
        "project_flags",
        ["flags: [bad]\n", "flags: null\n", "flags:\n  warn_error_options: [bad]\n"],
    )
    def test_malformed_flags_do_not_crash(self, tmp_path, project_flags):
        assert self.read_flags(tmp_path, project_flags) == ProjectFlags()


class TestGetRequiredVersion:
    @pytest.fixture
    def project_dict(self) -> Dict[str, Any]:
        return {
            "name": "test_project",
            "require-dbt-version": ">0.0.0",
        }

    def test_supported_version(self, project_dict: Dict[str, Any]) -> None:
        specifiers = _get_required_version(project_dict=project_dict, verify_version=True)
        assert set(x.to_version_string() for x in specifiers) == {">0.0.0"}

    def test_unsupported_version(self, project_dict: Dict[str, Any]) -> None:
        project_dict["require-dbt-version"] = ">99999.0.0"
        with pytest.raises(
            dbt.exceptions.DbtProjectError, match="This version of dbt is not supported"
        ):
            _get_required_version(project_dict=project_dict, verify_version=True)

    def test_unsupported_version_no_check(self, project_dict: Dict[str, Any]) -> None:
        project_dict["require-dbt-version"] = ">99999.0.0"
        specifiers = _get_required_version(project_dict=project_dict, verify_version=False)
        assert set(x.to_version_string() for x in specifiers) == {">99999.0.0"}

    def test_supported_version_range(self, project_dict: Dict[str, Any]) -> None:
        project_dict["require-dbt-version"] = [">0.0.0", "<=99999.0.0"]
        specifiers = _get_required_version(project_dict=project_dict, verify_version=True)
        assert set(x.to_version_string() for x in specifiers) == {">0.0.0", "<=99999.0.0"}

    def test_unsupported_version_range(self, project_dict: Dict[str, Any]) -> None:
        project_dict["require-dbt-version"] = [">0.0.0", "<=0.0.1"]
        with pytest.raises(
            dbt.exceptions.DbtProjectError, match="This version of dbt is not supported"
        ):
            _get_required_version(project_dict=project_dict, verify_version=True)

    def test_unsupported_version_range_no_check(self, project_dict: Dict[str, Any]) -> None:
        project_dict["require-dbt-version"] = [">0.0.0", "<=0.0.1"]
        specifiers = _get_required_version(project_dict=project_dict, verify_version=False)
        assert set(x.to_version_string() for x in specifiers) == {">0.0.0", "<=0.0.1"}

    def test_impossible_version_range(self, project_dict: Dict[str, Any]) -> None:
        project_dict["require-dbt-version"] = [">99999.0.0", "<=0.0.1"]
        with pytest.raises(
            dbt.exceptions.DbtProjectError,
            match="The package version requirement can never be satisfied",
        ):
            _get_required_version(project_dict=project_dict, verify_version=True)


class TestDeprecations:

    @mock.patch("dbt.jsonschemas.jsonschemas._can_run_validations", return_value=True)
    def test_jsonschema_validate(self, mock_can_run) -> None:
        from dbt.jsonschemas.jsonschemas import jsonschema_validate

        project_dict: Dict[str, Any] = {}

        event_catcher = EventCatcher(GenericJSONSchemaValidationDeprecation)
        note_catcher = EventCatcher(Note)
        get_event_manager().add_callback(event_catcher.catch)
        get_event_manager().add_callback(note_catcher.catch)

        jsonschema_validate(
            schema=project_schema(), json=project_dict, file_path="dbt_project.yml"
        )

        if GenericJSONSchemaValidationDeprecationCore()._is_preview:
            assert len(note_catcher.caught_events) == 1
            assert len(event_catcher.caught_events) == 0
            event = note_catcher.caught_events[0]
        else:
            assert len(event_catcher.caught_events) == 1
            assert len(note_catcher.caught_events) == 0
            event = event_catcher.caught_events[0]

        assert "'name' is a required property at top level" in event.info.msg
