import os
from datetime import datetime

import pytest
import pytz

from dbt.tests.util import (
    check_relations_equal,
    relation_from_name,
    run_dbt,
    update_config_file,
    write_file,
)
from tests.functional.snapshots.fixtures import (
    macros__test_no_overlaps_sql,
    macros_custom_snapshot__custom_sql,
    models__ref_snapshot_sql,
    models__schema_with_target_schema_yml,
    models__schema_yml,
    seeds__seed_csv,
    seeds__seed_newcol_csv,
    snapshots_pg__snapshot_mod_yml,
    snapshots_pg__snapshot_no_target_schema_sql,
    snapshots_pg__snapshot_sql,
    snapshots_pg__snapshot_yml,
    snapshots_pg__source_snapshot_yml,
    snapshots_pg_custom__snapshot_sql,
    snapshots_pg_custom_namespaced__snapshot_sql,
    sources_pg__source_mod_yml,
    sources_pg__source_yml,
)

snapshots_check_col__snapshot_sql = """
{% snapshot snapshot_actual %}

    {{
        config(
            target_database=var('target_database', database),
            target_schema=schema,
            unique_key='id || ' ~ "'-'" ~ ' || first_name',
            strategy='check',
            check_cols=['email'],
        )
    }}
    select * from {{target.database}}.{{schema}}.seed

{% endsnapshot %}

{# This should be exactly the same #}
{% snapshot snapshot_checkall %}
    {{
        config(
            target_database=var('target_database', database),
            target_schema=schema,
            unique_key='id || ' ~ "'-'" ~ ' || first_name',
            strategy='check',
            check_cols='all',
        )
    }}
    select * from {{target.database}}.{{schema}}.seed
{% endsnapshot %}
"""


snapshots_check_col_noconfig__snapshot_sql = """
{% snapshot snapshot_actual %}
    select * from {{target.database}}.{{schema}}.seed
{% endsnapshot %}

{# This should be exactly the same #}
{% snapshot snapshot_checkall %}
    {{ config(check_cols='all') }}
    select * from {{target.database}}.{{schema}}.seed
{% endsnapshot %}
"""


def snapshot_setup(project, num_snapshot_models=1):
    path = os.path.join(project.test_data_dir, "seed_pg.sql")
    project.run_sql_file(path)
    results = run_dbt(["snapshot"])
    assert len(results) == num_snapshot_models

    run_dbt(["test"])
    check_relations_equal(project.adapter, ["snapshot_actual", "snapshot_expected"])

    path = os.path.join(project.test_data_dir, "invalidate_postgres.sql")
    project.run_sql_file(path)

    path = os.path.join(project.test_data_dir, "update.sql")
    project.run_sql_file(path)

    results = run_dbt(["snapshot"])
    assert len(results) == num_snapshot_models

    run_dbt(["test"])
    check_relations_equal(project.adapter, ["snapshot_actual", "snapshot_expected"])


def ref_setup(project, num_snapshot_models=1):
    path = os.path.join(project.test_data_dir, "seed_pg.sql")
    project.run_sql_file(path)
    results = run_dbt(["snapshot"])
    assert len(results) == num_snapshot_models

    results = run_dbt(["run"])
    assert len(results) == 1


class Basic:
    @pytest.fixture(scope="class")
    def snapshots(self):
        return {"snapshot.sql": snapshots_pg__snapshot_sql}

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": models__schema_yml,
            "ref_snapshot.sql": models__ref_snapshot_sql,
        }

    @pytest.fixture(scope="class")
    def macros(self):
        return {"test_no_overlaps.sql": macros__test_no_overlaps_sql}

    @pytest.fixture(scope="class")
    def seeds(self):
        return {"seed_newcol.csv": seeds__seed_newcol_csv, "seed.csv": seeds__seed_csv}


class TestBasicSnapshot(Basic):
    def test_basic_snapshot(self, project):
        snapshot_setup(project, num_snapshot_models=1)


class TestBasicRef(Basic):
    def test_basic_ref(self, project):
        ref_setup(project, num_snapshot_models=1)


class TestBasicTargetSchemaConfig(Basic):
    @pytest.fixture(scope="class")
    def snapshots(self):
        return {"snapshot.sql": snapshots_pg__snapshot_no_target_schema_sql}

    @pytest.fixture(scope="class")
    def project_config_update(self, unique_schema):
        return {
            "snapshots": {
                "test": {
                    "schema": "alt",
                }
            }
        }

    def test_target_schema(self, project):
        manifest = run_dbt(["parse"])
        assert len(manifest.nodes) == 5
        # ensure that the schema in the snapshot node is the same as target_schema
        snapshot_id = "snapshot.test.snapshot_actual"
        snapshot_node = manifest.nodes[snapshot_id]
        # The schema field be changed by the default "generate_schema_name"
        # to append an underscore plus the configured schema of "alt".
        assert snapshot_node.schema == f"{project.test_schema}_alt"
        assert (
            snapshot_node.relation_name
            == f'"{project.database}"."{project.test_schema}_alt"."snapshot_actual"'
        )
        assert snapshot_node.meta == {"owner": "a_owner"}

        # write out schema.yml file and check again
        write_file(models__schema_with_target_schema_yml, "models", "schema.yml")
        manifest = run_dbt(["parse"])
        snapshot_node = manifest.nodes[snapshot_id]
        assert snapshot_node.schema == "schema_from_schema_yml"


class CustomNamespace:
    @pytest.fixture(scope="class")
    def snapshots(self):
        return {"snapshot.sql": snapshots_pg_custom_namespaced__snapshot_sql}

    @pytest.fixture(scope="class")
    def macros(self):
        return {
            "test_no_overlaps.sql": macros__test_no_overlaps_sql,
            "custom.sql": macros_custom_snapshot__custom_sql,
        }

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": models__schema_yml,
            "ref_snapshot.sql": models__ref_snapshot_sql,
        }

    @pytest.fixture(scope="class")
    def seeds(self):
        return {"seed_newcol.csv": seeds__seed_newcol_csv, "seed.csv": seeds__seed_csv}


class TestBasicCustomNamespace(CustomNamespace):
    def test_custom_namespace_snapshot(self, project):
        snapshot_setup(project, num_snapshot_models=1)


class TestRefCustomNamespace(CustomNamespace):
    def test_custom_namespace_ref(self, project):
        ref_setup(project, num_snapshot_models=1)


class CustomSnapshot:
    @pytest.fixture(scope="class")
    def snapshots(self):
        return {"snapshot.sql": snapshots_pg_custom__snapshot_sql}

    @pytest.fixture(scope="class")
    def macros(self):
        return {
            "test_no_overlaps.sql": macros__test_no_overlaps_sql,
            "custom.sql": macros_custom_snapshot__custom_sql,
        }

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": models__schema_yml,
            "ref_snapshot.sql": models__ref_snapshot_sql,
        }

    @pytest.fixture(scope="class")
    def seeds(self):
        return {"seed_newcol.csv": seeds__seed_newcol_csv, "seed.csv": seeds__seed_csv}


class TestBasicCustomSnapshot(CustomSnapshot):
    def test_custom_snapshot(self, project):
        snapshot_setup(project, num_snapshot_models=1)


class TestRefCustomSnapshot(CustomSnapshot):
    def test_custom_ref(self, project):
        ref_setup(project, num_snapshot_models=1)


class CheckCols:
    @pytest.fixture(scope="class")
    def snapshots(self):
        return {"snapshot.sql": snapshots_check_col__snapshot_sql}

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": models__schema_yml,
            "ref_snapshot.sql": models__ref_snapshot_sql,
        }

    @pytest.fixture(scope="class")
    def macros(self):
        return {"test_no_overlaps.sql": macros__test_no_overlaps_sql}

    @pytest.fixture(scope="class")
    def seeds(self):
        return {"seed_newcol.csv": seeds__seed_newcol_csv, "seed.csv": seeds__seed_csv}


class TestBasicCheckCols(CheckCols):
    def test_basic_snapshot(self, project):
        snapshot_setup(project, num_snapshot_models=2)


class TestRefCheckCols(CheckCols):
    def test_check_cols_ref(self, project):
        ref_setup(project, num_snapshot_models=2)


class ConfiguredCheckCols:
    @pytest.fixture(scope="class")
    def snapshots(self):
        return {"snapshot.sql": snapshots_check_col_noconfig__snapshot_sql}

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": models__schema_yml,
            "ref_snapshot.sql": models__ref_snapshot_sql,
        }

    @pytest.fixture(scope="class")
    def macros(self):
        return {"test_no_overlaps.sql": macros__test_no_overlaps_sql}

    @pytest.fixture(scope="class")
    def seeds(self):
        return {"seed_newcol.csv": seeds__seed_newcol_csv, "seed.csv": seeds__seed_csv}

    @pytest.fixture(scope="class")
    def project_config_update(self):
        snapshot_config = {
            "snapshots": {
                "test": {
                    "target_schema": "{{ target.schema }}",
                    "unique_key": "id || '-' || first_name",
                    "strategy": "check",
                    "check_cols": ["email"],
                }
            }
        }
        return snapshot_config


class TestBasicConfiguredCheckCols(ConfiguredCheckCols):
    def test_configured_snapshot(self, project):
        snapshot_setup(project, num_snapshot_models=2)


class TestRefConfiguredCheckCols(ConfiguredCheckCols):
    def test_configured_ref(self, project):
        ref_setup(project, num_snapshot_models=2)


class UpdatedAtCheckCols:
    @pytest.fixture(scope="class")
    def snapshots(self):
        return {"snapshot.sql": snapshots_check_col_noconfig__snapshot_sql}

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": models__schema_yml,
            "ref_snapshot.sql": models__ref_snapshot_sql,
        }

    @pytest.fixture(scope="class")
    def macros(self):
        return {"test_no_overlaps.sql": macros__test_no_overlaps_sql}

    @pytest.fixture(scope="class")
    def seeds(self):
        return {"seed_newcol.csv": seeds__seed_newcol_csv, "seed.csv": seeds__seed_csv}

    @pytest.fixture(scope="class")
    def project_config_update(self):
        snapshot_config = {
            "snapshots": {
                "test": {
                    "target_schema": "{{ target.schema }}",
                    "unique_key": "id || '-' || first_name",
                    "strategy": "check",
                    "check_cols": "all",
                    "updated_at": "updated_at",
                }
            }
        }
        return snapshot_config


class TestBasicUpdatedAtCheckCols(UpdatedAtCheckCols):
    def test_updated_at_snapshot(self, project):
        snapshot_setup(project, num_snapshot_models=2)

        snapshot_expected_relation = relation_from_name(project.adapter, "snapshot_expected")
        revived_records = project.run_sql(
            """
            select id, updated_at, dbt_valid_from from {}
            """.format(
                snapshot_expected_relation
            ),
            fetch="all",
        )
        for result in revived_records:
            # result is a tuple, the updated_at is second and dbt_valid_from is latest
            assert isinstance(result[1], datetime)
            assert isinstance(result[2], datetime)
            assert result[1].replace(tzinfo=pytz.UTC) == result[2].replace(tzinfo=pytz.UTC)


class TestRefUpdatedAtCheckCols(UpdatedAtCheckCols):
    def test_updated_at_ref(self, project):
        ref_setup(project, num_snapshot_models=2)


class BasicYaml(Basic):
    @pytest.fixture(scope="class")
    def snapshots(self):
        """Overrides the same function in Basic to use the YAML method of
        defining a snapshot."""
        return {
            "snapshot.yml": snapshots_pg__snapshot_yml,
            "source_snapshot.yml": snapshots_pg__source_snapshot_yml,
        }

    @pytest.fixture(scope="class")
    def models(self):
        """Overrides the same function in Basic to use a modified version of
        schema.yml without snapshot config."""
        return {
            "sources.yml": sources_pg__source_yml,
            "ref_snapshot.sql": models__ref_snapshot_sql,
        }


class TestBasicSnapshotYaml(BasicYaml):
    def test_basic_snapshot_yaml(self, project):
        snapshot_setup(project, num_snapshot_models=2)


class TestYamlSnapshotCompiledPath(BasicYaml):
    def test_compiled_path(self, project):
        """Verify yml-based snapshots get correct compiled paths without doubling."""
        manifest = run_dbt(["parse"])
        node = manifest.nodes["snapshot.test.snapshot_actual"]
        assert node.path == os.path.join("snapshot.yml", "snapshot_actual.sql")
        assert node.original_file_path == os.path.join("snapshots", "snapshot.yml")

        run_dbt(["compile"])
        expected = os.path.join(
            project.project_root,
            "target",
            "compiled",
            "test",
            "snapshots",
            "snapshot.yml",
            "snapshot_actual.sql",
        )
        assert os.path.exists(expected), f"Compiled file not found: {expected}"


snapshot_name_matches_file__sql = """
{% snapshot snapshot_collide %}
    {{ config(target_schema=schema, unique_key='id', strategy='check', check_cols='all') }}
    select 1 as id, 'hello' as name
{% endsnapshot %}

{% snapshot snapshot_other %}
    {{ config(target_schema=schema, unique_key='id', strategy='check', check_cols='all') }}
    select 2 as id, 'world' as name
{% endsnapshot %}
"""


class TestMultiSnapshotNameMatchesFile:
    """Regression test: when a file contains multiple snapshots and one snapshot's
    name matches the filename, compiled output must not conflict (EISDIR)."""

    @pytest.fixture(scope="class")
    def snapshots(self):
        # File is named "snapshot_collide.sql" and contains a snapshot also named
        # "snapshot_collide" plus a second snapshot "snapshot_other".
        return {"snapshot_collide.sql": snapshot_name_matches_file__sql}

    def test_compile_no_eisdir(self, project):
        run_dbt(["compile"])
        # Both snapshots should nest under the source filename
        for snap_name in ("snapshot_collide", "snapshot_other"):
            compiled = os.path.join(
                project.project_root,
                "target",
                "compiled",
                "test",
                "snapshots",
                "snapshot_collide.sql",
                f"{snap_name}.sql",
            )
            assert os.path.exists(compiled), f"Compiled file not found: {compiled}"


subdir_snapshot__sql = """
{% snapshot subdir_snap %}
    {{ config(target_schema=schema, unique_key='id', strategy='check', check_cols='all') }}
    select 1 as id, 'alice' as name
{% endsnapshot %}
"""


class TestSnapshotNodePathIncludesSubdir:
    """Regression test: node.path for a snapshot in a subdirectory must include
    the subdirectory (e.g. 'subdir/subdir_snap.sql'), not just the filename.
    See https://github.com/dbt-labs/dbt-core/issues/12783."""

    @pytest.fixture(scope="class")
    def snapshots(self):
        return {"subdir": {"subdir_snap.sql": subdir_snapshot__sql}}

    def test_node_path_includes_subdir(self, project):
        manifest = run_dbt(["parse"])
        node = manifest.nodes["snapshot.test.subdir_snap"]
        assert node.path == os.path.join(
            "subdir", "subdir_snap.sql"
        ), f"Expected node.path to include subdirectory, got: {node.path!r}"
        assert node.original_file_path == os.path.join("snapshots", "subdir", "subdir_snap.sql")


subdir_model__sql = """
select 1 as id, 'alice' as name
"""

subdir_snapshot__yml = """
version: 2
snapshots:
  - name: subdir_snap
    relation: ref('subdir_model')
    config:
      strategy: check
      unique_key: id
      check_cols: all
      hard_deletes: new_record"""


class TestYamlSnapshotNodePathIncludesSubdir:
    """Regression test: node.path for a snapshot in a subdirectory must include
    the subdirectory and original filename (e.g. 'subdir/subdir_snap.yml/subdir_snap.sql'),
    not just the filename.
    See https://github.com/dbt-labs/dbt-core/issues/15753."""

    @pytest.fixture(scope="class")
    def models(self):
        return {"subdir": {"subdir_model.sql": subdir_model__sql}}

    @pytest.fixture(scope="class")
    def snapshots(self):
        return {"subdir": {"subdir_snap.yml": subdir_snapshot__yml}}

    def test_node_path_includes_subdir(self, project):
        manifest = run_dbt(["parse"])
        node = manifest.nodes["snapshot.test.subdir_snap"]
        assert node.path == os.path.join(
            "subdir", "subdir_snap.yml", "subdir_snap.sql"
        ), f"Expected node.path to include subdirectory, got: {node.path!r}"
        assert node.original_file_path == os.path.join("snapshots", "subdir", "subdir_snap.yml")


class TestYamlSnapshotPartialParsing(BasicYaml):
    def test_snapshot_partial_parsing(self, project):
        manifest = run_dbt(["parse"])
        snapshot_id = "snapshot.test.snapshot_actual"
        assert snapshot_id in manifest.nodes
        snapshot = manifest.nodes[snapshot_id]
        assert snapshot.meta["owner"] == "a_owner"

        # change an unrelated source file
        write_file(sources_pg__source_mod_yml, "models", "sources.yml")
        manifest = run_dbt(["parse"])
        snapshot = manifest.nodes[snapshot_id]
        assert snapshot.meta["owner"] == "a_owner"

        # change snapshot yml file and re-parse
        write_file(snapshots_pg__snapshot_mod_yml, "snapshots", "snapshot.yml")
        manifest = run_dbt(["parse"])
        snapshot = manifest.nodes[snapshot_id]
        assert snapshot.meta["owner"] == "b_owner"

        # modify dbt_project.yml and re-parse
        config_updates = {
            "snapshots": {
                "test": {
                    "+snapshot_meta_column_names": {
                        "dbt_valid_to": "test_valid_to",
                        "dbt_valid_from": "test_valid_from",
                    },
                }
            }
        }
        update_config_file(config_updates, "dbt_project.yml")
        manifest = run_dbt(["parse"])
        snapshot = manifest.nodes[snapshot_id]
        assert snapshot.config.snapshot_meta_column_names.dbt_valid_to == "test_valid_to"
