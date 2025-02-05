import importlib
import os
import re
from argparse import Namespace
from copy import deepcopy
from typing import Any, Dict, Mapping, Optional, Set
from unittest import mock

import pytest
import pytz

import dbt_common.exceptions
from dbt.adapters import factory, postgres
from dbt.clients.jinja import MacroStack
from dbt.config.project import VarProvider
from dbt.context import base, docs, macros, providers, query_header
from dbt.context.base import Var
from dbt.contracts.files import FileHash
from dbt.contracts.graph.nodes import (
    DependsOn,
    Macro,
    ModelNode,
    NodeConfig,
    UnitTestNode,
    UnitTestOverrides,
)
from dbt.node_types import NodeType
from dbt_common.events.functions import reset_metadata_vars
from dbt_common.helper_types import WarnErrorOptions
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
}
REQUIRED_MODEL_KEYS = REQUIRED_MACRO_KEYS | {"this", "compiled_code"}
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


def clean_value(value):
    if isinstance(value, set):
        return set(value)
    elif isinstance(value, Namespace):
        return value.__dict__
    elif isinstance(value, Var):
        return {k: v for k, v in value._merged.items()}
    elif isinstance(value, bool):
        return value
    elif value is None:
        return None
    elif isinstance(value, int):
        return value
    else:
        value_str = str(value)
        value_str = re.sub(r" at 0x[0-9a-fA-F]+>", ">", value_str)
        value_str = re.sub(r" id='[0-9]+'>", ">", value_str)
        return value_str


def walk_dict(dictionary):
    skip_paths = [
        ["invocation_id"],
        ["builtins", "invocation_id"],
        ["dbt_version"],
        ["builtins", "dbt_version"],
        ["invocation_args_dict", "invocation_command"],
        ["run_started_at"],
        ["builtins", "run_started_at"],
        ["selected_resources"],
        ["builtins", "selected_resources"],
    ]

    stack = [(dictionary, [])]
    visited = set()  # Set to keep track of visited dictionary objects

    while stack:
        current_dict, path = stack.pop(0)

        if id(current_dict) in visited:
            continue

        visited.add(id(current_dict))

        for key, value in current_dict.items():
            current_path = path + [key]

            if isinstance(value, Mapping):
                stack.append((value, current_path))
            else:
                if current_path not in skip_paths:
                    cv = clean_value(value)
                    if current_path == ["flags"]:
                        del cv["INVOCATION_COMMAND"]

                    yield (tuple(current_path), cv)


def add_prefix(path_dict, prefix):
    return {prefix + k: v for k, v in path_dict.items()}


def get_module_exports(module_name: str, filter_set: Optional[Set[str]] = None):
    module = importlib.import_module(module_name)
    export_names = filter_set or module.__all__

    return {
        ("modules", module_name, export): clean_value(getattr(module, export))
        for export in export_names
    }


PYTZ_COUNTRY_TIMEZONES = {
    ("modules", "pytz", "country_timezones", country_code): str(timezones)
    for country_code, timezones in pytz.country_timezones.items()
}

PYTZ_COUNTRY_NAMES = {
    ("modules", "pytz", "country_names", country_code): country_name
    for country_code, country_name in pytz.country_names.items()
}

COMMON_FLAGS_INVOCATION_ARGS = {
    "CACHE_SELECTED_ONLY": False,
    "LOG_FORMAT": "default",
    "LOG_PATH": "logs",
    "SEND_ANONYMOUS_USAGE_STATS": True,
    "INDIRECT_SELECTION": "eager",
    "INTROSPECT": True,
    "PARTIAL_PARSE": True,
    "PRINTER_WIDTH": 80,
    "QUIET": False,
    "STATIC_PARSER": True,
    "USE_COLORS": True,
    "VERSION_CHECK": True,
    "WRITE_JSON": True,
}

COMMON_FLAGS = {
    **COMMON_FLAGS_INVOCATION_ARGS,
    "LOG_CACHE_EVENTS": False,
    "FAIL_FAST": False,
    "DEBUG": False,
    "WARN_ERROR": None,
    "WARN_ERROR_OPTIONS": WarnErrorOptions(include=[], exclude=[]),
    "USE_EXPERIMENTAL_PARSER": False,
    "NO_PRINT": None,
    "PROFILES_DIR": None,
    "TARGET_PATH": None,
    "EMPTY": None,
    "FULL_REFRESH": False,
    "STORE_FAILURES": False,
    "WHICH": "run",
}


COMMON_BUILTINS = {
    ("diff_of_two_dicts",): "<function BaseContext.diff_of_two_dicts>",
    ("flags",): COMMON_FLAGS,
    ("fromjson",): "<function BaseContext.fromjson>",
    ("fromyaml",): "<function BaseContext.fromyaml>",
    ("local_md5",): "<function BaseContext.local_md5>",
    ("log",): "<function BaseContext.log>",
    ("print",): "<function BaseContext.print>",
    ("project_name",): "root",
    ("return",): "<function BaseContext._return>",
    ("set",): "<function BaseContext._set>",
    ("set_strict",): "<function BaseContext.set_strict>",
    ("thread_id",): "MainThread",
    ("tojson",): "<function BaseContext.tojson>",
    ("toyaml",): "<function BaseContext.toyaml>",
    ("var",): {},
    ("zip",): "<function BaseContext._zip>",
    ("zip_strict",): "<function BaseContext.zip_strict>",
}

COMMON_RUNTIME_CONTEXT = {
    **COMMON_BUILTINS,
    **add_prefix(COMMON_BUILTINS, ("builtins",)),
    ("target", "host"): "localhost",
    ("target", "port"): 1,
    ("target", "user"): "test",
    ("target", "database"): "test",
    ("target", "schema"): "analytics",
    ("target", "connect_timeout"): 10,
    ("target", "role"): None,
    ("target", "search_path"): None,
    ("target", "keepalives_idle"): 0,
    ("target", "sslmode"): None,
    ("target", "sslcert"): None,
    ("target", "sslkey"): None,
    ("target", "sslrootcert"): None,
    ("target", "application_name"): "dbt",
    ("target", "retries"): 1,
    ("target", "dbname"): "test",
    ("target", "type"): "postgres",
    ("target", "threads"): 1,
    ("target", "name"): "test",
    ("target", "target_name"): "test",
    ("target", "profile_name"): "test",
    **get_module_exports("datetime", {"date", "datetime", "time", "timedelta", "tzinfo"}),
    **get_module_exports("re"),
    **get_module_exports(
        "itertools",
        {
            "count",
            "cycle",
            "repeat",
            "accumulate",
            "chain",
            "compress",
            "islice",
            "starmap",
            "tee",
            "zip_longest",
            "product",
            "permutations",
            "combinations",
            "combinations_with_replacement",
        },
    ),
    ("modules", "pytz", "timezone"): "<function timezone>",
    ("modules", "pytz", "utc"): "UTC",
    ("modules", "pytz", "AmbiguousTimeError"): "<class 'pytz.exceptions.AmbiguousTimeError'>",
    ("modules", "pytz", "InvalidTimeError"): "<class 'pytz.exceptions.InvalidTimeError'>",
    ("modules", "pytz", "NonExistentTimeError"): "<class 'pytz.exceptions.NonExistentTimeError'>",
    ("modules", "pytz", "UnknownTimeZoneError"): "<class 'pytz.exceptions.UnknownTimeZoneError'>",
    ("modules", "pytz", "all_timezones"): str(pytz.all_timezones),
    ("modules", "pytz", "all_timezones_set"): set(pytz.all_timezones_set),
    ("modules", "pytz", "common_timezones"): str(pytz.common_timezones),
    ("modules", "pytz", "common_timezones_set"): set(),
    ("modules", "pytz", "BaseTzInfo"): "<class 'pytz.tzinfo.BaseTzInfo'>",
    ("modules", "pytz", "FixedOffset"): "<function FixedOffset>",
    **PYTZ_COUNTRY_TIMEZONES,
    **PYTZ_COUNTRY_NAMES,
}

MODEL_BUILTINS = {
    ("adapter",): "<dbt.context.providers.RuntimeDatabaseWrapper object>",
    (
        "adapter_macro",
    ): "<bound method ProviderContext.adapter_macro of <dbt.context.providers.ModelContext object>>",
    ("column",): "<MagicMock name='get_adapter().Column'>",
    ("compiled_code",): "<MagicMock name='model_one.compiled_code'>",
    ("config",): "<dbt.context.providers.RuntimeConfigObject object>",
    ("context_macro_stack",): "<dbt.clients.jinja.MacroStack object>",
    ("database",): "dbt",
    ("defer_relation",): "<MagicMock name='get_adapter().Relation.create_from()'>",
    (
        "env_var",
    ): "<bound method ProviderContext.env_var of <dbt.context.providers.ModelContext object>>",
    ("execute",): True,
    ("graph",): "<MagicMock name='mock.flat_graph'>",
    (
        "load_agate_table",
    ): "<bound method ProviderContext.load_agate_table of <dbt.context.providers.ModelContext object>>",
    (
        "load_result",
    ): "<bound method ProviderContext.load_result of <dbt.context.providers.ModelContext object>>",
    ("metric",): "<dbt.context.providers.RuntimeMetricResolver object>",
    ("model",): "<MagicMock name='model_one.to_dict()'>",
    ("post_hooks",): "[]",
    ("pre_hooks",): "[]",
    ("ref",): "<dbt.context.providers.RuntimeRefResolver object>",
    (
        "render",
    ): "<bound method ProviderContext.render of <dbt.context.providers.ModelContext object>>",
    ("schema",): "analytics",
    ("source",): "<dbt.context.providers.RuntimeSourceResolver object>",
    ("sql",): "<MagicMock name='model_one.compiled_code'>",
    ("sql_now",): "<MagicMock name='get_adapter().date_function()'>",
    (
        "store_raw_result",
    ): "<bound method ProviderContext.store_raw_result of <dbt.context.providers.ModelContext object>>",
    (
        "store_result",
    ): "<bound method ProviderContext.store_result of <dbt.context.providers.ModelContext object>>",
    (
        "submit_python_job",
    ): "<bound method ProviderContext.submit_python_job of <dbt.context.providers.ModelContext object>>",
    ("this",): "<MagicMock name='get_adapter().Relation.create_from()'>",
    (
        "try_or_compiler_error",
    ): "<bound method ProviderContext.try_or_compiler_error of <dbt.context.providers.ModelContext object>>",
    (
        "write",
    ): "<bound method ProviderContext.write of <dbt.context.providers.ModelContext object>>",
}

MODEL_RUNTIME_BUILTINS = {
    **MODEL_BUILTINS,
}

MODEL_EXCEPTIONS = {
    ("exceptions", "warn"): "<function warn>",
    ("exceptions", "missing_config"): "<function missing_config>",
    ("exceptions", "missing_materialization"): "<function missing_materialization>",
    ("exceptions", "missing_relation"): "<function missing_relation>",
    ("exceptions", "raise_ambiguous_alias"): "<function raise_ambiguous_alias>",
    ("exceptions", "raise_ambiguous_catalog_match"): "<function raise_ambiguous_catalog_match>",
    ("exceptions", "raise_cache_inconsistent"): "<function raise_cache_inconsistent>",
    ("exceptions", "raise_dataclass_not_dict"): "<function raise_dataclass_not_dict>",
    ("exceptions", "raise_compiler_error"): "<function raise_compiler_error>",
    ("exceptions", "raise_database_error"): "<function raise_database_error>",
    ("exceptions", "raise_dep_not_found"): "<function raise_dep_not_found>",
    ("exceptions", "raise_dependency_error"): "<function raise_dependency_error>",
    ("exceptions", "raise_duplicate_patch_name"): "<function raise_duplicate_patch_name>",
    ("exceptions", "raise_duplicate_resource_name"): "<function raise_duplicate_resource_name>",
    (
        "exceptions",
        "raise_invalid_property_yml_version",
    ): "<function raise_invalid_property_yml_version>",
    ("exceptions", "raise_not_implemented"): "<function raise_not_implemented>",
    ("exceptions", "relation_wrong_type"): "<function relation_wrong_type>",
    ("exceptions", "raise_contract_error"): "<function raise_contract_error>",
    ("exceptions", "column_type_missing"): "<function column_type_missing>",
    ("exceptions", "raise_fail_fast_error"): "<function raise_fail_fast_error>",
    (
        "exceptions",
        "warn_snapshot_timestamp_data_types",
    ): "<function warn_snapshot_timestamp_data_types>",
}

MODEL_MACROS = {
    ("macro_a",): "<dbt.clients.jinja.MacroGenerator object>",
    ("macro_b",): "<dbt.clients.jinja.MacroGenerator object>",
}

EXPECTED_MODEL_RUNTIME_CONTEXT = deepcopy(
    {
        **COMMON_RUNTIME_CONTEXT,
        **MODEL_RUNTIME_BUILTINS,
        **add_prefix(MODEL_RUNTIME_BUILTINS, ("builtins",)),
        **MODEL_MACROS,
        **add_prefix(MODEL_MACROS, ("root",)),
        **add_prefix(
            {(k.lower(),): v for k, v in COMMON_FLAGS_INVOCATION_ARGS.items()},
            ("invocation_args_dict",),
        ),
        ("invocation_args_dict", "profile_dir"): "/dev/null",
        ("invocation_args_dict", "warn_error_options", "include"): "[]",
        ("invocation_args_dict", "warn_error_options", "exclude"): "[]",
        **MODEL_EXCEPTIONS,
        ("api", "Column"): "<MagicMock name='get_adapter().Column'>",
        ("api", "Relation"): "<dbt.context.providers.RelationProxy object>",
        ("validation", "any"): "<function ProviderContext.validation.<locals>.validate_any>",
    }
)

EXPECTED_MODEL_RUNTIME_CONTEXT = deepcopy(
    {
        **COMMON_RUNTIME_CONTEXT,
        **MODEL_RUNTIME_BUILTINS,
        **add_prefix(MODEL_RUNTIME_BUILTINS, ("builtins",)),
        **MODEL_MACROS,
        **add_prefix(MODEL_MACROS, ("root",)),
        **add_prefix(
            {(k.lower(),): v for k, v in COMMON_FLAGS_INVOCATION_ARGS.items()},
            ("invocation_args_dict",),
        ),
        ("invocation_args_dict", "profile_dir"): "/dev/null",
        ("invocation_args_dict", "warn_error_options", "include"): "[]",
        ("invocation_args_dict", "warn_error_options", "exclude"): "[]",
        **MODEL_EXCEPTIONS,
        ("api", "Column"): "<MagicMock name='get_adapter().Column'>",
        ("api", "Relation"): "<dbt.context.providers.RelationProxy object>",
        ("validation", "any"): "<function ProviderContext.validation.<locals>.validate_any>",
    }
)

DOCS_BUILTINS = {
    ("doc",): "<bound method DocsRuntimeContext.doc of "
    "<dbt.context.docs.DocsRuntimeContext object>>",
    ("env_var",): "<bound method SchemaYamlContext.env_var of "
    "<dbt.context.docs.DocsRuntimeContext object>>",
}

EXPECTED_DOCS_RUNTIME_CONTEXT = deepcopy(
    {
        **COMMON_RUNTIME_CONTEXT,
        **DOCS_BUILTINS,
        **add_prefix(DOCS_BUILTINS, ("builtins",)),
    }
)


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
    assert ctx["invocation_args_dict"]["warn_error_options"] == {"include": [], "exclude": []}


def test_model_parse_context(config_postgres, manifest_fx, get_adapter, get_include_paths):
    ctx = providers.generate_parser_model_context(
        model=mock_model(),
        config=config_postgres,
        manifest=manifest_fx,
        context_config=mock.MagicMock(),
    )
    actual_model_context = {k: v for (k, v) in walk_dict(ctx)}
    assert actual_model_context == EXPECTED_MODEL_RUNTIME_CONTEXT


def test_model_runtime_context(config_postgres, manifest_fx, get_adapter, get_include_paths):
    ctx = providers.generate_runtime_model_context(
        model=mock_model(),
        config=config_postgres,
        manifest=manifest_fx,
    )
    actual_model_context = {k: v for (k, v) in walk_dict(ctx)}
    assert actual_model_context == EXPECTED_MODEL_RUNTIME_CONTEXT


def test_docs_runtime_context(config_postgres):
    ctx = docs.generate_runtime_docs_context(config_postgres, mock_model(), [], "root")
    actual_docs_runtime_context = {k: v for (k, v) in walk_dict(ctx)}
    assert actual_docs_runtime_context == EXPECTED_DOCS_RUNTIME_CONTEXT


def test_macro_namespace_duplicates(config_postgres, manifest_fx):
    mn = macros.MacroNamespaceBuilder("root", "search", MacroStack(), ["dbt_postgres", "dbt"])
    mn.add_macros(manifest_fx.macros.values(), {})

    # same pkg, same name: error
    with pytest.raises(dbt_common.exceptions.CompilationError):
        mn.add_macro(mock_macro("macro_a", "root"), {})

    # different pkg, same name: no error
    mn.add_macros(mock_macro("macro_a", "dbt"), {})


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
