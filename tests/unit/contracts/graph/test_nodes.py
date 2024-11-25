import pickle
import re
from argparse import Namespace
from dataclasses import replace

import pytest

from dbt.artifacts.resources import ColumnInfo, TestConfig, TestMetadata
from dbt.compilation import inject_ctes_into_sql
from dbt.contracts.files import FileHash
from dbt.contracts.graph.nodes import (
    DependsOn,
    GenericTestNode,
    InjectedCTE,
    ModelConfig,
    ModelNode,
)
from dbt.node_types import NodeType
from tests.unit.fixtures import generic_test_node, model_node
from tests.unit.utils import (
    assert_fails_validation,
    assert_from_dict,
    assert_symmetric,
    replace_config,
)


@pytest.fixture
def args_for_flags() -> Namespace:
    return Namespace(state_modified_compare_vars=False)


def norm_whitespace(string):
    _RE_COMBINE_WHITESPACE = re.compile(r"\s+")
    string = _RE_COMBINE_WHITESPACE.sub(" ", string).strip()
    return string


@pytest.fixture
def basic_uncompiled_model():
    return ModelNode(
        package_name="test",
        path="/root/models/foo.sql",
        original_file_path="models/foo.sql",
        language="sql",
        raw_code='select * from {{ ref("other") }}',
        name="foo",
        resource_type=NodeType.Model,
        unique_id="model.test.foo",
        fqn=["test", "models", "foo"],
        refs=[],
        sources=[],
        metrics=[],
        depends_on=DependsOn(),
        description="",
        database="test_db",
        schema="test_schema",
        alias="bar",
        tags=[],
        config=ModelConfig(),
        meta={},
        compiled=False,
        extra_ctes=[],
        extra_ctes_injected=False,
        checksum=FileHash.from_contents(""),
        unrendered_config={},
    )


@pytest.fixture
def basic_compiled_model():
    return model_node()


@pytest.fixture
def minimal_uncompiled_dict():
    return {
        "name": "foo",
        "created_at": 1,
        "resource_type": str(NodeType.Model),
        "path": "/root/models/foo.sql",
        "original_file_path": "models/foo.sql",
        "package_name": "test",
        "language": "sql",
        "raw_code": 'select * from {{ ref("other") }}',
        "unique_id": "model.test.foo",
        "fqn": ["test", "models", "foo"],
        "database": "test_db",
        "schema": "test_schema",
        "alias": "bar",
        "compiled": False,
        "checksum": {
            "name": "sha256",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
        "unrendered_config": {},
    }


@pytest.fixture
def basic_uncompiled_dict():
    return {
        "name": "foo",
        "created_at": 1,
        "resource_type": str(NodeType.Model),
        "path": "/root/models/foo.sql",
        "original_file_path": "models/foo.sql",
        "package_name": "test",
        "language": "sql",
        "raw_code": 'select * from {{ ref("other") }}',
        "unique_id": "model.test.foo",
        "fqn": ["test", "models", "foo"],
        "refs": [],
        "sources": [],
        "metrics": [],
        "depends_on": {"macros": [], "nodes": []},
        "database": "test_db",
        "description": "",
        "schema": "test_schema",
        "alias": "bar",
        "tags": [],
        "config": {
            "column_types": {},
            "enabled": True,
            "materialized": "view",
            "persist_docs": {},
            "post-hook": [],
            "pre-hook": [],
            "quoting": {},
            "tags": [],
            "on_schema_change": "ignore",
            "on_configuration_change": "apply",
            "meta": {},
            "grants": {},
            "packages": [],
        },
        "docs": {"show": True},
        "columns": {},
        "meta": {},
        "compiled": False,
        "extra_ctes": [],
        "extra_ctes_injected": False,
        "checksum": {
            "name": "sha256",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
        "unrendered_config": {},
        "unrendered_config_call_dict": {},
        "config_call_dict": {},
    }


@pytest.fixture
def basic_compiled_dict():
    return {
        "name": "foo",
        "created_at": 1,
        "resource_type": str(NodeType.Model),
        "path": "/root/models/foo.sql",
        "original_file_path": "models/foo.sql",
        "package_name": "test",
        "language": "sql",
        "raw_code": 'select * from {{ ref("other") }}',
        "unique_id": "model.test.foo",
        "fqn": ["test", "models", "foo"],
        "refs": [],
        "sources": [],
        "metrics": [],
        "depends_on": {"macros": [], "nodes": []},
        "database": "test_db",
        "description": "",
        "primary_key": [],
        "schema": "test_schema",
        "alias": "bar",
        "tags": [],
        "config": {
            "column_types": {},
            "enabled": True,
            "materialized": "view",
            "persist_docs": {},
            "post-hook": [],
            "pre-hook": [],
            "quoting": {},
            "tags": [],
            "on_schema_change": "ignore",
            "on_configuration_change": "apply",
            "meta": {},
            "grants": {},
            "packages": [],
            "contract": {"enforced": False, "alias_types": True},
            "docs": {"show": True},
            "access": "protected",
            "lookback": 1,
        },
        "docs": {"show": True},
        "columns": {},
        "contract": {"enforced": False, "alias_types": True},
        "meta": {},
        "compiled": True,
        "extra_ctes": [{"id": "whatever", "sql": "select * from other"}],
        "extra_ctes_injected": True,
        "compiled_code": "with whatever as (select * from other) select * from whatever",
        "checksum": {
            "name": "sha256",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
        "unrendered_config": {},
        "unrendered_config_call_dict": {},
        "vars": {},
        "config_call_dict": {},
        "access": "protected",
        "constraints": [],
    }


@pytest.mark.skip("Haven't found where we would use uncompiled node")
def test_basic_uncompiled_model(
    minimal_uncompiled_dict, basic_uncompiled_dict, basic_uncompiled_model
):
    node_dict = basic_uncompiled_dict
    node = basic_uncompiled_model
    assert_symmetric(node, node_dict, ModelNode)
    assert node.empty is False
    assert node.is_refable is True
    assert node.is_ephemeral is False

    assert_from_dict(node, minimal_uncompiled_dict, ModelNode)
    pickle.loads(pickle.dumps(node))


def test_basic_compiled_model(basic_compiled_dict, basic_compiled_model):
    node_dict = basic_compiled_dict
    node = basic_compiled_model
    assert_symmetric(node, node_dict, ModelNode)
    assert node.empty is False
    assert node.is_refable is True
    assert node.is_ephemeral is False


def test_invalid_extra_fields_model(minimal_uncompiled_dict):
    bad_extra = minimal_uncompiled_dict
    bad_extra["notvalid"] = "nope"
    assert_fails_validation(bad_extra, ModelNode)


def test_invalid_bad_type_model(minimal_uncompiled_dict):
    bad_type = minimal_uncompiled_dict
    bad_type["resource_type"] = str(NodeType.Macro)
    assert_fails_validation(bad_type, ModelNode)


unchanged_compiled_models = [
    lambda u: (u, replace(u, description="a description")),
    lambda u: (u, replace(u, tags=["mytag"])),
    lambda u: (u, replace(u, meta={"cool_key": "cool value"})),
    # changing the final alias/schema/datbase isn't a change - could just be target changing!
    lambda u: (u, replace(u, database="nope")),
    lambda u: (u, replace(u, schema="nope")),
    lambda u: (u, replace(u, alias="nope")),
    # None -> False is a config change even though it's pretty much the same
    lambda u: (
        replace(u, config=replace(u.config, persist_docs={"relation": False})),
        replace(u, config=replace(u.config, persist_docs={"relation": False})),
    ),
    lambda u: (
        replace(u, config=replace(u.config, persist_docs={"columns": False})),
        replace(u, config=replace(u.config, persist_docs={"columns": False})),
    ),
    # True -> True
    lambda u: (
        replace(u, config=replace(u.config, persist_docs={"relation": True})),
        replace(u, config=replace(u.config, persist_docs={"relation": True})),
    ),
    lambda u: (
        replace(u, config=replace(u.config, persist_docs={"columns": True})),
        replace(u, config=replace(u.config, persist_docs={"columns": True})),
    ),
    # only columns docs enabled, but description changed
    lambda u: (
        replace(u, config=replace(u.config, persist_docs={"columns": True})),
        replace(
            u,
            config=replace(u.config, persist_docs={"columns": True}),
            description="a model description",
        ),
    ),
    # only relation docs eanbled, but columns changed
    lambda u: (
        replace(u, config=replace(u.config, persist_docs={"relation": True})),
        replace(
            u,
            config=replace(u.config, persist_docs={"relation": True}),
            columns={"a": ColumnInfo(name="a", description="a column description")},
        ),
    ),
]


changed_compiled_models = [
    lambda u: (u, None),
    lambda u: (u, replace(u, raw_code="select * from wherever")),
    lambda u: (
        u,
        replace(
            u,
            fqn=["test", "models", "subdir", "foo"],
            original_file_path="models/subdir/foo.sql",
            path="/root/models/subdir/foo.sql",
        ),
    ),
    lambda u: (u, replace_config(u, full_refresh=True)),
    lambda u: (u, replace_config(u, post_hook=["select 1 as id"])),
    lambda u: (u, replace_config(u, pre_hook=["select 1 as id"])),
    lambda u: (
        u,
        replace_config(u, quoting={"database": True, "schema": False, "identifier": False}),
    ),
    # we changed persist_docs values
    lambda u: (u, replace_config(u, persist_docs={"relation": True})),
    lambda u: (u, replace_config(u, persist_docs={"columns": True})),
    lambda u: (u, replace_config(u, persist_docs={"columns": True, "relation": True})),
    # None -> False is a config change even though it's pretty much the same
    lambda u: (u, replace_config(u, persist_docs={"relation": False})),
    lambda u: (u, replace_config(u, persist_docs={"columns": False})),
    # persist docs was true for the relation and we changed the model description
    lambda u: (
        replace_config(u, persist_docs={"relation": True}),
        replace_config(u, persist_docs={"relation": True}, description="a model description"),
    ),
    # persist docs was true for columns and we changed the model description
    lambda u: (
        replace_config(u, persist_docs={"columns": True}),
        replace_config(
            u,
            persist_docs={"columns": True},
            columns={"a": ColumnInfo(name="a", description="a column description")},
        ),
    ),
    # changing alias/schema/database on the config level is a change
    lambda u: (u, replace_config(u, database="nope")),
    lambda u: (u, replace_config(u, schema="nope")),
    lambda u: (u, replace_config(u, alias="nope")),
]


@pytest.mark.parametrize("func", unchanged_compiled_models)
def test_compare_unchanged_model(func, basic_uncompiled_model):
    node, compare = func(basic_uncompiled_model)
    assert node.same_contents(compare, "postgres")


@pytest.mark.parametrize("func", changed_compiled_models)
def test_compare_changed_model(func, basic_uncompiled_model):
    node, compare = func(basic_uncompiled_model)
    assert not node.same_contents(compare, "postgres")


@pytest.fixture
def minimal_schema_test_dict():
    return {
        "name": "foo",
        "created_at": 1,
        "resource_type": str(NodeType.Test),
        "path": "/root/x/path.sql",
        "original_file_path": "/root/path.sql",
        "package_name": "test",
        "language": "sql",
        "raw_code": 'select * from {{ ref("other") }}',
        "unique_id": "model.test.foo",
        "fqn": ["test", "models", "foo"],
        "database": "test_db",
        "schema": "dbt_test__audit",
        "alias": "bar",
        "test_metadata": {
            "name": "foo",
            "kwargs": {},
        },
        "compiled": False,
        "checksum": {
            "name": "sha256",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
    }


@pytest.fixture
def basic_uncompiled_schema_test_node():
    return GenericTestNode(
        package_name="test",
        path="/root/x/path.sql",
        original_file_path="/root/path.sql",
        language="sql",
        raw_code='select * from {{ ref("other") }}',
        name="foo",
        resource_type=NodeType.Test,
        unique_id="model.test.foo",
        fqn=["test", "models", "foo"],
        refs=[],
        sources=[],
        metrics=[],
        depends_on=DependsOn(),
        description="",
        database="test_db",
        schema="dbt_test__audit",
        alias="bar",
        tags=[],
        config=TestConfig(),
        meta={},
        compiled=False,
        extra_ctes=[],
        extra_ctes_injected=False,
        test_metadata=TestMetadata(namespace=None, name="foo", kwargs={}),
        checksum=FileHash.from_contents(""),
        unrendered_config={},
    )


@pytest.fixture
def basic_compiled_schema_test_node():
    return generic_test_node()


@pytest.fixture
def basic_uncompiled_schema_test_dict():
    return {
        "name": "foo",
        "created_at": 1,
        "resource_type": str(NodeType.Test),
        "path": "/root/x/path.sql",
        "original_file_path": "/root/path.sql",
        "package_name": "test",
        "language": "sql",
        "raw_code": 'select * from {{ ref("other") }}',
        "unique_id": "model.test.foo",
        "fqn": ["test", "models", "foo"],
        "refs": [],
        "sources": [],
        "metrics": [],
        "depends_on": {"macros": [], "nodes": []},
        "database": "test_db",
        "description": "",
        "schema": "dbt_test__audit",
        "alias": "bar",
        "tags": [],
        "config": {
            "enabled": True,
            "materialized": "test",
            "tags": [],
            "severity": "ERROR",
            "schema": "dbt_test__audit",
            "warn_if": "!= 0",
            "error_if": "!= 0",
            "fail_calc": "count(*)",
            "meta": {},
        },
        "docs": {"show": True},
        "columns": {},
        "meta": {},
        "compiled": False,
        "extra_ctes": [],
        "extra_ctes_injected": False,
        "test_metadata": {
            "name": "foo",
            "kwargs": {},
        },
        "checksum": {
            "name": "sha256",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
        "unrendered_config": {},
        "unrendered_config_call_dict": {},
        "config_call_dict": {},
    }


@pytest.fixture
def basic_compiled_schema_test_dict():
    return {
        "name": "foo",
        "created_at": 1,
        "resource_type": str(NodeType.Test),
        "path": "/root/x/path.sql",
        "original_file_path": "/root/path.sql",
        "package_name": "test",
        "language": "sql",
        "raw_code": 'select * from {{ ref("other") }}',
        "unique_id": "model.test.foo",
        "fqn": ["test", "models", "foo"],
        "refs": [],
        "sources": [],
        "metrics": [],
        "depends_on": {"macros": [], "nodes": []},
        "database": "test_db",
        "description": "",
        "schema": "dbt_test__audit",
        "alias": "bar",
        "tags": [],
        "config": {
            "enabled": True,
            "materialized": "test",
            "tags": [],
            "severity": "warn",
            "schema": "dbt_test__audit",
            "warn_if": "!= 0",
            "error_if": "!= 0",
            "fail_calc": "count(*)",
            "meta": {},
        },
        "docs": {"show": True},
        "columns": {},
        "contract": {"enforced": False, "alias_types": True},
        "meta": {},
        "compiled": True,
        "extra_ctes": [{"id": "whatever", "sql": "select * from other"}],
        "extra_ctes_injected": True,
        "compiled_code": "with whatever as (select * from other) select * from whatever",
        "column_name": "id",
        "test_metadata": {
            "name": "foo",
            "kwargs": {},
        },
        "checksum": {
            "name": "sha256",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
        "unrendered_config": {
            "severity": "warn",
        },
        "unrendered_config_call_dict": {},
        "vars": {},
        "config_call_dict": {},
    }


@pytest.mark.skip("Haven't found where we would use uncompiled node")
def test_basic_uncompiled_schema_test(
    basic_uncompiled_schema_test_node, basic_uncompiled_schema_test_dict, minimal_schema_test_dict
):
    node = basic_uncompiled_schema_test_node
    node_dict = basic_uncompiled_schema_test_dict
    minimum = minimal_schema_test_dict
    assert_symmetric(node, node_dict, GenericTestNode)
    assert node.empty is False
    assert node.is_refable is False
    assert node.is_ephemeral is False

    assert_from_dict(node, minimum, GenericTestNode)


def test_basic_compiled_schema_test(
    basic_compiled_schema_test_node, basic_compiled_schema_test_dict
):
    node = basic_compiled_schema_test_node
    node_dict = basic_compiled_schema_test_dict

    assert_symmetric(node, node_dict, GenericTestNode)
    assert node.empty is False
    assert node.is_refable is False
    assert node.is_ephemeral is False


def test_invalid_extra_schema_test_fields(minimal_schema_test_dict):
    bad_extra = minimal_schema_test_dict
    bad_extra["extra"] = "extra value"
    assert_fails_validation(bad_extra, GenericTestNode)


def test_invalid_resource_type_schema_test(minimal_schema_test_dict):
    bad_type = minimal_schema_test_dict
    bad_type["resource_type"] = str(NodeType.Model)
    assert_fails_validation(bad_type, GenericTestNode)


unchanged_schema_tests = [
    # for tests, raw_code isn't a change (because it's always the same for a given test macro)
    lambda u: replace(u, raw_code="select * from wherever"),
    lambda u: replace(u, description="a description"),
    lambda u: replace(u, tags=["mytag"]),
    lambda u: replace(u, meta={"cool_key": "cool value"}),
    # these values don't even mean anything on schema tests!
    lambda u: replace_config(u, alias="nope"),
    lambda u: replace_config(u, database="nope"),
    lambda u: replace_config(u, schema="nope"),
    lambda u: replace(u, database="other_db"),
    lambda u: replace(u, schema="other_schema"),
    lambda u: replace(u, alias="foo"),
    lambda u: replace_config(u, full_refresh=True),
    lambda u: replace_config(u, post_hook=["select 1 as id"]),
    lambda u: replace_config(u, pre_hook=["select 1 as id"]),
    lambda u: replace_config(u, quoting={"database": True, "schema": False, "identifier": False}),
]


changed_schema_tests = [
    lambda u: None,
    lambda u: replace(
        u,
        fqn=["test", "models", "subdir", "foo"],
        original_file_path="models/subdir/foo.sql",
        path="/root/models/subdir/foo.sql",
    ),
    lambda u: replace_config(u, severity="warn"),
    # If we checked test metadata, these would caount. But we don't, because these changes would all change the unique ID, so it's irrelevant.
    # lambda u: replace(u, test_metadata=replace(u.test_metadata, namespace='something')),
    # lambda u: replace(u, test_metadata=replace(u.test_metadata, name='bar')),
    # lambda u: replace(u, test_metadata=replace(u.test_metadata, kwargs={'arg': 'value'})),
]


@pytest.mark.parametrize("func", unchanged_schema_tests)
def test_compare_unchanged_schema_test(func, basic_uncompiled_schema_test_node):
    value = func(basic_uncompiled_schema_test_node)
    assert basic_uncompiled_schema_test_node.same_contents(value, "postgres")


@pytest.mark.parametrize("func", changed_schema_tests)
def test_compare_changed_schema_test(func, basic_uncompiled_schema_test_node):
    value = func(basic_uncompiled_schema_test_node)
    assert not basic_uncompiled_schema_test_node.same_contents(value, "postgres")


def test_compare_to_compiled(basic_uncompiled_schema_test_node, basic_compiled_schema_test_node):
    # if you fix the severity, they should be the "same".
    uncompiled = basic_uncompiled_schema_test_node
    compiled = basic_compiled_schema_test_node
    assert not uncompiled.same_contents(compiled, "postgres")
    fixed_config = replace(compiled.config, severity=uncompiled.config.severity)
    fixed_compiled = replace(
        compiled, config=fixed_config, unrendered_config=uncompiled.unrendered_config
    )
    assert uncompiled.same_contents(fixed_compiled, "postgres")


def test_inject_ctes_simple1():
    starting_sql = "select * from __dbt__cte__base"
    ctes = [
        InjectedCTE(
            id="model.test.base",
            sql=" __dbt__cte__base as (\n\n\nselect * from test16873767336887004702_test_ephemeral.seed\n)",
        )
    ]
    expected_sql = """with __dbt__cte__base as (
        select * from test16873767336887004702_test_ephemeral.seed
        ) select * from __dbt__cte__base"""

    generated_sql = inject_ctes_into_sql(starting_sql, ctes)
    assert norm_whitespace(generated_sql) == norm_whitespace(expected_sql)


def test_inject_ctes_simple2():
    starting_sql = "select * from __dbt__cte__ephemeral_level_two"
    ctes = [
        InjectedCTE(
            id="model.test.ephemeral_level_two",
            sql=' __dbt__cte__ephemeral_level_two as (\n\nselect * from "dbt"."test16873757769710148165_test_ephemeral"."source_table"\n)',
        )
    ]
    expected_sql = """with __dbt__cte__ephemeral_level_two as (
        select * from "dbt"."test16873757769710148165_test_ephemeral"."source_table"
        ) select * from __dbt__cte__ephemeral_level_two"""

    generated_sql = inject_ctes_into_sql(starting_sql, ctes)
    assert norm_whitespace(generated_sql) == norm_whitespace(expected_sql)


def test_inject_ctes_multiple_ctes():

    starting_sql = "select * from __dbt__cte__ephemeral"
    ctes = [
        InjectedCTE(
            id="model.test.ephemeral_level_two",
            sql=' __dbt__cte__ephemeral_level_two as (\n\nselect * from "dbt"."test16873735573223965828_test_ephemeral"."source_table"\n)',
        ),
        InjectedCTE(
            id="model.test.ephemeral",
            sql=" __dbt__cte__ephemeral as (\n\nselect * from __dbt__cte__ephemeral_level_two\n)",
        ),
    ]
    expected_sql = """with __dbt__cte__ephemeral_level_two as (
            select * from "dbt"."test16873735573223965828_test_ephemeral"."source_table"
        ),  __dbt__cte__ephemeral as (
            select * from __dbt__cte__ephemeral_level_two
        ) select * from __dbt__cte__ephemeral"""

    generated_sql = inject_ctes_into_sql(starting_sql, ctes)
    assert norm_whitespace(generated_sql) == norm_whitespace(expected_sql)


def test_inject_ctes_multiple_ctes_more_complex():
    starting_sql = """select * from __dbt__cte__female_only
        union all
        select * from "dbt"."test16873757723266827902_test_ephemeral"."double_dependent" where gender = 'Male'"""
    ctes = [
        InjectedCTE(
            id="model.test.base",
            sql=" __dbt__cte__base as (\n\n\nselect * from test16873757723266827902_test_ephemeral.seed\n)",
        ),
        InjectedCTE(
            id="model.test.base_copy",
            sql=" __dbt__cte__base_copy as (\n\n\nselect * from __dbt__cte__base\n)",
        ),
        InjectedCTE(
            id="model.test.female_only",
            sql=" __dbt__cte__female_only as (\n\n\nselect * from __dbt__cte__base_copy where gender = 'Female'\n)",
        ),
    ]
    expected_sql = """with __dbt__cte__base as (
            select * from test16873757723266827902_test_ephemeral.seed
        ),  __dbt__cte__base_copy as (
            select * from __dbt__cte__base
        ),  __dbt__cte__female_only as (
            select * from __dbt__cte__base_copy where gender = 'Female'
        ) select * from __dbt__cte__female_only
        union all
        select * from "dbt"."test16873757723266827902_test_ephemeral"."double_dependent" where gender = 'Male'"""

    generated_sql = inject_ctes_into_sql(starting_sql, ctes)
    assert norm_whitespace(generated_sql) == norm_whitespace(expected_sql)


def test_inject_ctes_starting_with1():
    starting_sql = """
       with internal_cte as (select * from sessions)
       select * from internal_cte
    """
    ctes = [
        InjectedCTE(
            id="cte_id_1",
            sql="__dbt__cte__ephemeral as (select * from table)",
        ),
        InjectedCTE(
            id="cte_id_2",
            sql="__dbt__cte__events as (select id, type from events)",
        ),
    ]
    expected_sql = """with __dbt__cte__ephemeral as (select * from table),
       __dbt__cte__events as (select id, type from events),
       internal_cte as (select * from sessions)
       select * from internal_cte"""

    generated_sql = inject_ctes_into_sql(starting_sql, ctes)
    assert norm_whitespace(generated_sql) == norm_whitespace(expected_sql)


def test_inject_ctes_starting_with2():
    starting_sql = """with my_other_cool_cte as (
        select id, name from __dbt__cte__ephemeral
        where id > 1000
    )
    select name, id from my_other_cool_cte"""
    ctes = [
        InjectedCTE(
            id="model.singular_tests_ephemeral.ephemeral",
            sql=' __dbt__cte__ephemeral as (\n\n\nwith my_cool_cte as (\n  select name, id from "dbt"."test16873917221900185954_test_singular_tests_ephemeral"."base"\n)\nselect id, name from my_cool_cte where id is not null\n)',
        )
    ]
    expected_sql = """with  __dbt__cte__ephemeral as (
        with my_cool_cte as (
          select name, id from "dbt"."test16873917221900185954_test_singular_tests_ephemeral"."base"
        )
        select id, name from my_cool_cte where id is not null
        ), my_other_cool_cte as (
            select id, name from __dbt__cte__ephemeral
            where id > 1000
        )
        select name, id from my_other_cool_cte"""

    generated_sql = inject_ctes_into_sql(starting_sql, ctes)
    assert norm_whitespace(generated_sql) == norm_whitespace(expected_sql)


def test_inject_ctes_comment_with():
    # Test injection with a comment containing "with"
    starting_sql = """
        --- This is sql with a comment
        select * from __dbt__cte__base
    """
    ctes = [
        InjectedCTE(
            id="model.test.base",
            sql=" __dbt__cte__base as (\n\n\nselect * from test16873767336887004702_test_ephemeral.seed\n)",
        )
    ]
    expected_sql = """with __dbt__cte__base as (
        select * from test16873767336887004702_test_ephemeral.seed
        ) --- This is sql with a comment
        select * from __dbt__cte__base"""

    generated_sql = inject_ctes_into_sql(starting_sql, ctes)
    assert norm_whitespace(generated_sql) == norm_whitespace(expected_sql)


def test_inject_ctes_with_recursive():
    # Test injection with "recursive" keyword
    starting_sql = """
        with recursive t(n) as (
            select * from __dbt__cte__first_ephemeral_model
          union all
            select n+1 from t where n < 100
        )
        select sum(n) from t
    """
    ctes = [
        InjectedCTE(
            id="model.test.first_ephemeral_model",
            sql=" __dbt__cte__first_ephemeral_model as (\n\nselect 1 as fun\n)",
        )
    ]
    expected_sql = """with recursive  __dbt__cte__first_ephemeral_model as (
        select 1 as fun
        ), t(n) as (
            select * from __dbt__cte__first_ephemeral_model
          union all
            select n+1 from t where n < 100
        )
        select sum(n) from t
    """
    generated_sql = inject_ctes_into_sql(starting_sql, ctes)
    assert norm_whitespace(generated_sql) == norm_whitespace(expected_sql)
