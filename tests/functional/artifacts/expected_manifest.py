import hashlib
import dbt
import os
from unittest.mock import ANY
from dbt.tests.util import AnyStringWith

# This produces an "expected manifest", with a number of the fields
# modified to avoid ephemeral changes.
#   ANY
#   AnyStringWith
#   LineIndifferent
# It also uses some convenience methods to generate the
# various config dictionaries.


def get_rendered_model_config(**updates):
    result = {
        "database": None,
        "schema": None,
        "alias": None,
        "enabled": True,
        "group": None,
        "materialized": "view",
        "pre-hook": [],
        "post-hook": [],
        "column_types": {},
        "quoting": {},
        "tags": [],
        "persist_docs": {},
        "full_refresh": None,
        "on_schema_change": "ignore",
        "on_configuration_change": "apply",
        "meta": {},
        "unique_key": None,
        "grants": {},
        "packages": [],
        "incremental_strategy": None,
        "docs": {"node_color": None, "show": True},
        "contract": {"enforced": False},
    }
    result.update(updates)
    return result


def get_unrendered_model_config(**updates):
    return updates


def get_rendered_seed_config(**updates):
    result = {
        "enabled": True,
        "group": None,
        "materialized": "seed",
        "persist_docs": {},
        "pre-hook": [],
        "post-hook": [],
        "column_types": {},
        "quoting": {},
        "tags": [],
        "quote_columns": True,
        "full_refresh": None,
        "on_schema_change": "ignore",
        "on_configuration_change": "apply",
        "database": None,
        "schema": None,
        "alias": None,
        "meta": {},
        "unique_key": None,
        "grants": {},
        "packages": [],
        "incremental_strategy": None,
        "docs": {"node_color": None, "show": True},
        "contract": {"enforced": False},
    }
    result.update(updates)
    return result


def get_unrendered_seed_config(**updates):
    result = {"quote_columns": True}
    result.update(updates)
    return result


def get_rendered_snapshot_config(**updates):
    result = {
        "database": None,
        "schema": None,
        "alias": None,
        "enabled": True,
        "group": None,
        "materialized": "snapshot",
        "pre-hook": [],
        "post-hook": [],
        "column_types": {},
        "quoting": {},
        "tags": [],
        "persist_docs": {},
        "full_refresh": None,
        "on_schema_change": "ignore",
        "on_configuration_change": "apply",
        "strategy": "check",
        "check_cols": "all",
        "unique_key": "id",
        "target_database": None,
        "target_schema": None,
        "updated_at": None,
        "meta": {},
        "grants": {},
        "packages": [],
        "incremental_strategy": None,
        "docs": {"node_color": None, "show": True},
        "contract": {"enforced": False},
    }
    result.update(updates)
    return result


def get_unrendered_snapshot_config(**updates):
    result = {"check_cols": "all", "strategy": "check", "target_schema": None, "unique_key": "id"}
    result.update(updates)
    return result


def get_rendered_tst_config(**updates):
    result = {
        "enabled": True,
        "group": None,
        "materialized": "test",
        "tags": [],
        "severity": "ERROR",
        "store_failures": None,
        "warn_if": "!= 0",
        "error_if": "!= 0",
        "fail_calc": "count(*)",
        "where": None,
        "limit": None,
        "database": None,
        "schema": "dbt_test__audit",
        "alias": None,
        "meta": {},
    }
    result.update(updates)
    return result


def get_unrendered_tst_config(**updates):
    result = {}
    result.update(updates)
    return result


def quote(value):
    quote_char = '"'
    return "{0}{1}{0}".format(quote_char, value)


def relation_name_format(quote_database: bool, quote_schema: bool, quote_identifier: bool):
    return ".".join(
        (
            quote("{0}") if quote_database else "{0}",
            quote("{1}") if quote_schema else "{1}",
            quote("{2}") if quote_identifier else "{2}",
        )
    )


def checksum_file(path):
    """windows has silly git behavior that adds newlines, and python does
    silly things if we just open(..., 'r').encode('utf-8').
    """
    with open(path, "rb") as fp:
        # We strip the file contents because we want the checksum to match the stored contents
        hashed = hashlib.sha256(fp.read().strip()).hexdigest()
    return {
        "name": "sha256",
        "checksum": hashed,
    }


def read_file_replace_returns(path):
    with open(path, "r") as fp:
        return fp.read().replace("\r", "").replace("\n", "")


class LineIndifferent:
    def __init__(self, expected):
        self.expected = expected.replace("\r", "")

    def __eq__(self, other):
        got = other.replace("\r", "").replace("\n", "")
        return self.expected == got

    def __repr__(self):
        return "LineIndifferent({!r})".format(self.expected)

    def __str__(self):
        return self.__repr__()


def expected_seeded_manifest(project, model_database=None, quote_model=False):

    model_sql_path = os.path.join("models", "model.sql")
    second_model_sql_path = os.path.join("models", "second_model.sql")
    model_schema_yml_path = os.path.join("models", "schema.yml")
    seed_schema_yml_path = os.path.join("seeds", "schema.yml")
    seed_path = os.path.join("seeds", "seed.csv")
    snapshot_path = os.path.join("snapshots", "snapshot_seed.sql")

    my_schema_name = project.test_schema
    alternate_schema = project.test_schema + "_test"
    test_audit_schema = my_schema_name + "_dbt_test__audit"

    model_database = project.database

    model_config = get_rendered_model_config(docs={"node_color": None, "show": False})
    second_config = get_rendered_model_config(
        schema="test", docs={"node_color": None, "show": False}
    )

    unrendered_model_config = get_unrendered_model_config(
        materialized="view", docs={"show": False}
    )

    unrendered_second_config = get_unrendered_model_config(
        schema="test", materialized="view", docs={"show": False}
    )

    seed_config = get_rendered_seed_config()
    unrendered_seed_config = get_unrendered_seed_config()

    test_config = get_rendered_tst_config()
    unrendered_test_config = get_unrendered_tst_config()

    snapshot_config = get_rendered_snapshot_config(target_schema=alternate_schema)
    unrendered_snapshot_config = get_unrendered_snapshot_config(target_schema=alternate_schema)

    quote_database = quote_schema = True
    relation_name_node_format = relation_name_format(quote_database, quote_schema, quote_model)
    relation_name_source_format = relation_name_format(
        quote_database, quote_schema, quote_identifier=True
    )

    compiled_model_path = os.path.join("target", "compiled", "test", "models")

    model_raw_code = read_file_replace_returns(model_sql_path).rstrip("\r\n")

    return {
        "dbt_schema_version": "https://schemas.getdbt.com/dbt/manifest/v7.json",
        "dbt_version": dbt.version.__version__,
        "nodes": {
            "model.test.model": {
                "compiled_path": os.path.join(compiled_model_path, "model.sql"),
                "build_path": None,
                "created_at": ANY,
                "name": "model",
                "relation_name": relation_name_node_format.format(
                    model_database, my_schema_name, "model"
                ),
                "resource_type": "model",
                "path": "model.sql",
                "original_file_path": model_sql_path,
                "package_name": "test",
                "raw_code": LineIndifferent(model_raw_code),
                "language": "sql",
                "refs": [{"name": "seed", "package": None, "version": None}],
                "sources": [],
                "depends_on": {"nodes": ["seed.test.seed"], "macros": [], "public_nodes": []},
                "deprecation_date": None,
                "unique_id": "model.test.model",
                "fqn": ["test", "model"],
                "metrics": [],
                "tags": [],
                "meta": {},
                "config": model_config,
                "group": None,
                "schema": my_schema_name,
                "database": model_database,
                "deferred": False,
                "alias": "model",
                "description": "The test model",
                "columns": {
                    "id": {
                        "name": "id",
                        "description": "The user ID number",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "first_name": {
                        "name": "first_name",
                        "description": "The user's first name",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "email": {
                        "name": "email",
                        "description": "The user's email",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "ip_address": {
                        "name": "ip_address",
                        "description": "The user's IP address",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "updated_at": {
                        "name": "updated_at",
                        "description": "The last time this user's email was updated",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                },
                "contract": {"checksum": None, "enforced": False},
                "constraints": [],
                "patch_path": "test://" + model_schema_yml_path,
                "docs": {"node_color": None, "show": False},
                "compiled": True,
                "compiled_code": ANY,
                "extra_ctes_injected": True,
                "extra_ctes": [],
                "checksum": checksum_file(model_sql_path),
                "unrendered_config": unrendered_model_config,
                "access": "protected",
                "version": None,
                "latest_version": None,
            },
            "model.test.second_model": {
                "compiled_path": os.path.join(compiled_model_path, "second_model.sql"),
                "build_path": None,
                "created_at": ANY,
                "name": "second_model",
                "relation_name": relation_name_node_format.format(
                    project.database, alternate_schema, "second_model"
                ),
                "resource_type": "model",
                "path": "second_model.sql",
                "original_file_path": second_model_sql_path,
                "package_name": "test",
                "raw_code": LineIndifferent(
                    read_file_replace_returns(second_model_sql_path).rstrip("\r\n")
                ),
                "language": "sql",
                "refs": [{"name": "seed", "package": None, "version": None}],
                "sources": [],
                "depends_on": {"nodes": ["seed.test.seed"], "macros": [], "public_nodes": []},
                "deprecation_date": None,
                "unique_id": "model.test.second_model",
                "fqn": ["test", "second_model"],
                "metrics": [],
                "tags": [],
                "meta": {},
                "config": second_config,
                "group": None,
                "schema": alternate_schema,
                "database": project.database,
                "deferred": False,
                "alias": "second_model",
                "description": "The second test model",
                "columns": {
                    "id": {
                        "name": "id",
                        "description": "The user ID number",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "first_name": {
                        "name": "first_name",
                        "description": "The user's first name",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "email": {
                        "name": "email",
                        "description": "The user's email",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "ip_address": {
                        "name": "ip_address",
                        "description": "The user's IP address",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "updated_at": {
                        "name": "updated_at",
                        "description": "The last time this user's email was updated",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                },
                "contract": {"checksum": None, "enforced": False},
                "constraints": [],
                "patch_path": "test://" + model_schema_yml_path,
                "docs": {"node_color": None, "show": False},
                "compiled": True,
                "compiled_code": ANY,
                "extra_ctes_injected": True,
                "extra_ctes": [],
                "checksum": checksum_file(second_model_sql_path),
                "unrendered_config": unrendered_second_config,
                "access": "protected",
                "version": None,
                "latest_version": None,
            },
            "seed.test.seed": {
                "build_path": None,
                "created_at": ANY,
                "config": seed_config,
                "group": None,
                "patch_path": "test://" + seed_schema_yml_path,
                "path": "seed.csv",
                "name": "seed",
                "root_path": project.project_root,
                "resource_type": "seed",
                "raw_code": "",
                "package_name": "test",
                "original_file_path": seed_path,
                "unique_id": "seed.test.seed",
                "fqn": ["test", "seed"],
                "tags": [],
                "meta": {},
                "depends_on": {"macros": []},
                "schema": my_schema_name,
                "database": project.database,
                "alias": "seed",
                "deferred": False,
                "description": "The test seed",
                "columns": {
                    "id": {
                        "name": "id",
                        "description": "The user ID number",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "first_name": {
                        "name": "first_name",
                        "description": "The user's first name",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "email": {
                        "name": "email",
                        "description": "The user's email",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "ip_address": {
                        "name": "ip_address",
                        "description": "The user's IP address",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "updated_at": {
                        "name": "updated_at",
                        "description": "The last time this user's email was updated",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                },
                "docs": {"node_color": None, "show": True},
                "checksum": checksum_file(seed_path),
                "unrendered_config": unrendered_seed_config,
                "relation_name": relation_name_node_format.format(
                    project.database, my_schema_name, "seed"
                ),
            },
            "test.test.not_null_model_id.d01cc630e6": {
                "alias": "not_null_model_id",
                "attached_node": "model.test.model",
                "compiled_path": os.path.join(
                    compiled_model_path, "schema.yml", "not_null_model_id.sql"
                ),
                "build_path": None,
                "created_at": ANY,
                "column_name": "id",
                "columns": {},
                "config": test_config,
                "sources": [],
                "group": None,
                "depends_on": {
                    "macros": ["macro.dbt.test_not_null", "macro.dbt.get_where_subquery"],
                    "nodes": ["model.test.model"],
                    "public_nodes": [],
                },
                "deferred": False,
                "description": "",
                "file_key_name": "models.model",
                "fqn": ["test", "not_null_model_id"],
                "metrics": [],
                "name": "not_null_model_id",
                "original_file_path": model_schema_yml_path,
                "package_name": "test",
                "patch_path": None,
                "path": "not_null_model_id.sql",
                "raw_code": "{{ test_not_null(**_dbt_generic_test_kwargs) }}",
                "language": "sql",
                "refs": [{"name": "model", "package": None, "version": None}],
                "relation_name": None,
                "resource_type": "test",
                "schema": test_audit_schema,
                "database": project.database,
                "tags": [],
                "meta": {},
                "unique_id": "test.test.not_null_model_id.d01cc630e6",
                "docs": {"node_color": None, "show": True},
                "compiled": True,
                "compiled_code": AnyStringWith("where id is null"),
                "extra_ctes_injected": True,
                "extra_ctes": [],
                "test_metadata": {
                    "namespace": None,
                    "name": "not_null",
                    "kwargs": {
                        "column_name": "id",
                        "model": "{{ get_where_subquery(ref('model')) }}",
                    },
                },
                "checksum": {"name": "none", "checksum": ""},
                "unrendered_config": unrendered_test_config,
                "contract": {"checksum": None, "enforced": False},
            },
            "snapshot.test.snapshot_seed": {
                "alias": "snapshot_seed",
                "compiled_path": None,
                "build_path": None,
                "created_at": ANY,
                "checksum": checksum_file(snapshot_path),
                "columns": {},
                "compiled": True,
                "compiled_code": ANY,
                "config": snapshot_config,
                "contract": {"checksum": None, "enforced": False},
                "database": project.database,
                "group": None,
                "deferred": False,
                "depends_on": {
                    "macros": [],
                    "nodes": ["seed.test.seed"],
                    "public_nodes": [],
                },
                "description": "",
                "docs": {"node_color": None, "show": True},
                "extra_ctes": [],
                "extra_ctes_injected": True,
                "fqn": ["test", "snapshot_seed", "snapshot_seed"],
                "metrics": [],
                "meta": {},
                "name": "snapshot_seed",
                "original_file_path": snapshot_path,
                "package_name": "test",
                "patch_path": None,
                "path": "snapshot_seed.sql",
                "raw_code": LineIndifferent(
                    read_file_replace_returns(snapshot_path)
                    .replace("{% snapshot snapshot_seed %}", "")
                    .replace("{% endsnapshot %}", "")
                ),
                "language": "sql",
                "refs": [{"name": "seed", "package": None, "version": None}],
                "relation_name": relation_name_node_format.format(
                    project.database, alternate_schema, "snapshot_seed"
                ),
                "resource_type": "snapshot",
                "schema": alternate_schema,
                "sources": [],
                "tags": [],
                "unique_id": "snapshot.test.snapshot_seed",
                "unrendered_config": unrendered_snapshot_config,
            },
            "test.test.test_nothing_model_.5d38568946": {
                "alias": "test_nothing_model_",
                "attached_node": "model.test.model",
                "compiled_path": os.path.join(
                    compiled_model_path, "schema.yml", "test_nothing_model_.sql"
                ),
                "build_path": None,
                "created_at": ANY,
                "column_name": None,
                "columns": {},
                "config": test_config,
                "group": None,
                "contract": {"checksum": None, "enforced": False},
                "sources": [],
                "depends_on": {
                    "macros": ["macro.test.test_nothing", "macro.dbt.get_where_subquery"],
                    "nodes": ["model.test.model"],
                    "public_nodes": [],
                },
                "deferred": False,
                "description": "",
                "file_key_name": "models.model",
                "fqn": ["test", "test_nothing_model_"],
                "metrics": [],
                "name": "test_nothing_model_",
                "original_file_path": model_schema_yml_path,
                "package_name": "test",
                "patch_path": None,
                "path": "test_nothing_model_.sql",
                "raw_code": "{{ test.test_nothing(**_dbt_generic_test_kwargs) }}",
                "language": "sql",
                "refs": [{"name": "model", "package": None, "version": None}],
                "relation_name": None,
                "resource_type": "test",
                "schema": test_audit_schema,
                "database": project.database,
                "tags": [],
                "meta": {},
                "unique_id": "test.test.test_nothing_model_.5d38568946",
                "docs": {"node_color": None, "show": True},
                "compiled": True,
                "compiled_code": AnyStringWith("select 0"),
                "extra_ctes_injected": True,
                "extra_ctes": [],
                "test_metadata": {
                    "namespace": "test",
                    "name": "nothing",
                    "kwargs": {
                        "model": "{{ get_where_subquery(ref('model')) }}",
                    },
                },
                "checksum": {"name": "none", "checksum": ""},
                "unrendered_config": unrendered_test_config,
            },
            "test.test.unique_model_id.67b76558ff": {
                "alias": "unique_model_id",
                "attached_node": "model.test.model",
                "compiled_path": os.path.join(
                    compiled_model_path, "schema.yml", "unique_model_id.sql"
                ),
                "build_path": None,
                "created_at": ANY,
                "column_name": "id",
                "columns": {},
                "config": test_config,
                "group": None,
                "contract": {"checksum": None, "enforced": False},
                "sources": [],
                "depends_on": {
                    "macros": ["macro.dbt.test_unique", "macro.dbt.get_where_subquery"],
                    "nodes": ["model.test.model"],
                    "public_nodes": [],
                },
                "deferred": False,
                "description": "",
                "file_key_name": "models.model",
                "fqn": ["test", "unique_model_id"],
                "metrics": [],
                "name": "unique_model_id",
                "original_file_path": model_schema_yml_path,
                "package_name": "test",
                "patch_path": None,
                "path": "unique_model_id.sql",
                "raw_code": "{{ test_unique(**_dbt_generic_test_kwargs) }}",
                "language": "sql",
                "refs": [{"name": "model", "package": None, "version": None}],
                "relation_name": None,
                "resource_type": "test",
                "schema": test_audit_schema,
                "database": project.database,
                "tags": [],
                "meta": {},
                "unique_id": "test.test.unique_model_id.67b76558ff",
                "docs": {"node_color": None, "show": True},
                "compiled": True,
                "compiled_code": AnyStringWith("count(*)"),
                "extra_ctes_injected": True,
                "extra_ctes": [],
                "test_metadata": {
                    "namespace": None,
                    "name": "unique",
                    "kwargs": {
                        "column_name": "id",
                        "model": "{{ get_where_subquery(ref('model')) }}",
                    },
                },
                "checksum": {"name": "none", "checksum": ""},
                "unrendered_config": unrendered_test_config,
            },
        },
        "sources": {
            "source.test.my_source.my_table": {
                "created_at": ANY,
                "columns": {
                    "id": {
                        "description": "An ID field",
                        "name": "id",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    }
                },
                "config": {
                    "enabled": True,
                },
                "quoting": {
                    "database": None,
                    "schema": None,
                    "identifier": True,
                    "column": None,
                },
                "database": project.database,
                "description": "My table",
                "external": None,
                "freshness": {
                    "error_after": {"count": None, "period": None},
                    "warn_after": {"count": None, "period": None},
                    "filter": None,
                },
                "identifier": "seed",
                "loaded_at_field": None,
                "loader": "a_loader",
                "meta": {},
                "name": "my_table",
                "original_file_path": os.path.join("models", "schema.yml"),
                "package_name": "test",
                "path": os.path.join("models", "schema.yml"),
                "patch_path": None,
                "relation_name": relation_name_source_format.format(
                    project.database, my_schema_name, "seed"
                ),
                "resource_type": "source",
                "schema": my_schema_name,
                "source_description": "My source",
                "source_name": "my_source",
                "source_meta": {},
                "tags": [],
                "unique_id": "source.test.my_source.my_table",
                "fqn": ["test", "my_source", "my_table"],
                "unrendered_config": {},
            },
        },
        "exposures": {
            "exposure.test.notebook_exposure": {
                "created_at": ANY,
                "depends_on": {
                    "macros": [],
                    "nodes": ["model.test.model", "model.test.second_model"],
                    "public_nodes": [],
                },
                "description": "A description of the complex exposure\n",
                "label": None,
                "config": {
                    "enabled": True,
                },
                "fqn": ["test", "notebook_exposure"],
                "maturity": "medium",
                "meta": {"tool": "my_tool", "languages": ["python"]},
                "metrics": [],
                "tags": ["my_department"],
                "name": "notebook_exposure",
                "original_file_path": os.path.join("models", "schema.yml"),
                "owner": {"email": "something@example.com", "name": "Some name"},
                "package_name": "test",
                "path": "schema.yml",
                "refs": [
                    {"name": "model", "package": None, "version": None},
                    {"name": "second_model", "package": None, "version": None},
                ],
                "resource_type": "exposure",
                "sources": [],
                "type": "notebook",
                "unique_id": "exposure.test.notebook_exposure",
                "url": "http://example.com/notebook/1",
                "unrendered_config": {},
            },
            "exposure.test.simple_exposure": {
                "created_at": ANY,
                "depends_on": {
                    "macros": [],
                    "nodes": ["source.test.my_source.my_table", "model.test.model"],
                    "public_nodes": [],
                },
                "description": "",
                "label": None,
                "config": {
                    "enabled": True,
                },
                "fqn": ["test", "simple_exposure"],
                "metrics": [],
                "name": "simple_exposure",
                "original_file_path": os.path.join("models", "schema.yml"),
                "owner": {
                    "email": "something@example.com",
                    "name": None,
                },
                "package_name": "test",
                "path": "schema.yml",
                "refs": [{"name": "model", "package": None, "version": None}],
                "resource_type": "exposure",
                "sources": [["my_source", "my_table"]],
                "type": "dashboard",
                "unique_id": "exposure.test.simple_exposure",
                "url": None,
                "maturity": None,
                "meta": {},
                "tags": [],
                "unrendered_config": {},
            },
        },
        "metrics": {},
        "groups": {},
        "selectors": {},
        "parent_map": {
            "model.test.model": ["seed.test.seed"],
            "model.test.second_model": ["seed.test.seed"],
            "exposure.test.notebook_exposure": ["model.test.model", "model.test.second_model"],
            "exposure.test.simple_exposure": [
                "model.test.model",
                "source.test.my_source.my_table",
            ],
            "seed.test.seed": [],
            "snapshot.test.snapshot_seed": ["seed.test.seed"],
            "source.test.my_source.my_table": [],
            "test.test.not_null_model_id.d01cc630e6": ["model.test.model"],
            "test.test.test_nothing_model_.5d38568946": ["model.test.model"],
            "test.test.unique_model_id.67b76558ff": ["model.test.model"],
        },
        "child_map": {
            "model.test.model": [
                "exposure.test.notebook_exposure",
                "exposure.test.simple_exposure",
                "test.test.not_null_model_id.d01cc630e6",
                "test.test.test_nothing_model_.5d38568946",
                "test.test.unique_model_id.67b76558ff",
            ],
            "model.test.second_model": ["exposure.test.notebook_exposure"],
            "exposure.test.notebook_exposure": [],
            "exposure.test.simple_exposure": [],
            "seed.test.seed": [
                "model.test.model",
                "model.test.second_model",
                "snapshot.test.snapshot_seed",
            ],
            "snapshot.test.snapshot_seed": [],
            "source.test.my_source.my_table": ["exposure.test.simple_exposure"],
            "test.test.not_null_model_id.d01cc630e6": [],
            "test.test.test_nothing_model_.5d38568946": [],
            "test.test.unique_model_id.67b76558ff": [],
        },
        "group_map": {},
        "docs": {
            "doc.dbt.__overview__": ANY,
            "doc.test.macro_info": ANY,
            "doc.test.macro_arg_info": ANY,
        },
        "disabled": {},
        "public_nodes": {},
    }


def expected_references_manifest(project):
    model_database = project.database
    my_schema_name = project.test_schema
    docs_path = os.path.join("models", "docs.md")
    ephemeral_copy_path = os.path.join("models", "ephemeral_copy.sql")
    ephemeral_summary_path = os.path.join("models", "ephemeral_summary.sql")
    view_summary_path = os.path.join("models", "view_summary.sql")
    seed_path = os.path.join("seeds", "seed.csv")
    snapshot_path = os.path.join("snapshots", "snapshot_seed.sql")
    compiled_model_path = os.path.join("target", "compiled", "test", "models")
    schema_yml_path = os.path.join("models", "schema.yml")

    ephemeral_copy_sql = read_file_replace_returns(ephemeral_copy_path).rstrip("\r\n")
    ephemeral_summary_sql = read_file_replace_returns(ephemeral_summary_path).rstrip("\r\n")
    view_summary_sql = read_file_replace_returns(view_summary_path).rstrip("\r\n")
    alternate_schema = project.test_schema + "_test"

    return {
        "dbt_schema_version": "https://schemas.getdbt.com/dbt/manifest/v7.json",
        "dbt_version": dbt.version.__version__,
        "nodes": {
            "model.test.ephemeral_copy": {
                "alias": "ephemeral_copy",
                "compiled_path": os.path.join(compiled_model_path, "ephemeral_copy.sql"),
                "build_path": None,
                "created_at": ANY,
                "columns": {},
                "config": get_rendered_model_config(materialized="ephemeral"),
                "sources": [["my_source", "my_table"]],
                "depends_on": {
                    "macros": [],
                    "nodes": ["source.test.my_source.my_table"],
                    "public_nodes": [],
                },
                "deprecation_date": None,
                "deferred": False,
                "description": "",
                "docs": {"node_color": None, "show": True},
                "fqn": ["test", "ephemeral_copy"],
                "group": None,
                "metrics": [],
                "name": "ephemeral_copy",
                "original_file_path": ephemeral_copy_path,
                "package_name": "test",
                "patch_path": None,
                "path": "ephemeral_copy.sql",
                "raw_code": LineIndifferent(ephemeral_copy_sql),
                "language": "sql",
                "refs": [],
                "relation_name": None,
                "resource_type": "model",
                "schema": my_schema_name,
                "database": project.database,
                "tags": [],
                "meta": {},
                "unique_id": "model.test.ephemeral_copy",
                "compiled": True,
                "compiled_code": ANY,
                "contract": {"checksum": None, "enforced": False},
                "extra_ctes_injected": True,
                "extra_ctes": [],
                "checksum": checksum_file(ephemeral_copy_path),
                "unrendered_config": get_unrendered_model_config(materialized="ephemeral"),
                "access": "protected",
                "version": None,
                "latest_version": None,
                "constraints": [],
            },
            "model.test.ephemeral_summary": {
                "alias": "ephemeral_summary",
                "compiled_path": os.path.join(compiled_model_path, "ephemeral_summary.sql"),
                "build_path": None,
                "created_at": ANY,
                "columns": {
                    "first_name": {
                        "description": "The first name being summarized",
                        "name": "first_name",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "ct": {
                        "description": "The number of instances of the first name",
                        "name": "ct",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                },
                "config": get_rendered_model_config(materialized="table", group="test_group"),
                "contract": {"checksum": None, "enforced": False},
                "sources": [],
                "depends_on": {
                    "macros": [],
                    "nodes": ["model.test.ephemeral_copy"],
                    "public_nodes": [],
                },
                "deprecation_date": None,
                "deferred": False,
                "description": "A summmary table of the ephemeral copy of the seed data",
                "docs": {"node_color": None, "show": True},
                "fqn": ["test", "ephemeral_summary"],
                "group": "test_group",
                "metrics": [],
                "name": "ephemeral_summary",
                "original_file_path": ephemeral_summary_path,
                "package_name": "test",
                "patch_path": "test://" + os.path.join("models", "schema.yml"),
                "path": "ephemeral_summary.sql",
                "raw_code": LineIndifferent(ephemeral_summary_sql),
                "language": "sql",
                "refs": [{"name": "ephemeral_copy", "package": None, "version": None}],
                "relation_name": '"{0}"."{1}".ephemeral_summary'.format(
                    model_database, my_schema_name
                ),
                "resource_type": "model",
                "schema": my_schema_name,
                "database": project.database,
                "tags": [],
                "meta": {},
                "unique_id": "model.test.ephemeral_summary",
                "compiled": True,
                "compiled_code": ANY,
                "extra_ctes_injected": True,
                "extra_ctes": [ANY],
                "checksum": checksum_file(ephemeral_summary_path),
                "unrendered_config": get_unrendered_model_config(
                    materialized="table", group="test_group"
                ),
                "access": "protected",
                "version": None,
                "latest_version": None,
                "constraints": [],
            },
            "model.test.view_summary": {
                "alias": "view_summary",
                "compiled_path": os.path.join(compiled_model_path, "view_summary.sql"),
                "build_path": None,
                "created_at": ANY,
                "columns": {
                    "first_name": {
                        "description": "The first name being summarized",
                        "name": "first_name",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "ct": {
                        "description": "The number of instances of the first name",
                        "name": "ct",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                },
                "config": get_rendered_model_config(),
                "contract": {"checksum": None, "enforced": False},
                "database": project.database,
                "depends_on": {
                    "macros": [],
                    "nodes": ["model.test.ephemeral_summary"],
                    "public_nodes": [],
                },
                "deprecation_date": None,
                "deferred": False,
                "description": "A view of the summary of the ephemeral copy of the seed data",
                "docs": {"node_color": None, "show": True},
                "fqn": ["test", "view_summary"],
                "group": None,
                "metrics": [],
                "name": "view_summary",
                "original_file_path": view_summary_path,
                "package_name": "test",
                "patch_path": "test://" + schema_yml_path,
                "path": "view_summary.sql",
                "raw_code": LineIndifferent(view_summary_sql),
                "language": "sql",
                "refs": [{"name": "ephemeral_summary", "package": None, "version": None}],
                "relation_name": '"{0}"."{1}".view_summary'.format(model_database, my_schema_name),
                "resource_type": "model",
                "schema": my_schema_name,
                "sources": [],
                "tags": [],
                "meta": {},
                "unique_id": "model.test.view_summary",
                "compiled": True,
                "compiled_code": ANY,
                "extra_ctes_injected": True,
                "extra_ctes": [],
                "checksum": checksum_file(view_summary_path),
                "unrendered_config": get_unrendered_model_config(materialized="view"),
                "access": "protected",
                "version": None,
                "latest_version": None,
                "constraints": [],
            },
            "seed.test.seed": {
                "alias": "seed",
                "build_path": None,
                "created_at": ANY,
                "columns": {
                    "id": {
                        "name": "id",
                        "description": "The user ID number",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "first_name": {
                        "name": "first_name",
                        "description": "The user's first name",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "email": {
                        "name": "email",
                        "description": "The user's email",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "ip_address": {
                        "name": "ip_address",
                        "description": "The user's IP address",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "updated_at": {
                        "name": "updated_at",
                        "description": "The last time this user's email was updated",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                },
                "config": get_rendered_seed_config(),
                "deferred": False,
                "depends_on": {"macros": []},
                "description": "The test seed",
                "docs": {"node_color": None, "show": True},
                "fqn": ["test", "seed"],
                "group": None,
                "name": "seed",
                "original_file_path": seed_path,
                "package_name": "test",
                "patch_path": "test://" + os.path.join("seeds", "schema.yml"),
                "path": "seed.csv",
                "raw_code": "",
                "resource_type": "seed",
                "root_path": project.project_root,
                "schema": my_schema_name,
                "database": project.database,
                "tags": [],
                "meta": {},
                "unique_id": "seed.test.seed",
                "checksum": checksum_file(seed_path),
                "unrendered_config": get_unrendered_seed_config(),
                "relation_name": '"{0}"."{1}".seed'.format(project.database, my_schema_name),
            },
            "snapshot.test.snapshot_seed": {
                "alias": "snapshot_seed",
                "compiled_path": None,
                "build_path": None,
                "created_at": ANY,
                "checksum": checksum_file(snapshot_path),
                "columns": {},
                "compiled": True,
                "compiled_code": ANY,
                "config": get_rendered_snapshot_config(target_schema=alternate_schema),
                "contract": {"checksum": None, "enforced": False},
                "database": model_database,
                "deferred": False,
                "depends_on": {"macros": [], "nodes": ["seed.test.seed"], "public_nodes": []},
                "description": "",
                "docs": {"node_color": None, "show": True},
                "extra_ctes": [],
                "extra_ctes_injected": True,
                "fqn": ["test", "snapshot_seed", "snapshot_seed"],
                "group": None,
                "metrics": [],
                "meta": {},
                "name": "snapshot_seed",
                "original_file_path": snapshot_path,
                "package_name": "test",
                "patch_path": None,
                "path": "snapshot_seed.sql",
                "raw_code": ANY,
                "language": "sql",
                "refs": [{"name": "seed", "package": None, "version": None}],
                "relation_name": '"{0}"."{1}".snapshot_seed'.format(
                    model_database, alternate_schema
                ),
                "resource_type": "snapshot",
                "schema": alternate_schema,
                "sources": [],
                "tags": [],
                "unique_id": "snapshot.test.snapshot_seed",
                "unrendered_config": get_unrendered_snapshot_config(
                    target_schema=alternate_schema
                ),
            },
        },
        "sources": {
            "source.test.my_source.my_table": {
                "columns": {
                    "id": {
                        "description": "An ID field",
                        "name": "id",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    }
                },
                "config": {
                    "enabled": True,
                },
                "quoting": {
                    "database": False,
                    "schema": None,
                    "identifier": True,
                    "column": None,
                },
                "created_at": ANY,
                "database": project.database,
                "description": "My table",
                "external": None,
                "freshness": {
                    "error_after": {"count": None, "period": None},
                    "warn_after": {"count": None, "period": None},
                    "filter": None,
                },
                "identifier": "seed",
                "loaded_at_field": None,
                "loader": "a_loader",
                "meta": {},
                "name": "my_table",
                "original_file_path": os.path.join("models", "schema.yml"),
                "package_name": "test",
                "path": os.path.join("models", "schema.yml"),
                "patch_path": None,
                "relation_name": '{0}."{1}"."seed"'.format(project.database, my_schema_name),
                "resource_type": "source",
                "schema": my_schema_name,
                "source_description": "My source",
                "source_name": "my_source",
                "source_meta": {},
                "tags": [],
                "unique_id": "source.test.my_source.my_table",
                "fqn": ["test", "my_source", "my_table"],
                "unrendered_config": {},
            },
        },
        "exposures": {
            "exposure.test.notebook_exposure": {
                "created_at": ANY,
                "depends_on": {
                    "macros": [],
                    "nodes": ["model.test.view_summary"],
                    "public_nodes": [],
                },
                "description": "A description of the complex exposure",
                "label": None,
                "config": {
                    "enabled": True,
                },
                "fqn": ["test", "notebook_exposure"],
                "maturity": "medium",
                "meta": {"tool": "my_tool", "languages": ["python"]},
                "metrics": [],
                "tags": ["my_department"],
                "name": "notebook_exposure",
                "original_file_path": os.path.join("models", "schema.yml"),
                "owner": {"email": "something@example.com", "name": "Some name"},
                "package_name": "test",
                "path": "schema.yml",
                "refs": [{"name": "view_summary", "package": None, "version": None}],
                "resource_type": "exposure",
                "sources": [],
                "type": "notebook",
                "unique_id": "exposure.test.notebook_exposure",
                "url": "http://example.com/notebook/1",
                "unrendered_config": {},
            },
        },
        "metrics": {},
        "groups": {
            "group.test.test_group": {
                "name": "test_group",
                "resource_type": "group",
                "original_file_path": os.path.join("models", "schema.yml"),
                "owner": {"email": "test_group@test.com", "name": None},
                "package_name": "test",
                "path": "schema.yml",
                "unique_id": "group.test.test_group",
            }
        },
        "selectors": {},
        "docs": {
            "doc.dbt.__overview__": ANY,
            "doc.test.column_info": {
                "block_contents": "An ID field",
                "resource_type": "doc",
                "name": "column_info",
                "original_file_path": docs_path,
                "package_name": "test",
                "path": "docs.md",
                "unique_id": "doc.test.column_info",
            },
            "doc.test.ephemeral_summary": {
                "block_contents": ("A summmary table of the ephemeral copy of the seed data"),
                "resource_type": "doc",
                "name": "ephemeral_summary",
                "original_file_path": docs_path,
                "package_name": "test",
                "path": "docs.md",
                "unique_id": "doc.test.ephemeral_summary",
            },
            "doc.test.source_info": {
                "block_contents": "My source",
                "resource_type": "doc",
                "name": "source_info",
                "original_file_path": docs_path,
                "package_name": "test",
                "path": "docs.md",
                "unique_id": "doc.test.source_info",
            },
            "doc.test.summary_count": {
                "block_contents": "The number of instances of the first name",
                "resource_type": "doc",
                "name": "summary_count",
                "original_file_path": docs_path,
                "package_name": "test",
                "path": "docs.md",
                "unique_id": "doc.test.summary_count",
            },
            "doc.test.summary_first_name": {
                "block_contents": "The first name being summarized",
                "resource_type": "doc",
                "name": "summary_first_name",
                "original_file_path": docs_path,
                "package_name": "test",
                "path": "docs.md",
                "unique_id": "doc.test.summary_first_name",
            },
            "doc.test.table_info": {
                "block_contents": "My table",
                "resource_type": "doc",
                "name": "table_info",
                "original_file_path": docs_path,
                "package_name": "test",
                "path": "docs.md",
                "unique_id": "doc.test.table_info",
            },
            "doc.test.view_summary": {
                "block_contents": ("A view of the summary of the ephemeral copy of the seed data"),
                "resource_type": "doc",
                "name": "view_summary",
                "original_file_path": docs_path,
                "package_name": "test",
                "path": "docs.md",
                "unique_id": "doc.test.view_summary",
            },
            "doc.test.macro_info": {
                "block_contents": "My custom test that I wrote that does nothing",
                "resource_type": "doc",
                "name": "macro_info",
                "original_file_path": os.path.join("macros", "macro.md"),
                "package_name": "test",
                "path": "macro.md",
                "unique_id": "doc.test.macro_info",
            },
            "doc.test.notebook_info": {
                "block_contents": "A description of the complex exposure",
                "resource_type": "doc",
                "name": "notebook_info",
                "original_file_path": docs_path,
                "package_name": "test",
                "path": "docs.md",
                "unique_id": "doc.test.notebook_info",
            },
            "doc.test.macro_arg_info": {
                "block_contents": "The model for my custom test",
                "resource_type": "doc",
                "name": "macro_arg_info",
                "original_file_path": os.path.join("macros", "macro.md"),
                "package_name": "test",
                "path": "macro.md",
                "unique_id": "doc.test.macro_arg_info",
            },
        },
        "public_nodes": {},
        "child_map": {
            "model.test.ephemeral_copy": ["model.test.ephemeral_summary"],
            "exposure.test.notebook_exposure": [],
            "model.test.ephemeral_summary": ["model.test.view_summary"],
            "model.test.view_summary": ["exposure.test.notebook_exposure"],
            "seed.test.seed": ["snapshot.test.snapshot_seed"],
            "snapshot.test.snapshot_seed": [],
            "source.test.my_source.my_table": ["model.test.ephemeral_copy"],
        },
        "parent_map": {
            "model.test.ephemeral_copy": ["source.test.my_source.my_table"],
            "model.test.ephemeral_summary": ["model.test.ephemeral_copy"],
            "model.test.view_summary": ["model.test.ephemeral_summary"],
            "exposure.test.notebook_exposure": ["model.test.view_summary"],
            "seed.test.seed": [],
            "snapshot.test.snapshot_seed": ["seed.test.seed"],
            "source.test.my_source.my_table": [],
        },
        "group_map": {"test_group": ["model.test.ephemeral_summary"]},
        "disabled": {},
        "macros": {
            "macro.test.test_nothing": {
                "name": "test_nothing",
                "depends_on": {"macros": []},
                "created_at": ANY,
                "description": "My custom test that I wrote that does nothing",
                "docs": {"node_color": None, "show": True},
                "macro_sql": AnyStringWith("test nothing"),
                "original_file_path": os.path.join("macros", "dummy_test.sql"),
                "path": os.path.join("macros", "dummy_test.sql"),
                "package_name": "test",
                "meta": {
                    "some_key": 100,
                },
                "patch_path": "test://" + os.path.join("macros", "schema.yml"),
                "resource_type": "macro",
                "unique_id": "macro.test.test_nothing",
                "supported_languages": None,
                "arguments": [
                    {
                        "name": "model",
                        "type": "Relation",
                        "description": "The model for my custom test",
                    },
                ],
            }
        },
    }


def expected_versions_manifest(project):
    model_database = project.database
    my_schema_name = project.test_schema

    versioned_model_v1_path = os.path.join("models", "arbitrary_file_name.sql")
    versioned_model_v2_path = os.path.join("models", "versioned_model_v2.sql")
    ref_versioned_model_path = os.path.join("models", "ref_versioned_model.sql")
    compiled_model_path = os.path.join("target", "compiled", "test", "models")
    schema_yml_path = os.path.join("models", "schema.yml")

    versioned_model_v1_sql = read_file_replace_returns(versioned_model_v1_path).rstrip("\r\n")
    versioned_model_v2_sql = read_file_replace_returns(versioned_model_v2_path).rstrip("\r\n")
    ref_versioned_model_sql = read_file_replace_returns(ref_versioned_model_path).rstrip("\r\n")

    test_config = get_rendered_tst_config()
    unrendered_test_config = get_unrendered_tst_config()
    test_audit_schema = my_schema_name + "_dbt_test__audit"
    model_schema_yml_path = os.path.join("models", "schema.yml")

    return {
        "dbt_schema_version": "https://schemas.getdbt.com/dbt/manifest/v7.json",
        "dbt_version": dbt.version.__version__,
        "nodes": {
            "model.test.versioned_model.v1": {
                "alias": "versioned_model_v1",
                "compiled_path": os.path.join(compiled_model_path, "arbitrary_file_name.sql"),
                "build_path": None,
                "created_at": ANY,
                "columns": {
                    "first_name": {
                        "description": "The first name being summarized",
                        "name": "first_name",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "ct": {
                        "description": "The number of instances of the first name",
                        "name": "ct",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                },
                "config": get_rendered_model_config(
                    materialized="table",
                    group="test_group",
                    meta={"size": "large", "color": "blue"},
                ),
                "constraints": [],
                "sources": [],
                "depends_on": {"macros": [], "nodes": [], "public_nodes": []},
                "deferred": False,
                "description": "A versioned model",
                "deprecation_date": ANY,
                "docs": {"node_color": None, "show": True},
                "fqn": ["test", "versioned_model", "v1"],
                "group": "test_group",
                "metrics": [],
                "name": "versioned_model",
                "original_file_path": versioned_model_v1_path,
                "package_name": "test",
                "patch_path": "test://" + os.path.join("models", "schema.yml"),
                "path": "arbitrary_file_name.sql",
                "raw_code": LineIndifferent(versioned_model_v1_sql),
                "language": "sql",
                "refs": [],
                "relation_name": '"{0}"."{1}".versioned_model_v1'.format(
                    model_database, my_schema_name
                ),
                "resource_type": "model",
                "schema": my_schema_name,
                "database": project.database,
                "tags": [],
                "meta": {"size": "large", "color": "blue"},
                "unique_id": "model.test.versioned_model.v1",
                "compiled": True,
                "compiled_code": ANY,
                "contract": {"checksum": None, "enforced": False},
                "extra_ctes_injected": True,
                "extra_ctes": [],
                "checksum": checksum_file(versioned_model_v1_path),
                "unrendered_config": get_unrendered_model_config(
                    materialized="table",
                    group="test_group",
                    meta={"size": "large", "color": "blue"},
                ),
                "access": "protected",
                "version": 1,
                "latest_version": 2,
            },
            "model.test.versioned_model.v2": {
                "alias": "versioned_model_v2",
                "compiled_path": os.path.join(compiled_model_path, "versioned_model_v2.sql"),
                "build_path": None,
                "created_at": ANY,
                "columns": {
                    "first_name": {
                        "description": "The first name being summarized",
                        "name": "first_name",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                    "extra": {
                        "description": "",
                        "name": "extra",
                        "data_type": None,
                        "meta": {},
                        "quote": None,
                        "tags": [],
                        "constraints": [],
                    },
                },
                "config": get_rendered_model_config(
                    materialized="view", group="test_group", meta={"size": "large", "color": "red"}
                ),
                "constraints": [],
                "contract": {"checksum": None, "enforced": False},
                "sources": [],
                "depends_on": {"macros": [], "nodes": [], "public_nodes": []},
                "deferred": False,
                "description": "A versioned model",
                "deprecation_date": None,
                "docs": {"node_color": None, "show": True},
                "fqn": ["test", "versioned_model", "v2"],
                "group": "test_group",
                "metrics": [],
                "name": "versioned_model",
                "original_file_path": versioned_model_v2_path,
                "package_name": "test",
                "patch_path": "test://" + os.path.join("models", "schema.yml"),
                "path": "versioned_model_v2.sql",
                "raw_code": LineIndifferent(versioned_model_v2_sql),
                "language": "sql",
                "refs": [],
                "relation_name": '"{0}"."{1}".versioned_model_v2'.format(
                    model_database, my_schema_name
                ),
                "resource_type": "model",
                "schema": my_schema_name,
                "database": project.database,
                "tags": [],
                "meta": {"size": "large", "color": "red"},
                "unique_id": "model.test.versioned_model.v2",
                "compiled": True,
                "compiled_code": ANY,
                "extra_ctes_injected": True,
                "extra_ctes": [],
                "checksum": checksum_file(versioned_model_v2_path),
                "unrendered_config": get_unrendered_model_config(
                    materialized="view", group="test_group", meta={"size": "large", "color": "red"}
                ),
                "access": "protected",
                "version": 2,
                "latest_version": 2,
            },
            "model.test.ref_versioned_model": {
                "alias": "ref_versioned_model",
                "compiled_path": os.path.join(compiled_model_path, "ref_versioned_model.sql"),
                "build_path": None,
                "created_at": ANY,
                "columns": {},
                "config": get_rendered_model_config(),
                "constraints": [],
                "contract": {"checksum": None, "enforced": False},
                "database": project.database,
                "depends_on": {
                    "macros": [],
                    "nodes": [
                        "model.test.versioned_model.v2",
                        "model.test.versioned_model.v1",
                    ],
                    "public_nodes": [],
                },
                "deprecation_date": None,
                "deferred": False,
                "description": "",
                "docs": {"node_color": None, "show": True},
                "fqn": ["test", "ref_versioned_model"],
                "group": None,
                "metrics": [],
                "name": "ref_versioned_model",
                "original_file_path": ref_versioned_model_path,
                "package_name": "test",
                "patch_path": "test://" + schema_yml_path,
                "path": "ref_versioned_model.sql",
                "raw_code": LineIndifferent(ref_versioned_model_sql),
                "language": "sql",
                "refs": [
                    {"name": "versioned_model", "package": None, "version": 2},
                    {"name": "versioned_model", "package": None, "version": "2"},
                    {"name": "versioned_model", "package": None, "version": 2},
                    {"name": "versioned_model", "package": None, "version": None},
                    {"name": "versioned_model", "package": None, "version": 1},
                ],
                "relation_name": '"{0}"."{1}".ref_versioned_model'.format(
                    model_database, my_schema_name
                ),
                "resource_type": "model",
                "schema": my_schema_name,
                "sources": [],
                "tags": [],
                "meta": {},
                "unique_id": "model.test.ref_versioned_model",
                "compiled": True,
                "compiled_code": ANY,
                "extra_ctes_injected": True,
                "extra_ctes": [],
                "checksum": checksum_file(ref_versioned_model_path),
                "unrendered_config": get_unrendered_model_config(),
                "access": "protected",
                "version": None,
                "latest_version": None,
            },
            "test.test.unique_versioned_model_v1_first_name.6138195dec": {
                "alias": "unique_versioned_model_v1_first_name",
                "attached_node": "model.test.versioned_model.v1",
                "compiled_path": os.path.join(
                    compiled_model_path, "schema.yml", "unique_versioned_model_v1_first_name.sql"
                ),
                "build_path": None,
                "created_at": ANY,
                "column_name": "first_name",
                "columns": {},
                "config": test_config,
                "group": "test_group",
                "contract": {"checksum": None, "enforced": False},
                "sources": [],
                "depends_on": {
                    "macros": ["macro.dbt.test_unique", "macro.dbt.get_where_subquery"],
                    "nodes": ["model.test.versioned_model.v1"],
                    "public_nodes": [],
                },
                "deferred": False,
                "description": "",
                "file_key_name": "models.versioned_model",
                "fqn": ["test", "unique_versioned_model_v1_first_name"],
                "metrics": [],
                "name": "unique_versioned_model_v1_first_name",
                "original_file_path": model_schema_yml_path,
                "package_name": "test",
                "patch_path": None,
                "path": "unique_versioned_model_v1_first_name.sql",
                "raw_code": "{{ test_unique(**_dbt_generic_test_kwargs) }}",
                "language": "sql",
                "refs": [{"name": "versioned_model", "package": None, "version": 1}],
                "relation_name": None,
                "resource_type": "test",
                "schema": test_audit_schema,
                "database": project.database,
                "tags": [],
                "meta": {},
                "unique_id": "test.test.unique_versioned_model_v1_first_name.6138195dec",
                "docs": {"node_color": None, "show": True},
                "compiled": True,
                "compiled_code": AnyStringWith("count(*)"),
                "extra_ctes_injected": True,
                "extra_ctes": [],
                "test_metadata": {
                    "namespace": None,
                    "name": "unique",
                    "kwargs": {
                        "column_name": "first_name",
                        "model": "{{ get_where_subquery(ref('versioned_model', version='1')) }}",
                    },
                },
                "checksum": {"name": "none", "checksum": ""},
                "unrendered_config": unrendered_test_config,
            },
            "test.test.unique_versioned_model_v1_count.0b4c0b688a": {
                "alias": "unique_versioned_model_v1_count",
                "attached_node": "model.test.versioned_model.v1",
                "compiled_path": os.path.join(
                    compiled_model_path, "schema.yml", "unique_versioned_model_v1_count.sql"
                ),
                "build_path": None,
                "created_at": ANY,
                "column_name": None,
                "columns": {},
                "config": test_config,
                "group": "test_group",
                "contract": {"checksum": None, "enforced": False},
                "sources": [],
                "depends_on": {
                    "macros": ["macro.dbt.test_unique", "macro.dbt.get_where_subquery"],
                    "nodes": ["model.test.versioned_model.v1"],
                    "public_nodes": [],
                },
                "deferred": False,
                "description": "",
                "file_key_name": "models.versioned_model",
                "fqn": ["test", "unique_versioned_model_v1_count"],
                "metrics": [],
                "name": "unique_versioned_model_v1_count",
                "original_file_path": model_schema_yml_path,
                "package_name": "test",
                "patch_path": None,
                "path": "unique_versioned_model_v1_count.sql",
                "raw_code": "{{ test_unique(**_dbt_generic_test_kwargs) }}",
                "language": "sql",
                "refs": [{"name": "versioned_model", "package": None, "version": 1}],
                "relation_name": None,
                "resource_type": "test",
                "schema": test_audit_schema,
                "database": project.database,
                "tags": [],
                "meta": {},
                "unique_id": "test.test.unique_versioned_model_v1_count.0b4c0b688a",
                "docs": {"node_color": None, "show": True},
                "compiled": True,
                "compiled_code": AnyStringWith("count(*)"),
                "extra_ctes_injected": True,
                "extra_ctes": [],
                "test_metadata": {
                    "namespace": None,
                    "name": "unique",
                    "kwargs": {
                        "column_name": "count",
                        "model": "{{ get_where_subquery(ref('versioned_model', version='1')) }}",
                    },
                },
                "checksum": {"name": "none", "checksum": ""},
                "unrendered_config": unrendered_test_config,
            },
            "test.test.unique_versioned_model_v2_first_name.998430d28e": {
                "alias": "unique_versioned_model_v2_first_name",
                "attached_node": "model.test.versioned_model.v2",
                "compiled_path": os.path.join(
                    compiled_model_path, "schema.yml", "unique_versioned_model_v2_first_name.sql"
                ),
                "build_path": None,
                "created_at": ANY,
                "column_name": "first_name",
                "columns": {},
                "config": test_config,
                "group": "test_group",
                "contract": {"checksum": None, "enforced": False},
                "sources": [],
                "depends_on": {
                    "macros": ["macro.dbt.test_unique", "macro.dbt.get_where_subquery"],
                    "nodes": ["model.test.versioned_model.v2"],
                    "public_nodes": [],
                },
                "deferred": False,
                "description": "",
                "file_key_name": "models.versioned_model",
                "fqn": ["test", "unique_versioned_model_v2_first_name"],
                "metrics": [],
                "name": "unique_versioned_model_v2_first_name",
                "original_file_path": model_schema_yml_path,
                "package_name": "test",
                "patch_path": None,
                "path": "unique_versioned_model_v2_first_name.sql",
                "raw_code": "{{ test_unique(**_dbt_generic_test_kwargs) }}",
                "language": "sql",
                "refs": [{"name": "versioned_model", "package": None, "version": 2}],
                "relation_name": None,
                "resource_type": "test",
                "schema": test_audit_schema,
                "database": project.database,
                "tags": [],
                "meta": {},
                "unique_id": "test.test.unique_versioned_model_v2_first_name.998430d28e",
                "docs": {"node_color": None, "show": True},
                "compiled": True,
                "compiled_code": AnyStringWith("count(*)"),
                "extra_ctes_injected": True,
                "extra_ctes": [],
                "test_metadata": {
                    "namespace": None,
                    "name": "unique",
                    "kwargs": {
                        "column_name": "first_name",
                        "model": "{{ get_where_subquery(ref('versioned_model', version='2')) }}",
                    },
                },
                "checksum": {"name": "none", "checksum": ""},
                "unrendered_config": unrendered_test_config,
            },
        },
        "exposures": {
            "exposure.test.notebook_exposure": {
                "created_at": ANY,
                "depends_on": {
                    "macros": [],
                    "nodes": ["model.test.versioned_model.v2"],
                    "public_nodes": [],
                },
                "description": "notebook_info",
                "label": None,
                "config": {
                    "enabled": True,
                },
                "fqn": ["test", "notebook_exposure"],
                "maturity": None,
                "meta": {},
                "metrics": [],
                "tags": [],
                "name": "notebook_exposure",
                "original_file_path": os.path.join("models", "schema.yml"),
                "owner": {"email": "something@example.com", "name": "Some name"},
                "package_name": "test",
                "path": "schema.yml",
                "refs": [{"name": "versioned_model", "package": None, "version": 2}],
                "resource_type": "exposure",
                "sources": [],
                "type": "notebook",
                "unique_id": "exposure.test.notebook_exposure",
                "url": None,
                "unrendered_config": {},
            },
        },
        "metrics": {},
        "groups": {
            "group.test.test_group": {
                "name": "test_group",
                "resource_type": "group",
                "original_file_path": os.path.join("models", "schema.yml"),
                "owner": {"email": "test_group@test.com", "name": None},
                "package_name": "test",
                "path": "schema.yml",
                "unique_id": "group.test.test_group",
            }
        },
        "sources": {},
        "selectors": {},
        "docs": {},
        "child_map": {
            "model.test.versioned_model.v1": [
                "model.test.ref_versioned_model",
                "test.test.unique_versioned_model_v1_count.0b4c0b688a",
                "test.test.unique_versioned_model_v1_first_name.6138195dec",
            ],
            "model.test.versioned_model.v2": [
                "exposure.test.notebook_exposure",
                "model.test.ref_versioned_model",
                "test.test.unique_versioned_model_v2_first_name.998430d28e",
            ],
            "model.test.ref_versioned_model": [],
            "exposure.test.notebook_exposure": [],
            "test.test.unique_versioned_model_v1_first_name.6138195dec": [],
            "test.test.unique_versioned_model_v1_count.0b4c0b688a": [],
            "test.test.unique_versioned_model_v2_first_name.998430d28e": [],
        },
        "parent_map": {
            "model.test.versioned_model.v1": [],
            "model.test.versioned_model.v2": [],
            "model.test.ref_versioned_model": [
                "model.test.versioned_model.v1",
                "model.test.versioned_model.v2",
            ],
            "exposure.test.notebook_exposure": ["model.test.versioned_model.v2"],
            "test.test.unique_versioned_model_v1_first_name.6138195dec": [
                "model.test.versioned_model.v1"
            ],
            "test.test.unique_versioned_model_v1_count.0b4c0b688a": [
                "model.test.versioned_model.v1"
            ],
            "test.test.unique_versioned_model_v2_first_name.998430d28e": [
                "model.test.versioned_model.v2"
            ],
        },
        "group_map": {
            "test_group": [
                "model.test.versioned_model.v1",
                "model.test.versioned_model.v2",
                "test.test.unique_versioned_model_v1_first_name.6138195dec",
                "test.test.unique_versioned_model_v1_count.0b4c0b688a",
                "test.test.unique_versioned_model_v2_first_name.998430d28e",
            ]
        },
        "disabled": {},
        "macros": {},
        "public_nodes": {},
    }
