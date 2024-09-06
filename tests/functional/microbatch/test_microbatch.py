import os
from unittest import mock

import pytest
from freezegun import freeze_time

from dbt.tests.util import relation_from_name, run_dbt

input_model_sql = """
{{ config(materialized='table', event_time='event_time') }}

select 1 as id, TIMESTAMP '2020-01-01 00:00:00-0' as event_time
union all
select 2 as id, TIMESTAMP '2020-01-02 00:00:00-0' as event_time
union all
select 3 as id, TIMESTAMP '2020-01-03 00:00:00-0' as event_time
"""

microbatch_model_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='day') }}
select * from {{ ref('input_model') }}
"""


class BaseMicrobatchTest:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_sql,
        }

    def assert_row_count(self, project, relation_name: str, expected_row_count: int):
        relation = relation_from_name(project.adapter, relation_name)
        result = project.run_sql(f"select count(*) as num_rows from {relation}", fetch="one")

        if result[0] != expected_row_count:
            # running show for debugging
            run_dbt(["show", "--inline", f"select * from {relation}"])

            assert result[0] == expected_row_count


class TestMicrobatchCLI(BaseMicrobatchTest):
    @mock.patch.dict(os.environ, {"DBT_EXPERIMENTAL_MICROBATCH": "True"})
    def test_run_with_event_time(self, project):
        # run without --event-time-start or --event-time-end - 3 expected rows in output
        with freeze_time("2020-01-03 13:57:00", tick=True):
            run_dbt(["run"])
        self.assert_row_count(project, "microbatch_model", 3)

        # build model >= 2020-01-02
        with freeze_time("2020-01-03 13:57:00", tick=True):
            run_dbt(["run", "--event-time-start", "2020-01-02", "--full-refresh"])
        self.assert_row_count(project, "microbatch_model", 2)

        # build model < 2020-01-03
        run_dbt(["run", "--event-time-end", "2020-01-03", "--full-refresh"])
        self.assert_row_count(project, "microbatch_model", 2)

        # build model between 2020-01-02 >= event_time < 2020-01-03
        run_dbt(
            [
                "run",
                "--event-time-start",
                "2020-01-02",
                "--event-time-end",
                "2020-01-03",
                "--full-refresh",
            ]
        )
        self.assert_row_count(project, "microbatch_model", 1)


class TestMicroBatchBoundsDefault(BaseMicrobatchTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_sql,
        }

    @mock.patch.dict(os.environ, {"DBT_EXPERIMENTAL_MICROBATCH": "True"})
    def test_run_with_event_time(self, project):
        # initial run -- backfills up to current time
        with freeze_time("2020-01-01 13:57:00"):
            run_dbt(["run"])
        self.assert_row_count(project, "microbatch_model", 1)

        # our partition grain is "day" so running the same day without new data should produce the same results
        with freeze_time("2020-01-01 14:57:00"):
            run_dbt(["run"])
        self.assert_row_count(project, "microbatch_model", 1)

        # add next two days of data
        test_schema_relation = project.adapter.Relation.create(
            database=project.database, schema=project.test_schema
        )
        project.run_sql(
            f"insert into {test_schema_relation}.input_model(id, event_time) values (4, TIMESTAMP '2020-01-04 00:00:00-0'), (5, TIMESTAMP '2020-01-05 00:00:00-0')"
        )
        self.assert_row_count(project, "input_model", 5)

        # re-run without changing current time => no insert
        with freeze_time("2020-01-01 14:57:00"):
            run_dbt(["run", "--select", "microbatch_model"])
        self.assert_row_count(project, "microbatch_model", 1)

        # re-run by advancing time by one day changing current time => insert 1 row
        with freeze_time("2020-01-02 14:57:00"):
            run_dbt(["run", "--select", "microbatch_model"])
        self.assert_row_count(project, "microbatch_model", 2)

        # re-run by advancing time by one more day changing current time => insert 1 more row
        with freeze_time("2020-01-03 14:57:00"):
            run_dbt(["run", "--select", "microbatch_model"])
        self.assert_row_count(project, "microbatch_model", 3)


microbatch_model_failing_incremental_partition_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='day') }}
{% if '2020-01-02' in (model.config.event_time_start | string) %}
 invalid_sql
{% endif %}
select * from {{ ref('input_model') }}
"""


class TestMicrobatchIncrementalPartitionFailure(BaseMicrobatchTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_failing_incremental_partition_sql,
        }

    @mock.patch.dict(os.environ, {"DBT_EXPERIMENTAL_MICROBATCH": "True"})
    def test_run_with_event_time(self, project):
        # run all partitions from start - 2 expected rows in output, one failed
        with freeze_time("2020-01-03 13:57:00", tick=True):
            run_dbt(["run", "--event-time-start", "2020-01-01"])
        self.assert_row_count(project, "microbatch_model", 2)


microbatch_model_first_partition_failing_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='day') }}
{% if '2020-01-01' in (model.config.event_time_start | string) %}
 invalid_sql
{% endif %}
select * from {{ ref('input_model') }}
"""


class TestMicrobatchInitialPartitionFailure(BaseMicrobatchTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_first_partition_failing_sql,
        }

    @mock.patch.dict(os.environ, {"DBT_EXPERIMENTAL_MICROBATCH": "True"})
    def test_run_with_event_time(self, project):
        # run all partitions from start - 2 expected rows in output, one failed
        with freeze_time("2020-01-03 13:57:00", tick=True):
            run_dbt(["run", "--event-time-start", "2020-01-01"])
        self.assert_row_count(project, "microbatch_model", 2)
