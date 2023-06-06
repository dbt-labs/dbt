import pickle
import pytest

from dbt.node_types import NodeType, AccessType
from dbt.contracts.files import FileHash
from dbt.contracts.graph.model_config import (
    NodeConfig,
    SeedConfig,
    TestConfig,
    SnapshotConfig,
    SourceConfig,
    ExposureConfig,
    EmptySnapshotConfig,
    Hook,
)
from dbt.contracts.graph.nodes import (
    ModelNode,
    DependsOn,
    ColumnInfo,
    GenericTestNode,
    SnapshotNode,
    IntermediateSnapshotNode,
    Macro,
    Exposure,
    Metric,
    SeedNode,
    Docs,
    MacroDependsOn,
    SourceDefinition,
    Documentation,
    HookNode,
    Owner,
    TestMetadata,
)
from dbt.contracts.graph.unparsed import (
    ExposureType,
    FreshnessThreshold,
    MaturityType,
    Quoting,
    Time,
    TimePeriod,
)
from dbt import flags
from argparse import Namespace

from dbt.dataclass_schema import ValidationError
from .utils import (
    ContractTestCase,
    assert_symmetric,
    assert_from_dict,
    compare_dicts,
    assert_fails_validation,
    dict_replace,
    replace_config,
)

flags.set_from_args(Namespace(SEND_ANONYMOUS_USAGE_STATS=False), None)


@pytest.fixture
def populated_node_config_object():
    result = NodeConfig(
        column_types={"a": "text"},
        materialized="table",
        post_hook=[Hook(sql='insert into blah(a, b) select "1", 1')],
    )
    result._extra["extra"] = "even more"
    return result


@pytest.fixture
def populated_node_config_dict():
    return {
        "column_types": {"a": "text"},
        "enabled": True,
        "materialized": "table",
        "persist_docs": {},
        "post-hook": [{"sql": 'insert into blah(a, b) select "1", 1', "transaction": True}],
        "pre-hook": [],
        "quoting": {},
        "tags": [],
        "extra": "even more",
        "on_schema_change": "ignore",
        "on_configuration_change": "apply",
        "meta": {},
        "grants": {},
        "packages": [],
        "docs": {"show": True},
        "contract": {"enforced": False},
    }


def test_config_populated(populated_node_config_object, populated_node_config_dict):
    assert_symmetric(populated_node_config_object, populated_node_config_dict, NodeConfig)
    pickle.loads(pickle.dumps(populated_node_config_object))


@pytest.fixture
def unrendered_node_config_dict():
    return {
        "column_types": {"a": "text"},
        "materialized": "table",
        "post_hook": 'insert into blah(a, b) select "1", 1',
    }


different_node_configs = [
    lambda c: dict_replace(c, post_hook=[]),
    lambda c: dict_replace(c, materialized="view"),
    lambda c: dict_replace(c, quoting={"database": True}),
    lambda c: dict_replace(c, extra="different extra"),
    lambda c: dict_replace(c, column_types={"a": "varchar(256)"}),
]


same_node_configs = [
    lambda c: dict_replace(c, tags=["mytag"]),
    lambda c: dict_replace(c, alias="changed"),
    lambda c: dict_replace(c, schema="changed"),
    lambda c: dict_replace(c, database="changed"),
]


@pytest.mark.parametrize("func", different_node_configs)
def test_config_different(unrendered_node_config_dict, func):
    value = func(unrendered_node_config_dict)
    assert not NodeConfig.same_contents(unrendered_node_config_dict, value)


@pytest.mark.parametrize("func", same_node_configs)
def test_config_same(unrendered_node_config_dict, func):
    value = func(unrendered_node_config_dict)
    assert unrendered_node_config_dict != value
    assert NodeConfig.same_contents(unrendered_node_config_dict, value)


@pytest.fixture
def base_parsed_model_dict():
    return {
        "name": "foo",
        "created_at": 1.0,
        "resource_type": str(NodeType.Model),
        "path": "/root/x/path.sql",
        "original_file_path": "/root/path.sql",
        "package_name": "test",
        "language": "sql",
        "raw_code": "select * from wherever",
        "unique_id": "model.test.foo",
        "fqn": ["test", "models", "foo"],
        "refs": [],
        "sources": [],
        "metrics": [],
        "depends_on": {"macros": [], "nodes": [], "public_nodes": []},
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
            "docs": {"show": True},
            "contract": {"enforced": False},
            "packages": [],
        },
        "deferred": False,
        "docs": {"show": True},
        "contract": {"enforced": False},
        "columns": {},
        "meta": {},
        "checksum": {
            "name": "sha256",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
        "unrendered_config": {},
        "config_call_dict": {},
        "access": AccessType.Protected.value,
        "constraints": [],
    }


@pytest.fixture
def basic_parsed_model_object():
    return ModelNode(
        package_name="test",
        path="/root/x/path.sql",
        original_file_path="/root/path.sql",
        language="sql",
        raw_code="select * from wherever",
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
        config=NodeConfig(),
        meta={},
        checksum=FileHash.from_contents(""),
        created_at=1.0,
    )


@pytest.fixture
def minimal_parsed_model_dict():
    return {
        "name": "foo",
        "created_at": 1.0,
        "resource_type": str(NodeType.Model),
        "path": "/root/x/path.sql",
        "original_file_path": "/root/path.sql",
        "package_name": "test",
        "language": "sql",
        "raw_code": "select * from wherever",
        "unique_id": "model.test.foo",
        "fqn": ["test", "models", "foo"],
        "database": "test_db",
        "schema": "test_schema",
        "alias": "bar",
        "checksum": {
            "name": "sha256",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
        "unrendered_config": {},
    }


@pytest.fixture
def complex_parsed_model_dict():
    return {
        "name": "foo",
        "created_at": 1.0,
        "resource_type": str(NodeType.Model),
        "path": "/root/x/path.sql",
        "original_file_path": "/root/path.sql",
        "package_name": "test",
        "language": "sql",
        "raw_code": 'select * from {{ ref("bar") }}',
        "unique_id": "model.test.foo",
        "fqn": ["test", "models", "foo"],
        "refs": [],
        "sources": [],
        "metrics": [],
        "depends_on": {"macros": [], "nodes": ["model.test.bar"], "public_nodes": []},
        "database": "test_db",
        "deferred": True,
        "description": "My parsed node",
        "schema": "test_schema",
        "alias": "bar",
        "tags": ["tag"],
        "meta": {},
        "config": {
            "column_types": {"a": "text"},
            "enabled": True,
            "materialized": "ephemeral",
            "persist_docs": {},
            "post-hook": [{"sql": 'insert into blah(a, b) select "1", 1', "transaction": True}],
            "pre-hook": [],
            "quoting": {},
            "tags": [],
            "on_schema_change": "ignore",
            "on_configuration_change": "apply",
            "meta": {},
            "grants": {},
            "docs": {"show": True},
            "contract": {"enforced": False},
            "packages": [],
        },
        "docs": {"show": True},
        "contract": {"enforced": False},
        "columns": {
            "a": {
                "name": "a",
                "description": "a text field",
                "meta": {},
                "tags": [],
                "constraints": [],
            },
        },
        "checksum": {
            "name": "sha256",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
        "unrendered_config": {
            "column_types": {"a": "text"},
            "materialized": "ephemeral",
            "post_hook": ['insert into blah(a, b) select "1", 1'],
        },
        "config_call_dict": {},
        "access": AccessType.Protected.value,
        "constraints": [],
    }


@pytest.fixture
def complex_parsed_model_object():
    return ModelNode(
        package_name="test",
        path="/root/x/path.sql",
        original_file_path="/root/path.sql",
        language="sql",
        raw_code='select * from {{ ref("bar") }}',
        name="foo",
        resource_type=NodeType.Model,
        unique_id="model.test.foo",
        fqn=["test", "models", "foo"],
        refs=[],
        sources=[],
        metrics=[],
        depends_on=DependsOn(nodes=["model.test.bar"]),
        deferred=True,
        description="My parsed node",
        database="test_db",
        schema="test_schema",
        alias="bar",
        tags=["tag"],
        meta={},
        config=NodeConfig(
            column_types={"a": "text"},
            materialized="ephemeral",
            post_hook=[Hook(sql='insert into blah(a, b) select "1", 1')],
        ),
        columns={"a": ColumnInfo("a", "a text field", {})},
        checksum=FileHash.from_contents(""),
        unrendered_config={
            "column_types": {"a": "text"},
            "materialized": "ephemeral",
            "post_hook": ['insert into blah(a, b) select "1", 1'],
        },
    )


{
    "enabled": True,
    "tags": [],
    "meta": {},
    "materialized": "ephemeral",
    "persist_docs": {},
    "quoting": {},
    "column_types": {"a": "text"},
    "on_schema_change": "ignore",
    "on_configuration_change": "apply",
    "grants": {},
    "packages": [],
    "docs": {"show": True},
    "contract": {"enforced": False},
    "post-hook": [{"sql": 'insert into blah(a, b) select "1", 1', "transaction": True}],
    "pre-hook": [],
}

{
    "column_types": {"a": "text"},
    "enabled": True,
    "materialized": "ephemeral",
    "persist_docs": {},
    "post-hook": [{"sql": 'insert into blah(a, b) select "1", 1', "transaction": True}],
    "pre-hook": [],
    "quoting": {},
    "tags": [],
    "on_schema_change": "ignore",
    "on_configuration_change": "apply",
    "meta": {},
    "grants": {},
    "docs": {"show": True},
    "packages": [],
}


def test_model_basic(basic_parsed_model_object, base_parsed_model_dict, minimal_parsed_model_dict):
    node = basic_parsed_model_object
    node_dict = base_parsed_model_dict
    compare_dicts(node.to_dict(), node_dict)
    assert_symmetric(node, node_dict)
    assert node.empty is False
    assert node.is_refable is True
    assert node.is_ephemeral is False

    minimum = minimal_parsed_model_dict
    assert_from_dict(node, minimum)
    pickle.loads(pickle.dumps(node))


def test_model_complex(complex_parsed_model_object, complex_parsed_model_dict):
    node = complex_parsed_model_object
    node_dict = complex_parsed_model_dict
    assert_symmetric(node, node_dict)
    assert node.empty is False
    assert node.is_refable is True
    assert node.is_ephemeral is True


def test_invalid_bad_tags(base_parsed_model_dict):
    # bad top-level field
    bad_tags = base_parsed_model_dict
    bad_tags["tags"] = 100
    assert_fails_validation(bad_tags, ModelNode)


def test_invalid_bad_materialized(base_parsed_model_dict):
    # bad nested field
    bad_materialized = base_parsed_model_dict
    bad_materialized["config"]["materialized"] = None
    assert_fails_validation(bad_materialized, ModelNode)


unchanged_nodes = [
    lambda u: (u, u.replace(tags=["mytag"])),
    lambda u: (u, u.replace(meta={"something": 1000})),
    # True -> True
    lambda u: (
        replace_config(u, persist_docs={"relation": True}),
        replace_config(u, persist_docs={"relation": True}),
    ),
    lambda u: (
        replace_config(u, persist_docs={"columns": True}),
        replace_config(u, persist_docs={"columns": True}),
    ),
    # only columns docs enabled, but description changed
    lambda u: (
        replace_config(u, persist_docs={"columns": True}),
        replace_config(u, persist_docs={"columns": True}).replace(
            description="a model description"
        ),
    ),
    # only relation docs eanbled, but columns changed
    lambda u: (
        replace_config(u, persist_docs={"relation": True}),
        replace_config(u, persist_docs={"relation": True}).replace(
            columns={"a": ColumnInfo(name="a", description="a column description")}
        ),
    ),
    # not tracked, we track config.alias/config.schema/config.database
    lambda u: (u, u.replace(alias="other")),
    lambda u: (u, u.replace(schema="other")),
    lambda u: (u, u.replace(database="other")),
]


changed_nodes = [
    lambda u: (
        u,
        u.replace(
            fqn=["test", "models", "subdir", "foo"],
            original_file_path="models/subdir/foo.sql",
            path="/root/models/subdir/foo.sql",
        ),
    ),
    # None -> False is a config change even though it's pretty much the same
    lambda u: (u, replace_config(u, persist_docs={"relation": False})),
    lambda u: (u, replace_config(u, persist_docs={"columns": False})),
    # persist docs was true for the relation and we changed the model description
    lambda u: (
        replace_config(u, persist_docs={"relation": True}),
        replace_config(u, persist_docs={"relation": True}).replace(
            description="a model description"
        ),
    ),
    # persist docs was true for columns and we changed the model description
    lambda u: (
        replace_config(u, persist_docs={"columns": True}),
        replace_config(u, persist_docs={"columns": True}).replace(
            columns={"a": ColumnInfo(name="a", description="a column description")}
        ),
    ),
    # not tracked, we track config.alias/config.schema/config.database
    lambda u: (u, replace_config(u, alias="other")),
    lambda u: (u, replace_config(u, schema="other")),
    lambda u: (u, replace_config(u, database="other")),
]


@pytest.mark.parametrize("func", unchanged_nodes)
def test_compare_unchanged_parsed_model(func, basic_parsed_model_object):
    node, compare = func(basic_parsed_model_object)
    assert node.same_contents(compare, "postgres")


@pytest.mark.parametrize("func", changed_nodes)
def test_compare_changed_model(func, basic_parsed_model_object):
    node, compare = func(basic_parsed_model_object)
    assert not node.same_contents(compare, "postgres")


@pytest.fixture
def basic_parsed_seed_dict():
    return {
        "name": "foo",
        "created_at": 1.0,
        "resource_type": str(NodeType.Seed),
        "path": "/root/seeds/seed.csv",
        "original_file_path": "seeds/seed.csv",
        "package_name": "test",
        "raw_code": "",
        "unique_id": "seed.test.foo",
        "fqn": ["test", "seeds", "foo"],
        "database": "test_db",
        "depends_on": {"macros": []},
        "description": "",
        "schema": "test_schema",
        "tags": [],
        "alias": "foo",
        "config": {
            "column_types": {},
            "enabled": True,
            "materialized": "seed",
            "persist_docs": {},
            "post-hook": [],
            "pre-hook": [],
            "quoting": {},
            "tags": [],
            "on_schema_change": "ignore",
            "on_configuration_change": "apply",
            "meta": {},
            "grants": {},
            "docs": {"show": True},
            "contract": {"enforced": False},
            "packages": [],
        },
        "deferred": False,
        "docs": {"show": True},
        "columns": {},
        "meta": {},
        "checksum": {"name": "path", "checksum": "seeds/seed.csv"},
        "unrendered_config": {},
        "config_call_dict": {},
    }


@pytest.fixture
def basic_parsed_seed_object():
    return SeedNode(
        name="foo",
        resource_type=NodeType.Seed,
        path="/root/seeds/seed.csv",
        original_file_path="seeds/seed.csv",
        package_name="test",
        raw_code="",
        unique_id="seed.test.foo",
        fqn=["test", "seeds", "foo"],
        database="test_db",
        description="",
        schema="test_schema",
        tags=[],
        alias="foo",
        config=SeedConfig(),
        # config=SeedConfig(quote_columns=True),
        deferred=False,
        docs=Docs(show=True),
        columns={},
        meta={},
        checksum=FileHash(name="path", checksum="seeds/seed.csv"),
        unrendered_config={},
    )


@pytest.fixture
def minimal_parsed_seed_dict():
    return {
        "name": "foo",
        "created_at": 1.0,
        "resource_type": str(NodeType.Seed),
        "path": "/root/seeds/seed.csv",
        "original_file_path": "seeds/seed.csv",
        "package_name": "test",
        "raw_code": "",
        "unique_id": "seed.test.foo",
        "fqn": ["test", "seeds", "foo"],
        "database": "test_db",
        "schema": "test_schema",
        "alias": "foo",
        "checksum": {"name": "path", "checksum": "seeds/seed.csv"},
    }


@pytest.fixture
def complex_parsed_seed_dict():
    return {
        "name": "foo",
        "created_at": 1.0,
        "resource_type": str(NodeType.Seed),
        "path": "/root/seeds/seed.csv",
        "original_file_path": "seeds/seed.csv",
        "package_name": "test",
        "raw_code": "",
        "unique_id": "seed.test.foo",
        "fqn": ["test", "seeds", "foo"],
        "database": "test_db",
        "depends_on": {"macros": []},
        "description": "a description",
        "schema": "test_schema",
        "tags": ["mytag"],
        "alias": "foo",
        "config": {
            "column_types": {},
            "enabled": True,
            "materialized": "seed",
            "persist_docs": {"relation": True, "columns": True},
            "post-hook": [],
            "pre-hook": [],
            "quoting": {},
            "tags": [],
            "quote_columns": True,
            "on_schema_change": "ignore",
            "on_configuration_change": "apply",
            "meta": {},
            "grants": {},
            "docs": {"show": True},
            "contract": {"enforced": False},
            "packages": [],
        },
        "deferred": False,
        "docs": {"show": True},
        "columns": {
            "a": {
                "name": "a",
                "description": "a column description",
                "meta": {},
                "tags": [],
                "constraints": [],
            }
        },
        "meta": {"foo": 1000},
        "checksum": {
            "name": "sha256",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
        "unrendered_config": {
            "persist_docs": {"relation": True, "columns": True},
        },
        "config_call_dict": {},
    }


@pytest.fixture
def complex_parsed_seed_object():
    return SeedNode(
        name="foo",
        resource_type=NodeType.Seed,
        path="/root/seeds/seed.csv",
        original_file_path="seeds/seed.csv",
        package_name="test",
        raw_code="",
        unique_id="seed.test.foo",
        fqn=["test", "seeds", "foo"],
        database="test_db",
        depends_on=MacroDependsOn(),
        description="a description",
        schema="test_schema",
        tags=["mytag"],
        alias="foo",
        config=SeedConfig(
            quote_columns=True,
            persist_docs={"relation": True, "columns": True},
        ),
        deferred=False,
        docs=Docs(show=True),
        columns={"a": ColumnInfo(name="a", description="a column description")},
        meta={"foo": 1000},
        checksum=FileHash.from_contents(""),
        unrendered_config={
            "persist_docs": {"relation": True, "columns": True},
        },
    )


def test_seed_basic(basic_parsed_seed_dict, basic_parsed_seed_object, minimal_parsed_seed_dict):
    assert_symmetric(basic_parsed_seed_object, basic_parsed_seed_dict)

    assert basic_parsed_seed_object.get_materialization() == "seed"

    assert_from_dict(basic_parsed_seed_object, minimal_parsed_seed_dict, SeedNode)


def test_seed_complex(complex_parsed_seed_dict, complex_parsed_seed_object):
    assert_symmetric(complex_parsed_seed_object, complex_parsed_seed_dict)
    assert complex_parsed_seed_object.get_materialization() == "seed"


unchanged_seeds = [
    lambda u: (u, u.replace(tags=["mytag"])),
    lambda u: (u, u.replace(meta={"something": 1000})),
    # True -> True
    lambda u: (
        replace_config(u, persist_docs={"relation": True}),
        replace_config(u, persist_docs={"relation": True}),
    ),
    lambda u: (
        replace_config(u, persist_docs={"columns": True}),
        replace_config(u, persist_docs={"columns": True}),
    ),
    # only columns docs enabled, but description changed
    lambda u: (
        replace_config(u, persist_docs={"columns": True}),
        replace_config(u, persist_docs={"columns": True}).replace(
            description="a model description"
        ),
    ),
    # only relation docs eanbled, but columns changed
    lambda u: (
        replace_config(u, persist_docs={"relation": True}),
        replace_config(u, persist_docs={"relation": True}).replace(
            columns={"a": ColumnInfo(name="a", description="a column description")}
        ),
    ),
    lambda u: (u, u.replace(alias="other")),
    lambda u: (u, u.replace(schema="other")),
    lambda u: (u, u.replace(database="other")),
]


changed_seeds = [
    lambda u: (
        u,
        u.replace(
            fqn=["test", "models", "subdir", "foo"],
            original_file_path="models/subdir/foo.sql",
            path="/root/models/subdir/foo.sql",
        ),
    ),
    # None -> False is a config change even though it's pretty much the same
    lambda u: (u, replace_config(u, persist_docs={"relation": False})),
    lambda u: (u, replace_config(u, persist_docs={"columns": False})),
    # persist docs was true for the relation and we changed the model description
    lambda u: (
        replace_config(u, persist_docs={"relation": True}),
        replace_config(u, persist_docs={"relation": True}).replace(
            description="a model description"
        ),
    ),
    # persist docs was true for columns and we changed the model description
    lambda u: (
        replace_config(u, persist_docs={"columns": True}),
        replace_config(u, persist_docs={"columns": True}).replace(
            columns={"a": ColumnInfo(name="a", description="a column description")}
        ),
    ),
    lambda u: (u, replace_config(u, alias="other")),
    lambda u: (u, replace_config(u, schema="other")),
    lambda u: (u, replace_config(u, database="other")),
]


@pytest.mark.parametrize("func", unchanged_seeds)
def test_compare_unchanged_parsed_seed(func, basic_parsed_seed_object):
    node, compare = func(basic_parsed_seed_object)
    assert node.same_contents(compare, "postgres")


@pytest.mark.parametrize("func", changed_seeds)
def test_compare_changed_seed(func, basic_parsed_seed_object):
    node, compare = func(basic_parsed_seed_object)
    assert not node.same_contents(compare, "postgres")


@pytest.fixture
def minimal_parsed_hook_dict():
    return {
        "name": "foo",
        "resource_type": str(NodeType.Operation),
        "path": "/root/x/path.sql",
        "original_file_path": "/root/path.sql",
        "package_name": "test",
        "language": "sql",
        "raw_code": "select * from wherever",
        "unique_id": "model.test.foo",
        "fqn": ["test", "models", "foo"],
        "database": "test_db",
        "schema": "test_schema",
        "alias": "bar",
        "checksum": {
            "name": "sha256",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
    }


@pytest.fixture
def base_parsed_hook_dict():
    return {
        "name": "foo",
        "created_at": 1.0,
        "resource_type": str(NodeType.Operation),
        "path": "/root/x/path.sql",
        "original_file_path": "/root/path.sql",
        "package_name": "test",
        "language": "sql",
        "raw_code": "select * from wherever",
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
            "docs": {"show": True},
            "contract": {"enforced": False},
            "packages": [],
        },
        "docs": {"show": True},
        "contract": {"enforced": False},
        "columns": {},
        "meta": {},
        "checksum": {
            "name": "sha256",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
        "unrendered_config": {},
        "config_call_dict": {},
    }


@pytest.fixture
def base_parsed_hook_object():
    return HookNode(
        package_name="test",
        path="/root/x/path.sql",
        original_file_path="/root/path.sql",
        language="sql",
        raw_code="select * from wherever",
        name="foo",
        resource_type=NodeType.Operation,
        unique_id="model.test.foo",
        fqn=["test", "models", "foo"],
        refs=[],
        sources=[],
        metrics=[],
        depends_on=DependsOn(),
        description="",
        deferred=False,
        database="test_db",
        schema="test_schema",
        alias="bar",
        tags=[],
        config=NodeConfig(),
        index=None,
        checksum=FileHash.from_contents(""),
        unrendered_config={},
    )


@pytest.fixture
def complex_parsed_hook_dict():
    return {
        "name": "foo",
        "created_at": 1.0,
        "resource_type": str(NodeType.Operation),
        "path": "/root/x/path.sql",
        "original_file_path": "/root/path.sql",
        "package_name": "test",
        "language": "sql",
        "raw_code": 'select * from {{ ref("bar") }}',
        "unique_id": "model.test.foo",
        "fqn": ["test", "models", "foo"],
        "refs": [],
        "sources": [],
        "metrics": [],
        "depends_on": {"macros": [], "nodes": ["model.test.bar"], "public_nodes": []},
        "deferred": False,
        "database": "test_db",
        "description": "My parsed node",
        "schema": "test_schema",
        "alias": "bar",
        "tags": ["tag"],
        "meta": {},
        "config": {
            "column_types": {"a": "text"},
            "enabled": True,
            "materialized": "table",
            "persist_docs": {},
            "post-hook": [],
            "pre-hook": [],
            "quoting": {},
            "tags": [],
            "on_schema_change": "ignore",
            "on_configuration_change": "apply",
            "meta": {},
            "grants": {},
            "docs": {"show": True},
            "contract": {"enforced": False},
            "packages": [],
        },
        "docs": {"show": True},
        "contract": {"enforced": False},
        "columns": {
            "a": {
                "name": "a",
                "description": "a text field",
                "meta": {},
                "tags": [],
                "constraints": [],
            },
        },
        "index": 13,
        "checksum": {
            "name": "sha256",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
        "unrendered_config": {
            "column_types": {"a": "text"},
            "materialized": "table",
        },
        "config_call_dict": {},
    }


@pytest.fixture
def complex_parsed_hook_object():
    return HookNode(
        package_name="test",
        path="/root/x/path.sql",
        original_file_path="/root/path.sql",
        language="sql",
        raw_code='select * from {{ ref("bar") }}',
        name="foo",
        resource_type=NodeType.Operation,
        unique_id="model.test.foo",
        fqn=["test", "models", "foo"],
        refs=[],
        sources=[],
        metrics=[],
        depends_on=DependsOn(nodes=["model.test.bar"]),
        description="My parsed node",
        deferred=False,
        database="test_db",
        schema="test_schema",
        alias="bar",
        tags=["tag"],
        meta={},
        config=NodeConfig(column_types={"a": "text"}, materialized="table", post_hook=[]),
        columns={"a": ColumnInfo("a", "a text field", {})},
        index=13,
        checksum=FileHash.from_contents(""),
        unrendered_config={
            "column_types": {"a": "text"},
            "materialized": "table",
        },
    )


def test_basic_parsed_hook(
    minimal_parsed_hook_dict, base_parsed_hook_dict, base_parsed_hook_object
):
    node = base_parsed_hook_object
    node_dict = base_parsed_hook_dict
    minimum = minimal_parsed_hook_dict

    assert_symmetric(node, node_dict, HookNode)
    assert node.empty is False
    assert node.is_refable is False
    assert node.get_materialization() == "view"
    assert_from_dict(node, minimum, HookNode)
    pickle.loads(pickle.dumps(node))


def test_complex_parsed_hook(complex_parsed_hook_dict, complex_parsed_hook_object):
    node = complex_parsed_hook_object
    node_dict = complex_parsed_hook_dict
    # what's different?
    assert_symmetric(node, node_dict)
    assert node.empty is False
    assert node.is_refable is False
    assert node.get_materialization() == "table"


def test_invalid_hook_index_type(base_parsed_hook_dict):
    bad_index = base_parsed_hook_dict
    bad_index["index"] = "a string!?"
    assert_fails_validation(bad_index, HookNode)


@pytest.fixture
def minimal_parsed_schema_test_dict():
    return {
        "name": "foo",
        "created_at": 1.0,
        "resource_type": str(NodeType.Test),
        "path": "/root/x/path.sql",
        "original_file_path": "/root/path.sql",
        "package_name": "test",
        "language": "sql",
        "raw_code": "select * from wherever",
        "unique_id": "test.test.foo",
        "fqn": ["test", "models", "foo"],
        "database": "test_db",
        "schema": "test_schema",
        "alias": "bar",
        "meta": {},
        "test_metadata": {
            "name": "foo",
            "kwargs": {},
        },
        "checksum": {
            "name": "sha256",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
        "config_call_dict": {},
    }


@pytest.fixture
def basic_parsed_schema_test_dict():
    return {
        "name": "foo",
        "created_at": 1.0,
        "resource_type": str(NodeType.Test),
        "path": "/root/x/path.sql",
        "original_file_path": "/root/path.sql",
        "package_name": "test",
        "language": "sql",
        "raw_code": "select * from wherever",
        "unique_id": "test.test.foo",
        "fqn": ["test", "models", "foo"],
        "refs": [],
        "sources": [],
        "metrics": [],
        "depends_on": {"macros": [], "nodes": [], "public_nodes": []},
        "deferred": False,
        "database": "test_db",
        "description": "",
        "schema": "test_schema",
        "alias": "bar",
        "tags": [],
        "meta": {},
        "config": {
            "enabled": True,
            "materialized": "test",
            "tags": [],
            "severity": "ERROR",
            "warn_if": "!= 0",
            "error_if": "!= 0",
            "fail_calc": "count(*)",
            "meta": {},
            "schema": "dbt_test__audit",
        },
        "docs": {"show": True},
        "contract": {"enforced": False},
        "columns": {},
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
def basic_parsed_schema_test_object():
    return GenericTestNode(
        package_name="test",
        path="/root/x/path.sql",
        original_file_path="/root/path.sql",
        language="sql",
        raw_code="select * from wherever",
        name="foo",
        resource_type=NodeType.Test,
        unique_id="test.test.foo",
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
        meta={},
        config=TestConfig(),
        test_metadata=TestMetadata(namespace=None, name="foo", kwargs={}),
        checksum=FileHash.from_contents(""),
    )


@pytest.fixture
def complex_parsed_schema_test_dict():
    return {
        "name": "foo",
        "created_at": 1.0,
        "resource_type": str(NodeType.Test),
        "path": "/root/x/path.sql",
        "original_file_path": "/root/path.sql",
        "package_name": "test",
        "language": "sql",
        "raw_code": 'select * from {{ ref("bar") }}',
        "unique_id": "test.test.foo",
        "fqn": ["test", "models", "foo"],
        "refs": [],
        "sources": [],
        "metrics": [],
        "depends_on": {"macros": [], "nodes": ["model.test.bar"], "public_nodes": []},
        "database": "test_db",
        "deferred": False,
        "description": "My parsed node",
        "schema": "test_schema",
        "alias": "bar",
        "tags": ["tag"],
        "meta": {},
        "config": {
            "enabled": True,
            "materialized": "table",
            "tags": [],
            "severity": "WARN",
            "warn_if": "!= 0",
            "error_if": "!= 0",
            "fail_calc": "count(*)",
            "extra_key": "extra value",
            "meta": {},
            "schema": "dbt_test__audit",
        },
        "docs": {"show": False},
        "contract": {"enforced": False},
        "columns": {
            "a": {
                "name": "a",
                "description": "a text field",
                "meta": {},
                "tags": [],
                "constraints": [],
            },
        },
        "column_name": "id",
        "test_metadata": {
            "name": "foo",
            "kwargs": {},
        },
        "checksum": {
            "name": "sha256",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
        "unrendered_config": {"materialized": "table", "severity": "WARN"},
        "config_call_dict": {},
    }


@pytest.fixture
def complex_parsed_schema_test_object():
    cfg = TestConfig(materialized="table", severity="WARN")
    cfg._extra.update({"extra_key": "extra value"})
    return GenericTestNode(
        package_name="test",
        path="/root/x/path.sql",
        original_file_path="/root/path.sql",
        language="sql",
        raw_code='select * from {{ ref("bar") }}',
        name="foo",
        resource_type=NodeType.Test,
        unique_id="test.test.foo",
        fqn=["test", "models", "foo"],
        refs=[],
        sources=[],
        metrics=[],
        depends_on=DependsOn(nodes=["model.test.bar"]),
        description="My parsed node",
        database="test_db",
        schema="test_schema",
        alias="bar",
        tags=["tag"],
        meta={},
        config=cfg,
        columns={"a": ColumnInfo("a", "a text field", {})},
        column_name="id",
        docs=Docs(show=False),
        test_metadata=TestMetadata(namespace=None, name="foo", kwargs={}),
        checksum=FileHash.from_contents(""),
        unrendered_config={"materialized": "table", "severity": "WARN"},
    )


def test_basic_schema_test_node(
    minimal_parsed_schema_test_dict, basic_parsed_schema_test_dict, basic_parsed_schema_test_object
):
    node = basic_parsed_schema_test_object
    node_dict = basic_parsed_schema_test_dict
    minimum = minimal_parsed_schema_test_dict
    assert_symmetric(node, node_dict, GenericTestNode)

    assert node.empty is False
    assert node.is_ephemeral is False
    assert node.is_refable is False
    assert node.get_materialization() == "test"

    assert_from_dict(node, minimum, GenericTestNode)
    pickle.loads(pickle.dumps(node))


def test_complex_schema_test_node(
    complex_parsed_schema_test_dict, complex_parsed_schema_test_object
):
    # this tests for the presence of _extra keys
    node = complex_parsed_schema_test_object  # GenericTestNode
    assert node.config._extra["extra_key"]
    node_dict = complex_parsed_schema_test_dict
    assert_symmetric(node, node_dict)
    assert node.empty is False


def test_invalid_column_name_type(complex_parsed_schema_test_dict):
    # bad top-level field
    bad_column_name = complex_parsed_schema_test_dict
    bad_column_name["column_name"] = {}
    assert_fails_validation(bad_column_name, GenericTestNode)


def test_invalid_severity(complex_parsed_schema_test_dict):
    invalid_config_value = complex_parsed_schema_test_dict
    invalid_config_value["config"]["severity"] = "WERROR"
    assert_fails_validation(invalid_config_value, GenericTestNode)


@pytest.fixture
def basic_timestamp_snapshot_config_dict():
    return {
        "column_types": {},
        "enabled": True,
        "materialized": "snapshot",
        "persist_docs": {},
        "post-hook": [],
        "pre-hook": [],
        "quoting": {},
        "tags": [],
        "unique_key": "id",
        "strategy": "timestamp",
        "updated_at": "last_update",
        "target_database": "some_snapshot_db",
        "target_schema": "some_snapshot_schema",
        "on_schema_change": "ignore",
        "on_configuration_change": "apply",
        "meta": {},
        "grants": {},
        "packages": [],
        "docs": {"show": True},
        "contract": {"enforced": False},
    }


@pytest.fixture
def basic_timestamp_snapshot_config_object():
    return SnapshotConfig(
        strategy="timestamp",
        updated_at="last_update",
        unique_key="id",
        target_database="some_snapshot_db",
        target_schema="some_snapshot_schema",
    )


@pytest.fixture
def complex_timestamp_snapshot_config_dict():
    return {
        "column_types": {"a": "text"},
        "enabled": True,
        "materialized": "snapshot",
        "persist_docs": {},
        "post-hook": [{"sql": 'insert into blah(a, b) select "1", 1', "transaction": True}],
        "pre-hook": [],
        "quoting": {},
        "tags": [],
        "target_database": "some_snapshot_db",
        "target_schema": "some_snapshot_schema",
        "unique_key": "id",
        "extra": "even more",
        "strategy": "timestamp",
        "updated_at": "last_update",
        "on_schema_change": "ignore",
        "on_configuration_change": "apply",
        "meta": {},
        "grants": {},
        "packages": [],
        "docs": {"show": True},
        "contract": {"enforced": False},
    }


@pytest.fixture
def complex_timestamp_snapshot_config_object():
    cfg = SnapshotConfig(
        column_types={"a": "text"},
        materialized="snapshot",
        post_hook=[Hook(sql='insert into blah(a, b) select "1", 1')],
        strategy="timestamp",
        target_database="some_snapshot_db",
        target_schema="some_snapshot_schema",
        updated_at="last_update",
        unique_key="id",
    )
    cfg._extra["extra"] = "even more"
    return cfg


def test_basic_timestamp_snapshot_config(
    basic_timestamp_snapshot_config_dict, basic_timestamp_snapshot_config_object
):
    cfg = basic_timestamp_snapshot_config_object
    cfg_dict = basic_timestamp_snapshot_config_dict
    assert_symmetric(cfg, cfg_dict)
    pickle.loads(pickle.dumps(cfg))


def test_complex_timestamp_snapshot_config(
    complex_timestamp_snapshot_config_dict, complex_timestamp_snapshot_config_object
):
    cfg = complex_timestamp_snapshot_config_object
    cfg_dict = complex_timestamp_snapshot_config_dict
    assert_symmetric(cfg, cfg_dict, SnapshotConfig)


def test_invalid_missing_updated_at(basic_timestamp_snapshot_config_dict):
    bad_fields = basic_timestamp_snapshot_config_dict
    del bad_fields["updated_at"]
    bad_fields["check_cols"] = "all"
    assert_fails_validation(bad_fields, SnapshotConfig)


@pytest.fixture
def basic_check_snapshot_config_dict():
    return {
        "column_types": {},
        "enabled": True,
        "materialized": "snapshot",
        "persist_docs": {},
        "post-hook": [],
        "pre-hook": [],
        "quoting": {},
        "tags": [],
        "target_database": "some_snapshot_db",
        "target_schema": "some_snapshot_schema",
        "unique_key": "id",
        "strategy": "check",
        "check_cols": "all",
        "on_schema_change": "ignore",
        "on_configuration_change": "apply",
        "meta": {},
        "grants": {},
        "packages": [],
        "docs": {"show": True},
        "contract": {"enforced": False},
    }


@pytest.fixture
def basic_check_snapshot_config_object():
    return SnapshotConfig(
        strategy="check",
        check_cols="all",
        unique_key="id",
        target_database="some_snapshot_db",
        target_schema="some_snapshot_schema",
    )


@pytest.fixture
def complex_set_snapshot_config_dict():
    return {
        "column_types": {"a": "text"},
        "enabled": True,
        "materialized": "snapshot",
        "persist_docs": {},
        "post-hook": [{"sql": 'insert into blah(a, b) select "1", 1', "transaction": True}],
        "pre-hook": [],
        "quoting": {},
        "tags": [],
        "target_database": "some_snapshot_db",
        "target_schema": "some_snapshot_schema",
        "unique_key": "id",
        "extra": "even more",
        "strategy": "check",
        "check_cols": ["a", "b"],
        "on_schema_change": "ignore",
        "on_configuration_change": "apply",
        "meta": {},
        "grants": {},
        "packages": [],
        "docs": {"show": True},
        "contract": {"enforced": False},
    }


@pytest.fixture
def complex_set_snapshot_config_object():
    cfg = SnapshotConfig(
        column_types={"a": "text"},
        materialized="snapshot",
        post_hook=[Hook(sql='insert into blah(a, b) select "1", 1')],
        strategy="check",
        check_cols=["a", "b"],
        target_database="some_snapshot_db",
        target_schema="some_snapshot_schema",
        unique_key="id",
    )
    cfg._extra["extra"] = "even more"
    return cfg


def test_basic_snapshot_config(
    basic_check_snapshot_config_dict, basic_check_snapshot_config_object
):
    cfg_dict = basic_check_snapshot_config_dict
    cfg = basic_check_snapshot_config_object
    assert_symmetric(cfg, cfg_dict, SnapshotConfig)
    pickle.loads(pickle.dumps(cfg))


def test_complex_snapshot_config(
    complex_set_snapshot_config_dict, complex_set_snapshot_config_object
):
    cfg_dict = complex_set_snapshot_config_dict
    cfg = complex_set_snapshot_config_object
    assert_symmetric(cfg, cfg_dict)
    pickle.loads(pickle.dumps(cfg))


def test_invalid_check_wrong_strategy(basic_check_snapshot_config_dict):
    wrong_strategy = basic_check_snapshot_config_dict
    wrong_strategy["strategy"] = "timestamp"
    assert_fails_validation(wrong_strategy, SnapshotConfig)


def test_invalid_missing_check_cols(basic_check_snapshot_config_dict):
    wrong_fields = basic_check_snapshot_config_dict
    del wrong_fields["check_cols"]
    with pytest.raises(ValidationError, match=r"A snapshot configured with the check strategy"):
        SnapshotConfig.validate(wrong_fields)


def test_missing_snapshot_configs(basic_check_snapshot_config_dict):
    wrong_fields = basic_check_snapshot_config_dict
    del wrong_fields["strategy"]
    with pytest.raises(ValidationError, match=r"Snapshots must be configured with a 'strategy'"):
        SnapshotConfig.validate(wrong_fields)

    wrong_fields["strategy"] = "timestamp"
    del wrong_fields["unique_key"]
    with pytest.raises(ValidationError, match=r"Snapshots must be configured with a 'strategy'"):
        SnapshotConfig.validate(wrong_fields)

    wrong_fields["unique_key"] = "id"
    del wrong_fields["target_schema"]
    with pytest.raises(ValidationError, match=r"Snapshots must be configured with a 'strategy'"):
        SnapshotConfig.validate(wrong_fields)


def test_invalid_check_value(basic_check_snapshot_config_dict):
    invalid_check_type = basic_check_snapshot_config_dict
    invalid_check_type["check_cols"] = "some"
    assert_fails_validation(invalid_check_type, SnapshotConfig)


@pytest.fixture
def basic_timestamp_snapshot_dict():
    return {
        "name": "foo",
        "created_at": 1.0,
        "resource_type": str(NodeType.Snapshot),
        "path": "/root/x/path.sql",
        "original_file_path": "/root/path.sql",
        "package_name": "test",
        "language": "sql",
        "raw_code": "select * from wherever",
        "unique_id": "model.test.foo",
        "fqn": ["test", "models", "foo"],
        "refs": [],
        "sources": [],
        "metrics": [],
        "depends_on": {"macros": [], "nodes": [], "public_nodes": []},
        "deferred": False,
        "database": "test_db",
        "description": "",
        "schema": "test_schema",
        "alias": "bar",
        "tags": [],
        "config": {
            "column_types": {},
            "enabled": True,
            "materialized": "snapshot",
            "persist_docs": {},
            "post-hook": [],
            "pre-hook": [],
            "quoting": {},
            "tags": [],
            "target_database": "some_snapshot_db",
            "target_schema": "some_snapshot_schema",
            "unique_key": "id",
            "strategy": "timestamp",
            "updated_at": "last_update",
            "on_schema_change": "ignore",
            "on_configuration_change": "apply",
            "meta": {},
            "grants": {},
            "docs": {"show": True},
            "contract": {"enforced": False},
            "packages": [],
        },
        "docs": {"show": True},
        "contract": {"enforced": False},
        "columns": {},
        "meta": {},
        "checksum": {
            "name": "sha256",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
        "unrendered_config": {
            "strategy": "timestamp",
            "unique_key": "id",
            "updated_at": "last_update",
            "target_database": "some_snapshot_db",
            "target_schema": "some_snapshot_schema",
        },
        "config_call_dict": {},
    }


@pytest.fixture
def basic_timestamp_snapshot_object():
    return SnapshotNode(
        package_name="test",
        path="/root/x/path.sql",
        original_file_path="/root/path.sql",
        language="sql",
        raw_code="select * from wherever",
        name="foo",
        resource_type=NodeType.Snapshot,
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
        config=SnapshotConfig(
            strategy="timestamp",
            unique_key="id",
            updated_at="last_update",
            target_database="some_snapshot_db",
            target_schema="some_snapshot_schema",
        ),
        checksum=FileHash.from_contents(""),
        unrendered_config={
            "strategy": "timestamp",
            "unique_key": "id",
            "updated_at": "last_update",
            "target_database": "some_snapshot_db",
            "target_schema": "some_snapshot_schema",
        },
    )


@pytest.fixture
def basic_intermediate_timestamp_snapshot_object():
    cfg = EmptySnapshotConfig()
    cfg._extra.update(
        {
            "strategy": "timestamp",
            "unique_key": "id",
            "updated_at": "last_update",
            "target_database": "some_snapshot_db",
            "target_schema": "some_snapshot_schema",
        }
    )

    return IntermediateSnapshotNode(
        package_name="test",
        path="/root/x/path.sql",
        original_file_path="/root/path.sql",
        language="sql",
        raw_code="select * from wherever",
        name="foo",
        resource_type=NodeType.Snapshot,
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
        config=cfg,
        checksum=FileHash.from_contents(""),
        created_at=1,
        unrendered_config={
            "strategy": "timestamp",
            "unique_key": "id",
            "updated_at": "last_update",
            "target_database": "some_snapshot_db",
            "target_schema": "some_snapshot_schema",
        },
    )


@pytest.fixture
def basic_check_snapshot_dict():
    return {
        "name": "foo",
        "created_at": 1.0,
        "resource_type": str(NodeType.Snapshot),
        "path": "/root/x/path.sql",
        "original_file_path": "/root/path.sql",
        "package_name": "test",
        "language": "sql",
        "raw_code": "select * from wherever",
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
            "materialized": "snapshot",
            "persist_docs": {},
            "post-hook": [],
            "pre-hook": [],
            "quoting": {},
            "tags": [],
            "target_database": "some_snapshot_db",
            "target_schema": "some_snapshot_schema",
            "unique_key": "id",
            "strategy": "check",
            "check_cols": "all",
            "on_schema_change": "ignore",
            "on_configuration_change": "apply",
            "meta": {},
            "grants": {},
            "docs": {"show": True},
            "contract": {"enforced": False},
            "packages": [],
        },
        "docs": {"show": True},
        "contract": {"enforced": False},
        "columns": {},
        "meta": {},
        "checksum": {
            "name": "sha256",
            "checksum": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        },
        "unrendered_config": {
            "target_database": "some_snapshot_db",
            "target_schema": "some_snapshot_schema",
            "unique_key": "id",
            "strategy": "check",
            "check_cols": "all",
        },
        "config_call_dict": {},
    }


@pytest.fixture
def basic_check_snapshot_object():
    return SnapshotNode(
        package_name="test",
        path="/root/x/path.sql",
        original_file_path="/root/path.sql",
        language="sql",
        raw_code="select * from wherever",
        name="foo",
        resource_type=NodeType.Snapshot,
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
        config=SnapshotConfig(
            strategy="check",
            unique_key="id",
            check_cols="all",
            target_database="some_snapshot_db",
            target_schema="some_snapshot_schema",
        ),
        checksum=FileHash.from_contents(""),
        unrendered_config={
            "target_database": "some_snapshot_db",
            "target_schema": "some_snapshot_schema",
            "unique_key": "id",
            "strategy": "check",
            "check_cols": "all",
        },
    )


@pytest.fixture
def basic_intermediate_check_snapshot_object():
    cfg = EmptySnapshotConfig()
    cfg._extra.update(
        {
            "unique_key": "id",
            "strategy": "check",
            "check_cols": "all",
            "target_database": "some_snapshot_db",
            "target_schema": "some_snapshot_schema",
        }
    )

    return IntermediateSnapshotNode(
        package_name="test",
        path="/root/x/path.sql",
        original_file_path="/root/path.sql",
        language="sql",
        raw_code="select * from wherever",
        name="foo",
        resource_type=NodeType.Snapshot,
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
        config=cfg,
        checksum=FileHash.from_contents(""),
        created_at=1.0,
        unrendered_config={
            "target_database": "some_snapshot_db",
            "target_schema": "some_snapshot_schema",
            "unique_key": "id",
            "strategy": "check",
            "check_cols": "all",
        },
    )


def test_timestamp_snapshot_ok(
    basic_timestamp_snapshot_dict,
    basic_timestamp_snapshot_object,
    basic_intermediate_timestamp_snapshot_object,
):
    node_dict = basic_timestamp_snapshot_dict
    node = basic_timestamp_snapshot_object
    inter = basic_intermediate_timestamp_snapshot_object

    assert_symmetric(node, node_dict, SnapshotNode)
    #   node_from_dict = SnapshotNode.from_dict(inter.to_dict(omit_none=True))
    #   node_from_dict.created_at = 1
    assert SnapshotNode.from_dict(inter.to_dict(omit_none=True)) == node
    assert node.is_refable is True
    assert node.is_ephemeral is False
    pickle.loads(pickle.dumps(node))


def test_check_snapshot_ok(
    basic_check_snapshot_dict,
    basic_check_snapshot_object,
    basic_intermediate_check_snapshot_object,
):
    node_dict = basic_check_snapshot_dict
    node = basic_check_snapshot_object
    inter = basic_intermediate_check_snapshot_object

    assert_symmetric(node, node_dict, SnapshotNode)
    assert SnapshotNode.from_dict(inter.to_dict(omit_none=True)) == node
    assert node.is_refable is True
    assert node.is_ephemeral is False
    pickle.loads(pickle.dumps(node))


def test_invalid_snapshot_bad_resource_type(basic_timestamp_snapshot_dict):
    bad_resource_type = basic_timestamp_snapshot_dict
    bad_resource_type["resource_type"] = str(NodeType.Model)
    assert_fails_validation(bad_resource_type, SnapshotNode)


class TestParsedMacro(ContractTestCase):
    ContractType = Macro

    def _ok_dict(self):
        return {
            "name": "foo",
            "path": "/root/path.sql",
            "original_file_path": "/root/path.sql",
            "created_at": 1.0,
            "package_name": "test",
            "macro_sql": "{% macro foo() %}select 1 as id{% endmacro %}",
            "resource_type": "macro",
            "unique_id": "macro.test.foo",
            "depends_on": {"macros": []},
            "meta": {},
            "description": "my macro description",
            "docs": {"show": True},
            "arguments": [],
        }

    def test_ok(self):
        macro_dict = self._ok_dict()
        macro = self.ContractType(
            name="foo",
            path="/root/path.sql",
            original_file_path="/root/path.sql",
            package_name="test",
            macro_sql="{% macro foo() %}select 1 as id{% endmacro %}",
            resource_type=NodeType.Macro,
            unique_id="macro.test.foo",
            depends_on=MacroDependsOn(),
            meta={},
            description="my macro description",
            arguments=[],
        )
        assert_symmetric(macro, macro_dict)
        pickle.loads(pickle.dumps(macro))

    def test_invalid_missing_unique_id(self):
        bad_missing_uid = self._ok_dict()
        del bad_missing_uid["unique_id"]
        self.assert_fails_validation(bad_missing_uid)

    def test_invalid_extra_field(self):
        bad_extra_field = self._ok_dict()
        bad_extra_field["extra"] = "too many fields"
        self.assert_fails_validation(bad_extra_field)


class TestParsedDocumentation(ContractTestCase):
    ContractType = Documentation

    def _ok_dict(self):
        return {
            "block_contents": "some doc contents",
            "name": "foo",
            "resource_type": "doc",
            "original_file_path": "/root/docs/doc.md",
            "package_name": "test",
            "path": "/root/docs",
            "unique_id": "test.foo",
        }

    def test_ok(self):
        doc_dict = self._ok_dict()
        doc = self.ContractType(
            package_name="test",
            path="/root/docs",
            original_file_path="/root/docs/doc.md",
            name="foo",
            unique_id="test.foo",
            block_contents="some doc contents",
            resource_type=NodeType.Documentation,
        )
        self.assert_symmetric(doc, doc_dict)
        pickle.loads(pickle.dumps(doc))

    def test_invalid_missing(self):
        bad_missing_contents = self._ok_dict()
        del bad_missing_contents["block_contents"]
        self.assert_fails_validation(bad_missing_contents)

    def test_invalid_extra(self):
        bad_extra_field = self._ok_dict()
        bad_extra_field["extra"] = "more"
        self.assert_fails_validation(bad_extra_field)


@pytest.fixture
def minimum_parsed_source_definition_dict():
    return {
        "package_name": "test",
        "path": "/root/models/sources.yml",
        "original_file_path": "/root/models/sources.yml",
        "created_at": 1.0,
        "database": "some_db",
        "schema": "some_schema",
        "fqn": ["test", "source", "my_source", "my_source_table"],
        "source_name": "my_source",
        "name": "my_source_table",
        "source_description": "my source description",
        "loader": "stitch",
        "identifier": "my_source_table",
        "resource_type": str(NodeType.Source),
        "unique_id": "test.source.my_source.my_source_table",
    }


@pytest.fixture
def basic_parsed_source_definition_dict():
    return {
        "package_name": "test",
        "path": "/root/models/sources.yml",
        "original_file_path": "/root/models/sources.yml",
        "created_at": 1.0,
        "database": "some_db",
        "schema": "some_schema",
        "fqn": ["test", "source", "my_source", "my_source_table"],
        "source_name": "my_source",
        "name": "my_source_table",
        "source_description": "my source description",
        "loader": "stitch",
        "identifier": "my_source_table",
        "resource_type": str(NodeType.Source),
        "description": "",
        "columns": {},
        "quoting": {},
        "unique_id": "test.source.my_source.my_source_table",
        "meta": {},
        "source_meta": {},
        "tags": [],
        "config": {
            "enabled": True,
        },
        "unrendered_config": {},
    }


@pytest.fixture
def basic_parsed_source_definition_object():
    return SourceDefinition(
        columns={},
        database="some_db",
        description="",
        fqn=["test", "source", "my_source", "my_source_table"],
        identifier="my_source_table",
        loader="stitch",
        name="my_source_table",
        original_file_path="/root/models/sources.yml",
        package_name="test",
        path="/root/models/sources.yml",
        quoting=Quoting(),
        resource_type=NodeType.Source,
        schema="some_schema",
        source_description="my source description",
        source_name="my_source",
        unique_id="test.source.my_source.my_source_table",
        tags=[],
        config=SourceConfig(),
    )


@pytest.fixture
def complex_parsed_source_definition_dict():
    return {
        "package_name": "test",
        "path": "/root/models/sources.yml",
        "original_file_path": "/root/models/sources.yml",
        "created_at": 1.0,
        "database": "some_db",
        "schema": "some_schema",
        "fqn": ["test", "source", "my_source", "my_source_table"],
        "source_name": "my_source",
        "name": "my_source_table",
        "source_description": "my source description",
        "loader": "stitch",
        "identifier": "my_source_table",
        "resource_type": str(NodeType.Source),
        "description": "",
        "columns": {},
        "quoting": {},
        "unique_id": "test.source.my_source.my_source_table",
        "meta": {},
        "source_meta": {},
        "tags": ["my_tag"],
        "config": {
            "enabled": True,
        },
        "freshness": {"warn_after": {"period": "hour", "count": 1}, "error_after": {}},
        "loaded_at_field": "loaded_at",
        "unrendered_config": {},
    }


@pytest.fixture
def complex_parsed_source_definition_object():
    return SourceDefinition(
        columns={},
        database="some_db",
        description="",
        fqn=["test", "source", "my_source", "my_source_table"],
        identifier="my_source_table",
        loader="stitch",
        name="my_source_table",
        original_file_path="/root/models/sources.yml",
        package_name="test",
        path="/root/models/sources.yml",
        quoting=Quoting(),
        resource_type=NodeType.Source,
        schema="some_schema",
        source_description="my source description",
        source_name="my_source",
        unique_id="test.source.my_source.my_source_table",
        tags=["my_tag"],
        config=SourceConfig(),
        freshness=FreshnessThreshold(warn_after=Time(period=TimePeriod.hour, count=1)),
        loaded_at_field="loaded_at",
    )


def test_basic_source_definition(
    minimum_parsed_source_definition_dict,
    basic_parsed_source_definition_dict,
    basic_parsed_source_definition_object,
):
    node = basic_parsed_source_definition_object
    node_dict = basic_parsed_source_definition_dict
    minimum = minimum_parsed_source_definition_dict

    assert_symmetric(node, node_dict, SourceDefinition)

    assert node.is_ephemeral is False
    assert node.is_refable is False
    assert node.has_freshness is False

    assert_from_dict(node, minimum, SourceDefinition)
    pickle.loads(pickle.dumps(node))


def test_invalid_missing(minimum_parsed_source_definition_dict):
    bad_missing_name = minimum_parsed_source_definition_dict
    del bad_missing_name["name"]
    assert_fails_validation(bad_missing_name, SourceDefinition)


def test_invalid_bad_resource_type(minimum_parsed_source_definition_dict):
    bad_resource_type = minimum_parsed_source_definition_dict
    bad_resource_type["resource_type"] = str(NodeType.Model)
    assert_fails_validation(bad_resource_type, SourceDefinition)


def test_complex_source_definition(
    complex_parsed_source_definition_dict, complex_parsed_source_definition_object
):
    node = complex_parsed_source_definition_object
    node_dict = complex_parsed_source_definition_dict
    assert_symmetric(node, node_dict, SourceDefinition)

    assert node.is_ephemeral is False
    assert node.is_refable is False
    assert node.has_freshness is True

    pickle.loads(pickle.dumps(node))


def test_source_no_loaded_at(complex_parsed_source_definition_object):
    node = complex_parsed_source_definition_object
    assert node.has_freshness is True
    # no loaded_at_field -> does not have freshness
    node.loaded_at_field = None
    assert node.has_freshness is False


def test_source_no_freshness(complex_parsed_source_definition_object):
    node = complex_parsed_source_definition_object
    assert node.has_freshness is True
    node.freshness = None
    assert node.has_freshness is False


unchanged_source_definitions = [
    lambda u: (u, u.replace(tags=["mytag"])),
    lambda u: (u, u.replace(meta={"a": 1000})),
]

changed_source_definitions = [
    lambda u: (
        u,
        u.replace(
            freshness=FreshnessThreshold(warn_after=Time(period=TimePeriod.hour, count=1)),
            loaded_at_field="loaded_at",
        ),
    ),
    lambda u: (u, u.replace(loaded_at_field="loaded_at")),
    lambda u: (
        u,
        u.replace(freshness=FreshnessThreshold(error_after=Time(period=TimePeriod.hour, count=1))),
    ),
    lambda u: (u, u.replace(quoting=Quoting(identifier=True))),
    lambda u: (u, u.replace(database="other_database")),
    lambda u: (u, u.replace(schema="other_schema")),
    lambda u: (u, u.replace(identifier="identifier")),
]


@pytest.mark.parametrize("func", unchanged_source_definitions)
def test_compare_unchanged_parsed_source_definition(func, basic_parsed_source_definition_object):
    node, compare = func(basic_parsed_source_definition_object)
    assert node.same_contents(compare)


@pytest.mark.parametrize("func", changed_source_definitions)
def test_compare_changed_source_definition(func, basic_parsed_source_definition_object):
    node, compare = func(basic_parsed_source_definition_object)
    assert not node.same_contents(compare)


@pytest.fixture
def minimal_parsed_exposure_dict():
    return {
        "name": "my_exposure",
        "type": "notebook",
        "owner": {
            "email": "test@example.com",
        },
        "fqn": ["test", "exposures", "my_exposure"],
        "unique_id": "exposure.test.my_exposure",
        "package_name": "test",
        "meta": {},
        "tags": [],
        "path": "models/something.yml",
        "original_file_path": "models/something.yml",
        "description": "",
        "created_at": 1.0,
        "resource_type": "exposure",
    }


@pytest.fixture
def basic_parsed_exposure_dict():
    return {
        "name": "my_exposure",
        "type": "notebook",
        "owner": {
            "email": "test@example.com",
        },
        "resource_type": "exposure",
        "depends_on": {
            "nodes": [],
            "macros": [],
            "public_nodes": [],
        },
        "refs": [],
        "sources": [],
        "metrics": [],
        "fqn": ["test", "exposures", "my_exposure"],
        "unique_id": "exposure.test.my_exposure",
        "package_name": "test",
        "path": "models/something.yml",
        "original_file_path": "models/something.yml",
        "description": "",
        "meta": {},
        "tags": [],
        "created_at": 1.0,
        "config": {
            "enabled": True,
        },
        "unrendered_config": {},
    }


@pytest.fixture
def basic_parsed_exposure_object():
    return Exposure(
        name="my_exposure",
        resource_type=NodeType.Exposure,
        type=ExposureType.Notebook,
        fqn=["test", "exposures", "my_exposure"],
        unique_id="exposure.test.my_exposure",
        package_name="test",
        path="models/something.yml",
        original_file_path="models/something.yml",
        owner=Owner(email="test@example.com"),
        description="",
        meta={},
        tags=[],
        config=ExposureConfig(),
        unrendered_config={},
    )


@pytest.fixture
def complex_parsed_exposure_dict():
    return {
        "name": "my_exposure",
        "type": "analysis",
        "created_at": 1.0,
        "owner": {
            "email": "test@example.com",
            "name": "A Name",
        },
        "resource_type": "exposure",
        "maturity": "low",
        "url": "https://example.com/analyses/1",
        "description": "my description",
        "meta": {"tool": "my_tool", "is_something": False},
        "tags": ["my_department"],
        "depends_on": {
            "nodes": ["models.test.my_model"],
            "macros": [],
            "public_nodes": [],
        },
        "refs": [],
        "sources": [],
        "metrics": [],
        "fqn": ["test", "exposures", "my_exposure"],
        "unique_id": "exposure.test.my_exposure",
        "package_name": "test",
        "path": "models/something.yml",
        "original_file_path": "models/something.yml",
        "config": {
            "enabled": True,
        },
        "unrendered_config": {},
    }


@pytest.fixture
def complex_parsed_exposure_object():
    return Exposure(
        name="my_exposure",
        resource_type=NodeType.Exposure,
        type=ExposureType.Analysis,
        owner=Owner(email="test@example.com", name="A Name"),
        maturity=MaturityType.Low,
        url="https://example.com/analyses/1",
        description="my description",
        meta={"tool": "my_tool", "is_something": False},
        tags=["my_department"],
        depends_on=DependsOn(nodes=["models.test.my_model"]),
        fqn=["test", "exposures", "my_exposure"],
        unique_id="exposure.test.my_exposure",
        package_name="test",
        path="models/something.yml",
        original_file_path="models/something.yml",
        config=ExposureConfig(),
        unrendered_config={},
    )


def test_basic_parsed_exposure(
    minimal_parsed_exposure_dict, basic_parsed_exposure_dict, basic_parsed_exposure_object
):
    assert_symmetric(basic_parsed_exposure_object, basic_parsed_exposure_dict, Exposure)
    assert_from_dict(basic_parsed_exposure_object, minimal_parsed_exposure_dict, Exposure)
    pickle.loads(pickle.dumps(basic_parsed_exposure_object))


def test_complex_parsed_exposure(complex_parsed_exposure_dict, complex_parsed_exposure_object):
    assert_symmetric(complex_parsed_exposure_object, complex_parsed_exposure_dict, Exposure)


unchanged_parsed_exposures = [
    lambda u: (u, u),
]


changed_parsed_exposures = [
    lambda u: (u, u.replace(fqn=u.fqn[:-1] + ["something", u.fqn[-1]])),
    lambda u: (u, u.replace(type=ExposureType.ML)),
    lambda u: (u, u.replace(owner=u.owner.replace(name="My Name"))),
    lambda u: (u, u.replace(maturity=MaturityType.Medium)),
    lambda u: (u, u.replace(url="https://example.com/dashboard/1")),
    lambda u: (u, u.replace(description="My description")),
    lambda u: (u, u.replace(depends_on=DependsOn(nodes=["model.test.blah"]))),
]


@pytest.mark.parametrize("func", unchanged_parsed_exposures)
def test_compare_unchanged_parsed_exposure(func, basic_parsed_exposure_object):
    node, compare = func(basic_parsed_exposure_object)
    assert node.same_contents(compare)


@pytest.mark.parametrize("func", changed_parsed_exposures)
def test_compare_changed_exposure(func, basic_parsed_exposure_object):
    node, compare = func(basic_parsed_exposure_object)
    assert not node.same_contents(compare)


# METRICS
@pytest.fixture
def minimal_parsed_metric_dict():
    return {
        "name": "my_metric",
        "type": "count",
        "timestamp": "created_at",
        "time_grains": ["day"],
        "fqn": ["test", "metrics", "my_metric"],
        "unique_id": "metric.test.my_metric",
        "package_name": "test",
        "meta": {},
        "tags": [],
        "path": "models/something.yml",
        "original_file_path": "models/something.yml",
        "description": "",
        "created_at": 1.0,
    }


@pytest.fixture
def basic_parsed_metric_dict():
    return {
        "name": "new_customers",
        "label": "New Customers",
        "model": 'ref("dim_customers")',
        "calculation_method": "count",
        "expression": "user_id",
        "timestamp": "signup_date",
        "time_grains": ["day", "week", "month"],
        "dimensions": ["plan", "country"],
        "filters": [
            {
                "field": "is_paying",
                "value": "true",
                "operator": "=",
            }
        ],
        "resource_type": "metric",
        "refs": [["dim_customers"]],
        "sources": [],
        "metrics": [],
        "fqn": ["test", "metrics", "my_metric"],
        "unique_id": "metric.test.my_metric",
        "package_name": "test",
        "path": "models/something.yml",
        "original_file_path": "models/something.yml",
        "description": "",
        "meta": {},
        "tags": [],
        "created_at": 1.0,
        "depends_on": {
            "nodes": [],
            "macros": [],
            "public_nodes": [],
        },
    }


@pytest.fixture
def basic_parsed_metric_object():
    return Metric(
        name="my_metric",
        resource_type=NodeType.Metric,
        calculation_method="count",
        fqn=["test", "metrics", "my_metric"],
        unique_id="metric.test.my_metric",
        package_name="test",
        path="models/something.yml",
        original_file_path="models/something.yml",
        description="",
        meta={},
        tags=[],
    )
