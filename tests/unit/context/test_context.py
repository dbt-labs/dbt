import os
from typing import Any, Dict, Set
from unittest import mock

import pytest

import dbt_common.exceptions
from dbt.adapters import factory, postgres
from dbt.clients.jinja import MacroStack
from dbt.config.project import VarProvider
from dbt.context import base, docs, macros, providers, query_header
from dbt.contracts.files import FileHash
from dbt.contracts.graph.nodes import (
    DependsOn,
    Macro,
    ModelNode,
    NodeConfig,
    UnitTestNode,
    UnitTestOverrides,
)
from dbt.exceptions import PackageNotFoundForMacroError
from dbt.node_types import NodeType
from dbt_common.events.functions import reset_metadata_vars
from tests.unit.mock_adapter import adapter_factory
from tests.unit.utils import clear_plugin, config_from_parts_or_dicts, inject_adapter


class TestVar:
    @pytest.fixture
    def model(self):
        return ModelNode(
            alias="model_one",
            name="model_one",
            database="dbt",
            schema="analytics",
            resource_type=NodeType.Model,
            unique_id="model.root.model_one",
            fqn=["root", "model_one"],
            package_name="root",
            original_file_path="model_one.sql",
            refs=[],
            sources=[],
            depends_on=DependsOn(),
            config=NodeConfig.from_dict(
                {
                    "enabled": True,
                    "materialized": "view",
                    "persist_docs": {},
                    "post-hook": [],
                    "pre-hook": [],
                    "vars": {},
                    "quoting": {},
                    "column_types": {},
                    "tags": [],
                }
            ),
            tags=[],
            path="model_one.sql",
            language="sql",
            raw_code="",
            description="",
            columns={},
            checksum=FileHash.from_contents(""),
        )

    @pytest.fixture
    def context(self):
        return mock.MagicMock()

    @pytest.fixture
    def provider(self):
        return VarProvider({})

    @pytest.fixture
    def config(self, provider):
        return mock.MagicMock(config_version=2, vars=provider, cli_vars={}, project_name="root")

    def test_var_default_something(self, model, config, context):
        config.cli_vars = {"foo": "baz"}
        var = providers.RuntimeVar(context, config, model)

        assert var("foo") == "baz"
        assert var("foo", "bar") == "baz"

    def test_var_default_none(self, model, config, context):
        config.cli_vars = {"foo": None}
        var = providers.RuntimeVar(context, config, model)

        assert var("foo") is None
        assert var("foo", "bar") is None

    def test_var_not_defined(self, model, config, context):
        var = providers.RuntimeVar(self.context, config, model)

        assert var("foo", "bar") == "bar"
        with pytest.raises(dbt_common.exceptions.CompilationError):
            var("foo")

    def test_parser_var_default_something(self, model, config, context):
        config.cli_vars = {"foo": "baz"}
        var = providers.ParseVar(context, config, model)
        assert var("foo") == "baz"
        assert var("foo", "bar") == "baz"

    def test_parser_var_default_none(self, model, config, context):
        config.cli_vars = {"foo": None}
        var = providers.ParseVar(context, config, model)
        assert var("foo") is None
        assert var("foo", "bar") is None

    def test_parser_var_not_defined(self, model, config, context):
        # at parse-time, we should not raise if we encounter a missing var
        # that way disabled models don't get parse errors
        var = providers.ParseVar(context, config, model)

        assert var("foo", "bar") == "bar"
        assert var("foo") is None


class TestParseWrapper:
    @pytest.fixture
    def mock_adapter(self):
        mock_config = mock.MagicMock()
        mock_mp_context = mock.MagicMock()
        adapter_class = adapter_factory()
        return adapter_class(mock_config, mock_mp_context)

    @pytest.fixture
    def wrapper(self, mock_adapter):
        namespace = mock.MagicMock()
        return providers.ParseDatabaseWrapper(mock_adapter, namespace)

    @pytest.fixture
    def responder(self, mock_adapter):
        return mock_adapter.responder

    def test_unwrapped_method(self, wrapper, responder):
        assert wrapper.quote("test_value") == '"test_value"'
        responder.quote.assert_called_once_with("test_value")

    def test_wrapped_method(self, wrapper, responder):
        found = wrapper.get_relation("database", "schema", "identifier")
        assert found is None
        responder.get_relation.assert_not_called()


class TestRuntimeWrapper:
    @pytest.fixture
    def mock_adapter(self):
        mock_config = mock.MagicMock()
        mock_config.quoting = {
            "database": True,
            "schema": True,
            "identifier": True,
        }
        mock_mp_context = mock.MagicMock()
        adapter_class = adapter_factory()
        return adapter_class(mock_config, mock_mp_context)

    @pytest.fixture
    def wrapper(self, mock_adapter):
        namespace = mock.MagicMock()
        return providers.RuntimeDatabaseWrapper(mock_adapter, namespace)

    @pytest.fixture
    def responder(self, mock_adapter):
        return mock_adapter.responder

    def test_unwrapped_method(self, wrapper, responder):
        # the 'quote' method isn't wrapped, we should get our expected inputs
        assert wrapper.quote("test_value") == '"test_value"'
        responder.quote.assert_called_once_with("test_value")


def assert_has_keys(required_keys: Set[str], maybe_keys: Set[str], ctx: Dict[str, Any]):
    keys = set(ctx)
    for key in required_keys:
        assert key in keys, f"{key} in required keys but not in context"
        keys.remove(key)
    extras = keys.difference(maybe_keys)
    assert not extras, f"got extra keys in context: {extras}"


REQUIRED_BASE_KEYS = frozenset(
    {
        "context",
        "builtins",
        "dbt_version",
        "var",
        "env_var",
        "return",
        "fromjson",
        "tojson",
        "fromyaml",
        "toyaml",
        "set",
        "set_strict",
        "zip",
        "zip_strict",
        "log",
        "run_started_at",
        "invocation_id",
        "thread_id",
        "otel_trace_id",
        "otel_span_id",
        "modules",
        "flags",
        "print",
        "diff_of_two_dicts",
        "local_md5",
    }
)

REQUIRED_TARGET_KEYS = REQUIRED_BASE_KEYS | {"target"}
REQUIRED_DOCS_KEYS = REQUIRED_TARGET_KEYS | {"project_name"} | {"doc"}
MACROS = frozenset({"macro_a", "macro_b", "root", "dbt"})
REQUIRED_QUERY_HEADER_KEYS = (
    REQUIRED_TARGET_KEYS | {"project_name", "context_macro_stack"} | MACROS
)
REQUIRED_MACRO_KEYS = REQUIRED_QUERY_HEADER_KEYS | {
    "_sql_results",
    "load_result",
    "store_result",
    "store_raw_result",
    "validation",
    "write",
    "render",
    "try_or_compiler_error",
    "load_agate_table",
    "ref",
    "source",
    "metric",
    "config",
    "execute",
    "exceptions",
    "database",
    "schema",
    "adapter",
    "api",
    "column",
    "env",
    "graph",
    "model",
    "pre_hooks",
    "post_hooks",
    "sql",
    "sql_now",
    "adapter_macro",
    "selected_resources",
    "invocation_args_dict",
    "submit_python_job",
    "dbt_metadata_envs",
    "function",
}
REQUIRED_MODEL_KEYS = REQUIRED_MACRO_KEYS | {
    "this",
    "compiled_code",
    "sql_full_refresh",
    "compiled_code_full_refresh",
}
MAYBE_KEYS = frozenset({"debug", "defer_relation"})


POSTGRES_PROFILE_DATA = {
    "target": "test",
    "quoting": {},
    "outputs": {
        "test": {
            "type": "postgres",
            "host": "localhost",
            "schema": "analytics",
            "user": "test",
            "pass": "test",
            "dbname": "test",
            "port": 1,
        }
    },
}

PROJECT_DATA = {
    "name": "root",
    "version": "0.1",
    "profile": "test",
    "project-root": os.getcwd(),
    "config-version": 2,
}


def model():
    return ModelNode(
        alias="model_one",
        name="model_one",
        database="dbt",
        schema="analytics",
        resource_type=NodeType.Model,
        unique_id="model.root.model_one",
        fqn=["root", "model_one"],
        package_name="root",
        original_file_path="model_one.sql",
        refs=[],
        sources=[],
        depends_on=DependsOn(),
        config=NodeConfig.from_dict(
            {
                "enabled": True,
                "materialized": "view",
                "persist_docs": {},
                "post-hook": [],
                "pre-hook": [],
                "vars": {},
                "quoting": {},
                "column_types": {},
                "tags": [],
            }
        ),
        tags=[],
        path="model_one.sql",
        language="sql",
        raw_code="",
        description="",
        columns={},
    )


def test_base_context():
    ctx = base.generate_base_context({})
    assert_has_keys(REQUIRED_BASE_KEYS, MAYBE_KEYS, ctx)


def mock_macro(name, package_name):
    macro = mock.MagicMock(
        __class__=Macro,
        package_name=package_name,
        resource_type="macro",
        unique_id=f"macro.{package_name}.{name}",
    )
    # Mock(name=...) does not set the `name` attribute, this does.
    macro.name = name
    return macro


def mock_manifest(config, additional_macros=None):
    default_macro_names = ["macro_a", "macro_b"]
    default_macros = [mock_macro(name, config.project_name) for name in default_macro_names]
    additional_macros = additional_macros or []
    all_macros = default_macros + additional_macros

    manifest_macros = {}
    macros_by_package = {}
    for macro in all_macros:
        manifest_macros[macro.unique_id] = macro
        if macro.package_name not in macros_by_package:
            macros_by_package[macro.package_name] = {}
        macro_package = macros_by_package[macro.package_name]
        macro_package[macro.name] = macro

    def gmbp():
        return macros_by_package

    m = mock.MagicMock(macros=manifest_macros)
    m.get_macros_by_package = gmbp
    return m


def mock_model():
    return mock.MagicMock(
        __class__=ModelNode,
        alias="model_one",
        name="model_one",
        database="dbt",
        schema="analytics",
        resource_type=NodeType.Model,
        unique_id="model.root.model_one",
        fqn=["root", "model_one"],
        package_name="root",
        original_file_path="model_one.sql",
        refs=[],
        sources=[],
        depends_on=DependsOn(),
        config=NodeConfig.from_dict(
            {
                "enabled": True,
                "materialized": "view",
                "persist_docs": {},
                "post-hook": [],
                "pre-hook": [],
                "vars": {},
                "quoting": {},
                "column_types": {},
                "tags": [],
            }
        ),
        tags=[],
        path="model_one.sql",
        language="sql",
        raw_code="",
        description="",
        columns={},
    )


def mock_unit_test_node():
    return mock.MagicMock(
        __class__=UnitTestNode,
        resource_type=NodeType.Unit,
        tested_node_unique_id="model.root.model_one",
    )


@pytest.fixture
def get_adapter():
    with mock.patch.object(providers, "get_adapter") as patch:
        yield patch


@pytest.fixture
def get_include_paths():
    with mock.patch.object(factory, "get_include_paths") as patch:
        patch.return_value = []
        yield patch


@pytest.fixture
def config_postgres():
    return config_from_parts_or_dicts(PROJECT_DATA, POSTGRES_PROFILE_DATA)


@pytest.fixture
def manifest_fx(config_postgres):
    return mock_manifest(config_postgres)


@pytest.fixture
def postgres_adapter(config_postgres, get_adapter):
    adapter = postgres.PostgresAdapter(config_postgres)
    inject_adapter(adapter, postgres.Plugin)
    get_adapter.return_value = adapter
    yield adapter
    clear_plugin(postgres.Plugin)


def test_query_header_context(config_postgres, manifest_fx):
    ctx = query_header.generate_query_header_context(
        config=config_postgres,
        manifest=manifest_fx,
    )
    assert_has_keys(REQUIRED_QUERY_HEADER_KEYS, MAYBE_KEYS, ctx)


def test_macro_runtime_context(config_postgres, manifest_fx, get_adapter, get_include_paths):
    ctx = providers.generate_runtime_macro_context(
        macro=manifest_fx.macros["macro.root.macro_a"],
        config=config_postgres,
        manifest=manifest_fx,
        package_name="root",
    )
    assert_has_keys(REQUIRED_MACRO_KEYS, MAYBE_KEYS, ctx)


def test_invocation_args_to_dict_in_macro_runtime_context(
    config_postgres, manifest_fx, get_adapter, get_include_paths
):
    ctx = providers.generate_runtime_macro_context(
        macro=manifest_fx.macros["macro.root.macro_a"],
        config=config_postgres,
        manifest=manifest_fx,
        package_name="root",
    )

    # Comes from dbt/flags.py as they are the only values set that aren't None at default
    assert ctx["invocation_args_dict"]["printer_width"] == 80

    # Comes from unit/utils.py config_from_parts_or_dicts method
    assert ctx["invocation_args_dict"]["profile_dir"] == "/dev/null"

    assert isinstance(ctx["invocation_args_dict"]["warn_error_options"], Dict)
    assert ctx["invocation_args_dict"]["warn_error_options"] == {
        "error": [],
        "warn": [],
        "silence": [],
    }


def test_model_parse_context(config_postgres, manifest_fx, get_adapter, get_include_paths):
    ctx = providers.generate_parser_model_context(
        model=mock_model(),
        config=config_postgres,
        manifest=manifest_fx,
        context_config=mock.MagicMock(),
    )
    assert_has_keys(REQUIRED_MODEL_KEYS, MAYBE_KEYS, ctx)


def test_model_runtime_context(config_postgres, manifest_fx, get_adapter, get_include_paths):
    ctx = providers.generate_runtime_model_context(
        model=mock_model(),
        config=config_postgres,
        manifest=manifest_fx,
    )
    assert_has_keys(REQUIRED_MODEL_KEYS, MAYBE_KEYS, ctx)


def test_docs_runtime_context(config_postgres):
    ctx = docs.generate_runtime_docs_context(config_postgres, mock_model(), [], "root")
    assert_has_keys(REQUIRED_DOCS_KEYS, MAYBE_KEYS, ctx)


def test_macro_namespace(config_postgres, manifest_fx):
    mn = macros.MacroNamespaceBuilder("root", "search", MacroStack(), ["dbt_postgres", "dbt"])

    mbp = manifest_fx.get_macros_by_package()
    dbt_macro = mock_macro("some_macro", "dbt")
    mbp["dbt"] = {"some_macro": dbt_macro}

    # same namespace, same name, different pkg!
    pg_macro = mock_macro("some_macro", "dbt_postgres")
    mbp["dbt_postgres"] = {"some_macro": pg_macro}

    # same name, different package
    package_macro = mock_macro("some_macro", "root")
    mbp["root"]["some_macro"] = package_macro

    namespace = mn.build_namespace(mbp, {})
    dct = dict(namespace)
    for result in [dct, namespace]:
        assert "dbt" in result
        assert "root" in result
        assert "some_macro" in result
        assert "dbt_postgres" not in result
        # tests __len__
        assert len(result) == 5
        # tests __iter__
        assert set(result) == {"dbt", "root", "some_macro", "macro_a", "macro_b"}
        assert len(result["dbt"]) == 1
        # from the regular manifest + some_macro
        assert len(result["root"]) == 3
        assert result["dbt"]["some_macro"].macro is pg_macro
        assert result["root"]["some_macro"].macro is package_macro
        assert result["some_macro"].macro is package_macro


def test_macro_namespace_package_name_collides_with_macro(config_postgres, manifest_fx):
    # A dbt project can legitimately share its name with a macro. Package
    # resolution must not return the colliding macro (a MacroGenerator), which
    # would raise `AttributeError: 'MacroGenerator' object has no attribute 'get'`.
    # Regression test for the macro-namespace collision surfaced by the
    # get_catalog_for_single_relation test (project named after the macro).
    mn = macros.MacroNamespaceBuilder("shared_name", "dbt", MacroStack(), ["dbt_postgres", "dbt"])

    mbp = manifest_fx.get_macros_by_package()
    # a global macro whose name collides with a project (package) name, plus a
    # macro that is *only* a macro (no package of the same name)
    mbp["dbt"] = {
        "shared_name": mock_macro("shared_name", "dbt"),
        "only_a_macro": mock_macro("only_a_macro", "dbt"),
    }
    # a project named "shared_name" that provides a macro
    project_macro = mock_macro("some_macro", "shared_name")
    mbp["shared_name"] = {"some_macro": project_macro}

    namespace = mn.build_namespace(mbp, {})

    # resolving the package must return the project's macro, not blow up on the
    # colliding global macro of the same name
    resolved = namespace.get_from_package("shared_name", "some_macro")
    assert resolved is not None
    assert resolved.macro is project_macro

    # a name that is only a macro (no such package) raises cleanly rather than
    # dereferencing a MacroGenerator
    with pytest.raises(PackageNotFoundForMacroError):
        namespace.get_from_package("only_a_macro", "some_macro")


def test_dbt_metadata_envs(
    monkeypatch, config_postgres, manifest_fx, get_adapter, get_include_paths
):
    reset_metadata_vars()

    envs = {
        "DBT_ENV_CUSTOM_ENV_RUN_ID": 1234,
        "DBT_ENV_CUSTOM_ENV_JOB_ID": 5678,
        "DBT_ENV_RUN_ID": 91011,
        "RANDOM_ENV": 121314,
    }
    monkeypatch.setattr(os, "environ", envs)

    ctx = providers.generate_runtime_macro_context(
        macro=manifest_fx.macros["macro.root.macro_a"],
        config=config_postgres,
        manifest=manifest_fx,
        package_name="root",
    )

    assert ctx["dbt_metadata_envs"] == {"JOB_ID": 5678, "RUN_ID": 1234}

    # cleanup
    reset_metadata_vars()


def test_unit_test_runtime_context(config_postgres, manifest_fx, get_adapter, get_include_paths):
    ctx = providers.generate_runtime_unit_test_context(
        unit_test=mock_unit_test_node(),
        config=config_postgres,
        manifest=manifest_fx,
    )
    assert_has_keys(REQUIRED_MODEL_KEYS, MAYBE_KEYS, ctx)


def test_unit_test_runtime_context_macro_overrides_global(
    config_postgres, manifest_fx, get_adapter, get_include_paths
):
    unit_test = mock_unit_test_node()
    unit_test.overrides = UnitTestOverrides(macros={"macro_a": "override"})
    ctx = providers.generate_runtime_unit_test_context(
        unit_test=unit_test,
        config=config_postgres,
        manifest=manifest_fx,
    )
    assert ctx["macro_a"]() == "override"


def test_unit_test_runtime_context_macro_overrides_package(
    config_postgres, manifest_fx, get_adapter, get_include_paths
):
    unit_test = mock_unit_test_node()
    unit_test.overrides = UnitTestOverrides(macros={"some_package.some_macro": "override"})

    dbt_macro = mock_macro("some_macro", "some_package")
    manifest_with_dbt_macro = mock_manifest(config_postgres, additional_macros=[dbt_macro])

    ctx = providers.generate_runtime_unit_test_context(
        unit_test=unit_test,
        config=config_postgres,
        manifest=manifest_with_dbt_macro,
    )
    assert ctx["some_package"]["some_macro"]() == "override"


@pytest.mark.parametrize(
    "overrides,expected_override_value",
    [
        # override dbt macro at global level
        ({"some_macro": "override"}, "override"),
        # # override dbt macro at dbt-namespaced level level
        ({"dbt.some_macro": "override"}, "override"),
        # override dbt macro at both levels - global override should win
        (
            {"some_macro": "dbt_global_override", "dbt.some_macro": "dbt_namespaced_override"},
            "dbt_global_override",
        ),
        # override dbt macro at both levels - global override should win, regardless of order
        (
            {"dbt.some_macro": "dbt_namespaced_override", "some_macro": "dbt_global_override"},
            "dbt_global_override",
        ),
    ],
)
def test_unit_test_runtime_context_macro_overrides_dbt_macro(
    overrides,
    expected_override_value,
    config_postgres,
    manifest_fx,
    get_adapter,
    get_include_paths,
):
    unit_test = mock_unit_test_node()
    unit_test.overrides = UnitTestOverrides(macros=overrides)

    dbt_macro = mock_macro("some_macro", "dbt")
    manifest_with_dbt_macro = mock_manifest(config_postgres, additional_macros=[dbt_macro])

    ctx = providers.generate_runtime_unit_test_context(
        unit_test=unit_test,
        config=config_postgres,
        manifest=manifest_with_dbt_macro,
    )
    assert ctx["some_macro"]() == expected_override_value
    assert ctx["dbt"]["some_macro"]() == expected_override_value
