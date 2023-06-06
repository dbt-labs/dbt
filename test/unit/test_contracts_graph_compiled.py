import pickle
import pytest

from dbt.contracts.files import FileHash
from dbt.contracts.graph.nodes import (
    ColumnInfo,
    DependsOn,
    GenericTestNode,
    InjectedCTE,
    ModelNode,
    NodeConfig,
    TestConfig,
    TestMetadata,
    Contract,
)
from dbt.node_types import NodeType

from .utils import (
    assert_symmetric,
    assert_from_dict,
    assert_fails_validation,
    replace_config,
)


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
        deferred=False,
        description="",
        database="test_db",
        schema="test_schema",
        alias="bar",
        tags=[],
        config=NodeConfig(),
        meta={},
        compiled=False,
        extra_ctes=[],
        extra_ctes_injected=False,
        checksum=FileHash.from_contents(""),
        unrendered_config={},
    )


@pytest.fixture
def basic_compiled_model():
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
        deferred=True,
        description="",
        database="test_db",
        schema="test_schema",
        alias="bar",
        tags=[],
        config=NodeConfig(),
        contract=Contract(),
        meta={},
        compiled=True,
        extra_ctes=[InjectedCTE("whatever", "select * from other")],
        extra_ctes_injected=True,
        compiled_code="with whatever as (select * from other) select * from whatever",
        checksum=FileHash.from_contents(""),
        unrendered_config={},
    )


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
        "depends_on": {"macros": [], "nodes": [], "public_nodes": []},
        "database": "test_db",
        "deferred": False,
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
        "depends_on": {"macros": [], "nodes": [], "public_nodes": []},
        "database": "test_db",
        "deferred": True,
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
            "contract": {"enforced": False},
            "docs": {"show": True},
        },
        "docs": {"show": True},
        "columns": {},
        "contract": {"enforced": False},
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
    lambda u: (u, u.replace(description="a description")),
    lambda u: (u, u.replace(tags=["mytag"])),
    lambda u: (u, u.replace(meta={"cool_key": "cool value"})),
    # changing the final alias/schema/datbase isn't a change - could just be target changing!
    lambda u: (u, u.replace(database="nope")),
    lambda u: (u, u.replace(schema="nope")),
    lambda u: (u, u.replace(alias="nope")),
    # None -> False is a config change even though it's pretty much the same
    lambda u: (
        u.replace(config=u.config.replace(persist_docs={"relation": False})),
        u.replace(config=u.config.replace(persist_docs={"relation": False})),
    ),
    lambda u: (
        u.replace(config=u.config.replace(persist_docs={"columns": False})),
        u.replace(config=u.config.replace(persist_docs={"columns": False})),
    ),
    # True -> True
    lambda u: (
        u.replace(config=u.config.replace(persist_docs={"relation": True})),
        u.replace(config=u.config.replace(persist_docs={"relation": True})),
    ),
    lambda u: (
        u.replace(config=u.config.replace(persist_docs={"columns": True})),
        u.replace(config=u.config.replace(persist_docs={"columns": True})),
    ),
    # only columns docs enabled, but description changed
    lambda u: (
        u.replace(config=u.config.replace(persist_docs={"columns": True})),
        u.replace(
            config=u.config.replace(persist_docs={"columns": True}),
            description="a model description",
        ),
    ),
    # only relation docs eanbled, but columns changed
    lambda u: (
        u.replace(config=u.config.replace(persist_docs={"relation": True})),
        u.replace(
            config=u.config.replace(persist_docs={"relation": True}),
            columns={"a": ColumnInfo(name="a", description="a column description")},
        ),
    ),
]


changed_compiled_models = [
    lambda u: (u, None),
    lambda u: (u, u.replace(raw_code="select * from wherever")),
    lambda u: (
        u,
        u.replace(
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
        deferred=False,
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
        deferred=False,
        description="",
        database="test_db",
        schema="dbt_test__audit",
        alias="bar",
        tags=[],
        config=TestConfig(severity="warn"),
        contract=Contract(),
        meta={},
        compiled=True,
        extra_ctes=[InjectedCTE("whatever", "select * from other")],
        extra_ctes_injected=True,
        compiled_code="with whatever as (select * from other) select * from whatever",
        column_name="id",
        test_metadata=TestMetadata(namespace=None, name="foo", kwargs={}),
        checksum=FileHash.from_contents(""),
        unrendered_config={
            "severity": "warn",
        },
    )


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
        "depends_on": {"macros": [], "nodes": [], "public_nodes": []},
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
        "deferred": False,
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
        "depends_on": {"macros": [], "nodes": [], "public_nodes": []},
        "deferred": False,
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
        "contract": {"enforced": False},
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
    lambda u: u.replace(raw_code="select * from wherever"),
    lambda u: u.replace(description="a description"),
    lambda u: u.replace(tags=["mytag"]),
    lambda u: u.replace(meta={"cool_key": "cool value"}),
    # these values don't even mean anything on schema tests!
    lambda u: replace_config(u, alias="nope"),
    lambda u: replace_config(u, database="nope"),
    lambda u: replace_config(u, schema="nope"),
    lambda u: u.replace(database="other_db"),
    lambda u: u.replace(schema="other_schema"),
    lambda u: u.replace(alias="foo"),
    lambda u: replace_config(u, full_refresh=True),
    lambda u: replace_config(u, post_hook=["select 1 as id"]),
    lambda u: replace_config(u, pre_hook=["select 1 as id"]),
    lambda u: replace_config(u, quoting={"database": True, "schema": False, "identifier": False}),
]


changed_schema_tests = [
    lambda u: None,
    lambda u: u.replace(
        fqn=["test", "models", "subdir", "foo"],
        original_file_path="models/subdir/foo.sql",
        path="/root/models/subdir/foo.sql",
    ),
    lambda u: replace_config(u, severity="warn"),
    # If we checked test metadata, these would caount. But we don't, because these changes would all change the unique ID, so it's irrelevant.
    # lambda u: u.replace(test_metadata=u.test_metadata.replace(namespace='something')),
    # lambda u: u.replace(test_metadata=u.test_metadata.replace(name='bar')),
    # lambda u: u.replace(test_metadata=u.test_metadata.replace(kwargs={'arg': 'value'})),
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
    fixed_config = compiled.config.replace(severity=uncompiled.config.severity)
    fixed_compiled = compiled.replace(
        config=fixed_config, unrendered_config=uncompiled.unrendered_config
    )
    assert uncompiled.same_contents(fixed_compiled, "postgres")
