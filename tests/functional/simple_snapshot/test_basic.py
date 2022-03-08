import os
from datetime import datetime
import pytz
import pytest
from dbt.tests.util import run_dbt
from dbt.tests.tables import TableComparison
from tests.functional.simple_snapshot.fixtures import (  # noqa: F401
    models,
    seeds,
    macros,
    snapshots_pg,
    macros_custom_snapshot,
    snapshots_pg_custom_namespaced,
    snapshots_pg_custom,
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


class OverRideTabelComparison_dbt(TableComparison):
    def _assert_tables_equal_sql(self, relation_a, relation_b, columns=None):
        # When building the equality tests, only test columns that don't start
        # with 'dbt_', because those are time-sensitive
        if columns is None:
            columns = [
                c
                for c in self.get_relation_columns(relation_a)
                if not c[0].lower().startswith("dbt_")
            ]
        return super()._assert_tables_equal_sql(relation_a, relation_b, columns=columns)


class RevivedTableComparison(TableComparison):
    def _assert_tables_equal_sql(self, relation_a, relation_b, columns=None):
        revived_records = self.run_sql(
            """
            select
                id,
                updated_at,
                dbt_valid_from
            from {}
            """.format(
                relation_b
            ),
            fetch="all",
        )

        for result in revived_records:
            # result is a tuple, the updated_at is second and dbt_valid_from is latest
            assert isinstance(result[1], datetime)
            assert isinstance(result[2], datetime)
            assert result[1].replace(tzinfo=pytz.UTC) == result[2].replace(tzinfo=pytz.UTC)

        if columns is None:
            columns = [
                c
                for c in self.get_relation_columns(relation_a)
                if not c[0].lower().startswith("dbt_")
            ]
        return super()._assert_tables_equal_sql(relation_a, relation_b, columns=columns)


def snapshot_setup(project, NUM_SNAPSHOT_MODELS, table_comp):
    path = os.path.join(project.test_data_dir, "seed_pg.sql")
    project.run_sql_file(path)
    results = run_dbt(["snapshot"])
    assert len(results) == NUM_SNAPSHOT_MODELS

    run_dbt(["test"])
    table_comp.assert_tables_equal("snapshot_actual", "snapshot_expected")

    path = os.path.join(project.test_data_dir, "invalidate_postgres.sql")
    project.run_sql_file(path)

    path = os.path.join(project.test_data_dir, "update.sql")
    project.run_sql_file(path)

    results = run_dbt(["snapshot"])
    assert len(results) == NUM_SNAPSHOT_MODELS

    run_dbt(["test"])
    table_comp.assert_tables_equal("snapshot_actual", "snapshot_expected")


def ref_setup(project, NUM_SNAPSHOT_MODELS):

    path = os.path.join(project.test_data_dir, "seed_pg.sql")
    project.run_sql_file(path)
    results = run_dbt(["snapshot"])
    assert len(results) == NUM_SNAPSHOT_MODELS

    results = run_dbt(["run"])
    assert len(results) == 1


# these fixtures are slight variations of each other for the basic snapshot tests run
@pytest.fixture
def basic_snapshot(project):
    NUM_SNAPSHOT_MODELS = 1
    table_comp = TableComparison(
        adapter=project.adapter, unique_schema=project.test_schema, database=project.database
    )

    snapshot_setup(project, NUM_SNAPSHOT_MODELS, table_comp)


@pytest.fixture
def check_cols_snapshot(project):
    NUM_SNAPSHOT_MODELS = 2
    table_comp = OverRideTabelComparison_dbt(
        adapter=project.adapter, unique_schema=project.test_schema, database=project.database
    )

    snapshot_setup(project, NUM_SNAPSHOT_MODELS, table_comp)


@pytest.fixture
def revived_snapshot(project):
    NUM_SNAPSHOT_MODELS = 2
    table_comp = RevivedTableComparison(
        adapter=project.adapter, unique_schema=project.test_schema, database=project.database
    )

    snapshot_setup(project, NUM_SNAPSHOT_MODELS, table_comp)


@pytest.fixture
def basic_ref(project):
    NUM_SNAPSHOT_MODELS = 1
    ref_setup(project, NUM_SNAPSHOT_MODELS)


@pytest.fixture
def basic_ref_two_snapshots(project):
    NUM_SNAPSHOT_MODELS = 2
    ref_setup(project, NUM_SNAPSHOT_MODELS)


class Basic:
    @pytest.fixture(scope="class")
    def snapshots(self, snapshots_pg):  # noqa: F811
        return snapshots_pg


@pytest.mark.usefixtures("project")
class TestBasicSnapshot(Basic):
    def test_basic_snapshot(project, basic_snapshot):
        basic_snapshot


@pytest.mark.usefixtures("project")
class TestBasicRef(Basic):
    def test_basic_ref(project, basic_ref):
        basic_ref


class CustomNamespace:
    @pytest.fixture(scope="class")
    def snapshots(self, snapshots_pg_custom_namespaced):  # noqa: F811
        return snapshots_pg_custom_namespaced

    @pytest.fixture(scope="class")
    def macros(self, macros_custom_snapshot):  # noqa: F811
        return macros_custom_snapshot


@pytest.mark.usefixtures("project")
class TestBasicCustomNamespace(CustomNamespace):
    def test_basic_snapshot(project, basic_snapshot):
        basic_snapshot


@pytest.mark.usefixtures("project")
class TestRefCustomNamespace(CustomNamespace):
    def test_basic_ref(project, basic_ref):
        basic_ref


class CustomSnapshot:
    @pytest.fixture(scope="class")
    def snapshots(self, snapshots_pg_custom):  # noqa: F811
        return snapshots_pg_custom

    @pytest.fixture(scope="class")
    def macros(self, macros_custom_snapshot):  # noqa: F811
        return macros_custom_snapshot


@pytest.mark.usefixtures("project")
class TestBasicCustomSnapshot(CustomSnapshot):
    def test_basic_snapshot(project, basic_snapshot):
        basic_snapshot


@pytest.mark.usefixtures("project")
class TestRefCustomSnapshot(CustomSnapshot):
    def test_basic_ref(project, basic_ref):
        basic_ref


class CheckCols:
    @pytest.fixture(scope="class")
    def snapshots(self):
        return {"snapshot.sql": snapshots_check_col__snapshot_sql}


@pytest.mark.usefixtures("project")
class TestBasicCheckCols(CheckCols):
    def test_basic_snapshot(project, check_cols_snapshot):
        check_cols_snapshot


@pytest.mark.usefixtures("project")
class TestRefCheckCols(CheckCols):
    def test_basic_ref(project, basic_ref_two_snapshots):
        basic_ref_two_snapshots


class ConfiguredCheckCols:
    @pytest.fixture(scope="class")
    def snapshots(self):
        return {"snapshot.sql": snapshots_check_col_noconfig__snapshot_sql}

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


@pytest.mark.usefixtures("project")
class TestBasicConfiguredCheckCols(ConfiguredCheckCols):
    def test_basic_snapshot(project, check_cols_snapshot):
        check_cols_snapshot


@pytest.mark.usefixtures("project")
class TestRefConfiguredCheckCols(ConfiguredCheckCols):
    def test_basic_ref(project, basic_ref_two_snapshots):
        basic_ref_two_snapshots


class UpdatedAtCheckCols:
    @pytest.fixture(scope="class")
    def snapshots(self):
        return {"snapshot.sql": snapshots_check_col_noconfig__snapshot_sql}

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


@pytest.mark.usefixtures("project")
class TestBasicUpdatedAtCheckCols(UpdatedAtCheckCols):
    def test_basic_snapshot(project, revived_snapshot):
        revived_snapshot


@pytest.mark.usefixtures("project")
class TestRefUpdatedAtCheckCols(UpdatedAtCheckCols):
    def test_basic_ref(project, basic_ref_two_snapshots):
        basic_ref_two_snapshots
