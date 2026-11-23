import json
import os
from typing import Dict
from unittest import mock

import pytest
from pytest_mock import MockerFixture

from dbt.events.types import (
    GenericExceptionOnRun,
    InvalidConcurrentBatchesConfig,
    JinjaLogDebug,
    LogBatchResult,
    LogModelResult,
    MicrobatchExecutionDebug,
    MicrobatchMacroOutsideOfBatchesDeprecation,
    MicrobatchModelNoEventTimeInputs,
)
from dbt.tests.fixtures.project import TestProjInfo
from dbt.tests.util import (
    get_artifact,
    patch_microbatch_end_time,
    read_file,
    relation_from_name,
    run_dbt,
    run_dbt_and_capture,
    write_file,
)
from dbt_common.events.event_catcher import EventCatcher

input_model_sql = """
{{ config(materialized='table', event_time='event_time') }}

select 1 as id, TIMESTAMP '2020-01-01 00:00:00-0' as event_time
union all
select 2 as id, TIMESTAMP '2020-01-02 00:00:00-0' as event_time
union all
select 3 as id, TIMESTAMP '2020-01-03 00:00:00-0' as event_time
"""

input_model_invalid_sql = """
{{ config(materialized='table', event_time='event_time') }}

select invalid as event_time
"""

input_model_without_event_time_sql = """
{{ config(materialized='table') }}

select 1 as id, TIMESTAMP '2020-01-01 00:00:00-0' as event_time
union all
select 2 as id, TIMESTAMP '2020-01-02 00:00:00-0' as event_time
union all
select 3 as id, TIMESTAMP '2020-01-03 00:00:00-0' as event_time
"""

microbatch_model_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='day', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0)) }}
select * from {{ ref('input_model') }}
"""

microbatch_model_hour_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='hour', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0)) }}
select * from {{ ref('input_model') }}
"""

microbatch_model_month_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='month', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0)) }}
select * from {{ ref('input_model') }}
"""

microbatch_model_year_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='year', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0)) }}
select * from {{ ref('input_model') }}
"""

microbatch_model_with_pre_and_post_sql = """
{{ config(
        materialized='incremental',
        incremental_strategy='microbatch',
        unique_key='id',
        event_time='event_time',
        batch_size='day',
        begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0),
        pre_hook='{{log("execute: " ~ execute ~ ", pre-hook run by batch " ~ model.batch.id)}}',
        post_hook='{{log("execute: " ~ execute ~ ", post-hook run by batch " ~ model.batch.id)}}',
    )
}}
select * from {{ ref('input_model') }}
"""

microbatch_model_force_concurrent_batches_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='day', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0), concurrent_batches=true) }}
select * from {{ ref('input_model') }}
"""

microbatch_yearly_model_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='year', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0)) }}
select * from {{ ref('input_model') }}
"""

microbatch_yearly_model_downstream_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='year', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0)) }}
select * from {{ ref('microbatch_model') }}
"""

invalid_batch_jinja_context_macro_sql = """
{% macro check_invalid_batch_jinja_context() %}

{% if model is not mapping %}
    {{ exceptions.raise_compiler_error("`model` is invalid: expected mapping type") }}
{% elif compiled_code and compiled_code is not string %}
    {{ exceptions.raise_compiler_error("`compiled_code` is invalid: expected string type") }}
{% elif sql and sql is not string %}
    {{ exceptions.raise_compiler_error("`sql` is invalid: expected string type") }}
{% elif is_incremental is not callable %}
    {{ exceptions.raise_compiler_error("`is_incremental()` is invalid: expected callable type") }}
{% elif should_full_refresh is not callable %}
    {{ exceptions.raise_compiler_error("`should_full_refresh()` is invalid: expected callable type") }}
{% endif %}

{% endmacro %}
"""

microbatch_model_with_context_checks_sql = """
{{ config(pre_hook="{{ check_invalid_batch_jinja_context() }}", materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='day', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0)) }}

{{ check_invalid_batch_jinja_context() }}
select * from {{ ref('input_model') }}
"""

microbatch_model_downstream_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='day', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0)) }}
select * from {{ ref('microbatch_model') }}
"""

microbatch_model_ref_render_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='day', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0)) }}
select * from {{ ref('input_model').render() }}
"""

seed_csv = """id,event_time
1,'2020-01-01 00:00:00-0'
2,'2020-01-02 00:00:00-0'
3,'2020-01-03 00:00:00-0'
"""

seeds_yaml = """
seeds:
  - name: raw_source
    config:
      column_types:
        event_time: TIMESTAMP
"""

sources_yaml = """
sources:
  - name: seed_sources
    schema: "{{ target.schema }}"
    tables:
      - name: raw_source
        config:
          event_time: event_time
"""

microbatch_model_calling_source_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='day', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0)) }}
select * from {{ source('seed_sources', 'raw_source') }}
"""

custom_microbatch_strategy = """
{% macro get_incremental_microbatch_sql(arg_dict) %}
    {% do log('custom microbatch strategy', info=True) %}

     {%- set dest_cols_csv = get_quoted_csv(arg_dict["dest_columns"] | map(attribute="name")) -%}

    insert into {{ arg_dict["target_relation"] }} ({{ dest_cols_csv }})
    (
        select {{ dest_cols_csv }}
        from {{ arg_dict["temp_relation"] }}
    )

{% endmacro %}
"""


downstream_model_of_microbatch_sql = """
SELECT * FROM {{ ref('microbatch_model') }}
"""

microbatch_model_full_refresh_false_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='day', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0), full_refresh=False) }}
select * from {{ ref('input_model') }}
"""


class BaseMicrobatchCustomUserStrategy:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_sql,
        }

    @pytest.fixture(scope="class")
    def macros(self):
        return {"microbatch.sql": custom_microbatch_strategy}

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "flags": {
                "require_batched_execution_for_custom_microbatch_strategy": True,
            }
        }

    @pytest.fixture(scope="class")
    def deprecation_catcher(self) -> EventCatcher:
        return EventCatcher(MicrobatchMacroOutsideOfBatchesDeprecation)


class TestMicrobatchCustomUserStrategyDefault(BaseMicrobatchCustomUserStrategy):
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "flags": {
                "require_batched_execution_for_custom_microbatch_strategy": False,
            }
        }

    def test_use_custom_microbatch_strategy_by_default(
        self,
        project,
        deprecation_catcher: EventCatcher,
    ):
        # Initial run fires deprecation
        run_dbt(["run"], callbacks=[deprecation_catcher.catch])
        # Deprecation warning about custom microbatch macro fired
        assert len(deprecation_catcher.caught_events) == 1

        # Incremental run uses custom strategy
        _, logs = run_dbt_and_capture(["run"])
        assert "custom microbatch strategy" in logs
        # The custom strategy wasn't used with batch functionality
        assert "START batch" not in logs


class TestMicrobatchCustomUserStrategyProjectFlagTrueValid(BaseMicrobatchCustomUserStrategy):
    def test_use_custom_microbatch_strategy_project_flag_true_invalid_incremental_strategy(
        self,
        project,
        deprecation_catcher: EventCatcher,
    ):
        with mock.patch.object(
            type(project.adapter), "valid_incremental_strategies", lambda _: ["microbatch"]
        ):
            # Initial run
            with patch_microbatch_end_time("2020-01-03 13:57:00"):
                run_dbt(["run"], callbacks=[deprecation_catcher.catch])
            # Deprecation warning about custom microbatch macro not fired
            assert len(deprecation_catcher.caught_events) == 0

            # Incremental run uses custom strategy
            with patch_microbatch_end_time("2020-01-03 13:57:00"):
                _, logs = run_dbt_and_capture(["run"])
            assert "custom microbatch strategy" in logs
            # The custom strategy was used with batch functionality
            assert "START batch" in logs


class TestMicrobatchCustomUserStrategyProjectFlagTrueNoValidBuiltin(
    BaseMicrobatchCustomUserStrategy
):
    def test_use_custom_microbatch_strategy_project_flag_true_invalid_incremental_strategy(
        self, project
    ):
        with mock.patch.object(
            type(project.adapter), "valid_incremental_strategies", lambda _: []
        ):
            # Run of microbatch model while adapter doesn't have a "valid"
            # microbatch strategy causes no error when behaviour flag set to true
            # and there is a custom microbatch macro
            with patch_microbatch_end_time("2020-01-03 13:57:00"):
                _, logs = run_dbt_and_capture(["run"])
            assert "'microbatch' is not valid" not in logs
            assert (
                "The use of a custom microbatch macro outside of batched execution is deprecated"
                not in logs
            )


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
    CLI_COMMAND_NAME = "run"

    def test_run_with_event_time(self, project):
        # run without --event-time-start or --event-time-end - 3 expected rows in output

        model_catcher = EventCatcher(event_to_catch=LogModelResult)
        batch_catcher = EventCatcher(event_to_catch=LogBatchResult)

        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt([self.CLI_COMMAND_NAME], callbacks=[model_catcher.catch, batch_catcher.catch])
        self.assert_row_count(project, "microbatch_model", 3)

        assert len(model_catcher.caught_events) == 2
        assert len(batch_catcher.caught_events) == 3
        batch_creation_events = 0
        for caught_event in batch_catcher.caught_events:
            if "batch 2020" in caught_event.data.description:
                batch_creation_events += 1
                assert caught_event.data.execution_time > 0
        # 3 batches should have been run, so there should be 3 batch
        # creation events
        assert batch_creation_events == 3

        # build model between 2020-01-02 >= event_time < 2020-01-03
        run_dbt(
            [
                self.CLI_COMMAND_NAME,
                "--event-time-start",
                "2020-01-02",
                "--event-time-end",
                "2020-01-03",
                "--full-refresh",
            ]
        )
        self.assert_row_count(project, "microbatch_model", 1)


class TestMicrobatchCLIBuild(TestMicrobatchCLI):
    CLI_COMMAND_NAME = "build"


class TestMicrobatchCLIRunOutputJSON(BaseMicrobatchTest):
    def test_list_output_json(self, project: TestProjInfo):
        """Test whether the command `dbt list --output json` works"""
        model_catcher = EventCatcher(event_to_catch=LogModelResult)
        batch_catcher = EventCatcher(event_to_catch=LogBatchResult)

        _, microbatch_json = run_dbt(
            ["list", "--output", "json"], callbacks=[model_catcher.catch, batch_catcher.catch]
        )
        microbatch_dict = json.loads(microbatch_json)
        assert microbatch_dict["config"]["begin"] == "2020-01-01T00:00:00"


class TestMicroBatchBoundsDefault(BaseMicrobatchTest):
    def test_run_with_event_time(self, project):
        # initial run -- backfills all data
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(["run"])
        self.assert_row_count(project, "microbatch_model", 3)

        # our partition grain is "day" so running the same day without new data should produce the same results
        with patch_microbatch_end_time("2020-01-03 14:57:00"):
            run_dbt(["run"])
        self.assert_row_count(project, "microbatch_model", 3)

        # add next two days of data
        test_schema_relation = project.adapter.Relation.create(
            database=project.database, schema=project.test_schema
        )
        project.run_sql(
            f"insert into {test_schema_relation}.input_model(id, event_time) values (4, TIMESTAMP '2020-01-04 00:00:00-0'), (5, TIMESTAMP '2020-01-05 00:00:00-0')"
        )
        self.assert_row_count(project, "input_model", 5)

        # re-run without changing current time => no insert
        with patch_microbatch_end_time("2020-01-03 14:57:00"):
            run_dbt(["run", "--select", "microbatch_model"])
        self.assert_row_count(project, "microbatch_model", 3)

        # re-run by advancing time by one day changing current time => insert 1 row
        with patch_microbatch_end_time("2020-01-04 14:57:00"):
            run_dbt(["run", "--select", "microbatch_model"])
        self.assert_row_count(project, "microbatch_model", 4)

        # re-run by advancing time by one more day changing current time => insert 1 more row
        with patch_microbatch_end_time("2020-01-05 14:57:00"):
            run_dbt(["run", "--select", "microbatch_model"])
        self.assert_row_count(project, "microbatch_model", 5)


class TestMicrobatchWithSource(BaseMicrobatchTest):
    @pytest.fixture(scope="class")
    def seeds(self):
        return {
            "raw_source.csv": seed_csv,
        }

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "microbatch_model.sql": microbatch_model_calling_source_sql,
            "sources.yml": sources_yaml,
            "seeds.yml": seeds_yaml,
        }

    def test_run_with_event_time(self, project):
        # ensure seed is created for source
        run_dbt(["seed"])

        # initial run -- backfills all data
        catcher = EventCatcher(event_to_catch=MicrobatchModelNoEventTimeInputs)
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(["run"], callbacks=[catcher.catch])
        self.assert_row_count(project, "microbatch_model", 3)
        assert len(catcher.caught_events) == 0

        # our partition grain is "day" so running the same day without new data should produce the same results
        with patch_microbatch_end_time("2020-01-03 14:57:00"):
            run_dbt(["run"])
        self.assert_row_count(project, "microbatch_model", 3)

        # add next two days of data
        test_schema_relation = project.adapter.Relation.create(
            database=project.database, schema=project.test_schema
        )
        project.run_sql(
            f"insert into {test_schema_relation}.raw_source(id, event_time) values (4, TIMESTAMP '2020-01-04 00:00:00-0'), (5, TIMESTAMP '2020-01-05 00:00:00-0')"
        )
        self.assert_row_count(project, "raw_source", 5)

        # re-run without changing current time => no insert
        with patch_microbatch_end_time("2020-01-03 14:57:00"):
            run_dbt(["run", "--select", "microbatch_model"])
        self.assert_row_count(project, "microbatch_model", 3)

        # re-run by advancing time by one day changing current time => insert 1 row
        with patch_microbatch_end_time("2020-01-04 14:57:00"):
            run_dbt(["run", "--select", "microbatch_model"])
        self.assert_row_count(project, "microbatch_model", 4)

        # re-run by advancing time by one more day changing current time => insert 1 more row
        with patch_microbatch_end_time("2020-01-05 14:57:00"):
            run_dbt(["run", "--select", "microbatch_model"])
        self.assert_row_count(project, "microbatch_model", 5)


class TestMicrobatchJinjaContext(BaseMicrobatchTest):

    @pytest.fixture(scope="class")
    def macros(self):
        return {"check_batch_jinja_context.sql": invalid_batch_jinja_context_macro_sql}

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_with_context_checks_sql,
        }

    def test_run_with_event_time(self, project):
        # initial run -- backfills all data
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(["run"])
        self.assert_row_count(project, "microbatch_model", 3)


class TestMicrobatchWithInputWithoutEventTime(BaseMicrobatchTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_without_event_time_sql,
            "microbatch_model.sql": microbatch_model_sql,
        }

    def test_run_with_event_time(self, project):
        catcher = EventCatcher(event_to_catch=MicrobatchModelNoEventTimeInputs)

        # initial run -- backfills all data
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(["run"], callbacks=[catcher.catch])
        self.assert_row_count(project, "microbatch_model", 3)
        assert len(catcher.caught_events) == 1

        # our partition grain is "day" so running the same day without new data should produce the same results
        catcher.caught_events = []
        with patch_microbatch_end_time("2020-01-03 14:57:00"):
            run_dbt(["run"], callbacks=[catcher.catch])
        self.assert_row_count(project, "microbatch_model", 3)
        assert len(catcher.caught_events) == 1

        # add next two days of data
        test_schema_relation = project.adapter.Relation.create(
            database=project.database, schema=project.test_schema
        )
        project.run_sql(
            f"insert into {test_schema_relation}.input_model(id, event_time) values (4, TIMESTAMP '2020-01-04 00:00:00-0'), (5, TIMESTAMP '2020-01-05 00:00:00-0')"
        )
        self.assert_row_count(project, "input_model", 5)

        # re-run without changing current time => INSERT BECAUSE INPUT MODEL ISN'T BEING FILTERED
        with patch_microbatch_end_time("2020-01-03 14:57:00"):
            run_dbt(["run", "--select", "microbatch_model"])
        self.assert_row_count(project, "microbatch_model", 5)


class TestMicrobatchUsingRefRenderSkipsFilter(BaseMicrobatchTest):
    def test_run_with_event_time(self, project):
        # initial run -- backfills all data
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(["run"])
        self.assert_row_count(project, "microbatch_model", 3)

        # our partition grain is "day" so running the same day without new data should produce the same results
        with patch_microbatch_end_time("2020-01-03 14:57:00"):
            run_dbt(["run"])
        self.assert_row_count(project, "microbatch_model", 3)

        # add next two days of data
        test_schema_relation = project.adapter.Relation.create(
            database=project.database, schema=project.test_schema
        )
        project.run_sql(
            f"insert into {test_schema_relation}.input_model(id, event_time) values (4, TIMESTAMP '2020-01-04 00:00:00-0'), (5, TIMESTAMP '2020-01-05 00:00:00-0')"
        )
        self.assert_row_count(project, "input_model", 5)

        # re-run without changing current time => no insert
        with patch_microbatch_end_time("2020-01-03 14:57:00"):
            run_dbt(["run", "--select", "microbatch_model"])
        self.assert_row_count(project, "microbatch_model", 3)

        # Update microbatch model to call .render() on ref('input_model')
        write_file(
            microbatch_model_ref_render_sql, project.project_root, "models", "microbatch_model.sql"
        )

        # re-run without changing current time => INSERT because .render() skips filtering
        with patch_microbatch_end_time("2020-01-03 14:57:00"):
            run_dbt(["run", "--select", "microbatch_model"])
        self.assert_row_count(project, "microbatch_model", 5)


microbatch_model_context_vars = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='day', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0)) }}
{{ log("start: "~ model.config.__dbt_internal_microbatch_event_time_start, info=True)}}
{{ log("end: "~ model.config.__dbt_internal_microbatch_event_time_end, info=True)}}
{% if model.batch %}
{{ log("batch.event_time_start: "~ model.batch.event_time_start, info=True)}}
{{ log("batch.event_time_end: "~ model.batch.event_time_end, info=True)}}
{{ log("batch.id: "~ model.batch.id, info=True)}}
{{ log("start timezone: "~ model.batch.event_time_start.tzinfo, info=True)}}
{{ log("end timezone: "~ model.batch.event_time_end.tzinfo, info=True)}}
{% endif %}
select * from {{ ref('input_model') }}
"""


class TestMicrobatchJinjaContextVarsAvailable(BaseMicrobatchTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_context_vars,
        }

    def test_run_with_event_time_logs(self, project):
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            _, logs = run_dbt_and_capture(["run"])

        assert "start: 2020-01-01 00:00:00+00:00" in logs
        assert "end: 2020-01-02 00:00:00+00:00" in logs
        assert "batch.event_time_start: 2020-01-01 00:00:00+00:00" in logs
        assert "batch.event_time_end: 2020-01-02 00:00:00+00:00" in logs
        assert "batch.id: 20200101" in logs
        assert "start timezone: UTC" in logs
        assert "end timezone: UTC" in logs

        assert "start: 2020-01-02 00:00:00+00:00" in logs
        assert "end: 2020-01-03 00:00:00+00:00" in logs
        assert "batch.event_time_start: 2020-01-02 00:00:00+00:00" in logs
        assert "batch.event_time_end: 2020-01-03 00:00:00+00:00" in logs
        assert "batch.id: 20200102" in logs

        assert "start: 2020-01-03 00:00:00+00:00" in logs
        assert "end: 2020-01-03 13:57:00+00:00" in logs
        assert "batch.event_time_start: 2020-01-03 00:00:00+00:00" in logs
        assert "batch.event_time_end: 2020-01-03 13:57:00+00:00" in logs
        assert "batch.id: 20200103" in logs

        # compile does not have access to populated batch context vars, but should not break on access
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            _, compile_logs = run_dbt_and_capture(["compile"])

        assert "start:" in compile_logs
        assert "end:" in compile_logs
        assert "batch.event_time_start:" not in compile_logs
        assert "batch.event_time_end:" not in compile_logs


microbatch_model_failing_incremental_partition_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='day', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0)) }}
{% if '2020-01-02' in (model.config.__dbt_internal_microbatch_event_time_start | string) %}
 invalid_sql
{% endif %}
select * from {{ ref('input_model') }}
"""


class TestMicrobatchIncrementalBatchFailure(BaseMicrobatchTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_failing_incremental_partition_sql,
            "downstream_model.sql": downstream_model_of_microbatch_sql,
        }

    def test_run_with_event_time(self, project):
        event_catcher = EventCatcher(
            GenericExceptionOnRun, predicate=lambda event: event.data.node_info is not None
        )

        # run all partitions from start - 2 expected rows in output, one failed
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(["run"], callbacks=[event_catcher.catch], expect_pass=False)

        assert len(event_catcher.caught_events) == 1
        self.assert_row_count(project, "microbatch_model", 2)

        run_results = get_artifact(project.project_root, "target", "run_results.json")
        microbatch_run_result = run_results["results"][1]
        assert microbatch_run_result["status"] == "partial success"
        batch_results = microbatch_run_result["batch_results"]
        assert batch_results is not None
        assert len(batch_results["successful"]) == 2
        assert len(batch_results["failed"]) == 1
        assert run_results["results"][2]["status"] == "skipped"


class TestMicrobatchRetriesPartialSuccesses(BaseMicrobatchTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_failing_incremental_partition_sql,
        }

    def test_run_with_event_time(self, project):
        # run all partitions from start - 2 expected rows in output, one failed
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            _, console_output = run_dbt_and_capture(["run"], expect_pass=False)

        assert "PARTIAL SUCCESS (2/3)" in console_output
        assert "Completed with 1 partial success" in console_output

        self.assert_row_count(project, "microbatch_model", 2)

        run_results = get_artifact(project.project_root, "target", "run_results.json")
        microbatch_run_result = run_results["results"][1]
        assert microbatch_run_result["status"] == "partial success"
        batch_results = microbatch_run_result["batch_results"]
        assert batch_results is not None
        assert len(batch_results["successful"]) == 2
        assert len(batch_results["failed"]) == 1

        # update the microbatch model so that it no longer fails
        write_file(microbatch_model_sql, project.project_root, "models", "microbatch_model.sql")

        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            _, console_output = run_dbt_and_capture(["retry"])

        assert "PARTIAL SUCCESS" not in console_output
        assert "Completed with 1 partial success" not in console_output
        assert "Completed successfully" in console_output

        self.assert_row_count(project, "microbatch_model", 3)


class TestMicrobatchMultipleRetries(BaseMicrobatchTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_failing_incremental_partition_sql,
        }

    def test_run_with_event_time(self, project):
        # run all partitions from start - 2 expected rows in output, one failed
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            _, console_output = run_dbt_and_capture(["run"], expect_pass=False)

        assert "PARTIAL SUCCESS (2/3)" in console_output
        assert "Completed with 1 partial success" in console_output

        self.assert_row_count(project, "microbatch_model", 2)

        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            _, console_output = run_dbt_and_capture(["retry"], expect_pass=False)

        assert "PARTIAL SUCCESS" not in console_output
        assert "ERROR" in console_output
        assert "Completed with 1 error, 0 partial successes, and 0 warnings" in console_output

        self.assert_row_count(project, "microbatch_model", 2)

        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            _, console_output = run_dbt_and_capture(["retry"], expect_pass=False)

        assert "PARTIAL SUCCESS" not in console_output
        assert "ERROR" in console_output
        assert "Completed with 1 error, 0 partial successes, and 0 warnings" in console_output

        self.assert_row_count(project, "microbatch_model", 2)


microbatch_model_first_partition_failing_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='day', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0)) }}
{% if '2020-01-01' in (model.config.__dbt_internal_microbatch_event_time_start | string) %}
 invalid_sql
{% endif %}
select * from {{ ref('input_model') }}
"""

microbatch_model_second_batch_failing_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='day', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0)) }}
{% if '20200102' == model.batch.id %}
 invalid_sql
{% endif %}
select * from {{ ref('input_model') }}
"""


class TestMicrobatchInitialBatchFailure(BaseMicrobatchTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_first_partition_failing_sql,
        }

    def test_run_with_event_time(self, project):
        # When the first batch of a microbatch model fails, the rest of the batches should
        # be skipped and the model marked as failed (not _partial success_)

        general_exc_catcher = EventCatcher(
            GenericExceptionOnRun,
            predicate=lambda event: event.data.node_info is not None,
        )
        batch_catcher = EventCatcher(
            event_to_catch=LogBatchResult,
            predicate=lambda event: event.data.status == "skipped",
        )

        # run all partitions from start - 2 expected rows in output, one failed
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(
                ["run"],
                expect_pass=False,
                callbacks=[general_exc_catcher.catch, batch_catcher.catch],
            )

        assert len(general_exc_catcher.caught_events) == 1
        assert len(batch_catcher.caught_events) == 2

        # Because the first batch failed, and the rest of the batches were skipped, the table shouldn't
        # exist in the data warehosue
        relation_info = relation_from_name(project.adapter, "microbatch_model")
        relation = project.adapter.get_relation(
            relation_info.database, relation_info.schema, relation_info.name
        )
        assert relation is None


class TestMicrobatchSecondBatchFailure(BaseMicrobatchTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_second_batch_failing_sql,
        }

    def test_run_with_event_time(self, project):
        event_catcher = EventCatcher(
            GenericExceptionOnRun, predicate=lambda event: event.data.node_info is not None
        )

        # run all partitions from start - 2 expected rows in output, one failed
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(["run"], expect_pass=False, callbacks=[event_catcher.catch])
        assert len(event_catcher.caught_events) == 1
        self.assert_row_count(project, "microbatch_model", 2)


class TestMicrobatchCompiledRunPaths(BaseMicrobatchTest):
    def test_run_with_event_time(self, project):
        # run all partitions from start - 2 expected rows in output, one failed
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(["run"])

        # Compiled paths - batch compilations
        assert read_file(
            project.project_root,
            "target",
            "compiled",
            "test",
            "models",
            "microbatch_model",
            "microbatch_model_2020-01-01.sql",
        )
        assert read_file(
            project.project_root,
            "target",
            "compiled",
            "test",
            "models",
            "microbatch_model",
            "microbatch_model_2020-01-02.sql",
        )
        assert read_file(
            project.project_root,
            "target",
            "compiled",
            "test",
            "models",
            "microbatch_model",
            "microbatch_model_2020-01-03.sql",
        )

        assert read_file(
            project.project_root,
            "target",
            "run",
            "test",
            "models",
            "microbatch_model",
            "microbatch_model_2020-01-01.sql",
        )
        assert read_file(
            project.project_root,
            "target",
            "run",
            "test",
            "models",
            "microbatch_model",
            "microbatch_model_2020-01-02.sql",
        )
        assert read_file(
            project.project_root,
            "target",
            "run",
            "test",
            "models",
            "microbatch_model",
            "microbatch_model_2020-01-03.sql",
        )


class TestMicrobatchCompiledRunPathsHourly(BaseMicrobatchTest):

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_hour_sql,
        }

    def test_run_with_event_time(self, project):
        # run all partitions from start - 2 expected rows in output, one failed
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(["run"])

        # Compiled paths - batch compilations
        assert read_file(
            project.project_root,
            "target",
            "compiled",
            "test",
            "models",
            "microbatch_model",
            "microbatch_model_2020-01-03T13.sql",
        )
        assert read_file(
            project.project_root,
            "target",
            "run",
            "test",
            "models",
            "microbatch_model",
            "microbatch_model_2020-01-03T13.sql",
        )


class TestMicrobatchCompiledRunPathsMonthly(BaseMicrobatchTest):

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_month_sql,
        }

    def test_run_with_event_time(self, project):
        # run all partitions from start - 2 expected rows in output, one failed
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(["run"])

        # Compiled paths - batch compilations
        assert read_file(
            project.project_root,
            "target",
            "compiled",
            "test",
            "models",
            "microbatch_model",
            "microbatch_model_2020-01.sql",
        )
        assert read_file(
            project.project_root,
            "target",
            "run",
            "test",
            "models",
            "microbatch_model",
            "microbatch_model_2020-01.sql",
        )


class TestMicrobatchCompiledRunPathsYearly(BaseMicrobatchTest):

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_year_sql,
        }

    def test_run_with_event_time(self, project):
        # run all partitions from start - 2 expected rows in output, one failed
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(["run"])

        # Compiled paths - batch compilations
        assert read_file(
            project.project_root,
            "target",
            "compiled",
            "test",
            "models",
            "microbatch_model",
            "microbatch_model_2020.sql",
        )
        assert read_file(
            project.project_root,
            "target",
            "run",
            "test",
            "models",
            "microbatch_model",
            "microbatch_model_2020.sql",
        )


class TestMicrobatchFullRefreshConfigFalse(BaseMicrobatchTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_full_refresh_false_sql,
            "downstream_model.sql": downstream_model_of_microbatch_sql,
        }

    def test_run_with_event_time(self, project):
        # run all partitions from 2020-01-02 to spoofed "now" - 2 expected rows in output
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(
                [
                    "run",
                    "--event-time-start",
                    "2020-01-02",
                    "--event-time-end",
                    "2020-01-03 13:57:00",
                ]
            )
        self.assert_row_count(project, "microbatch_model", 2)

        # re-running shouldn't change what it's in the data set because there is nothing new
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(["run"])
        self.assert_row_count(project, "microbatch_model", 2)

        # running with --full-refresh shouldn't pick up 2020-01-01 BECAUSE the model has
        # full_refresh = false
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(["run", "--full-refresh"])
        self.assert_row_count(project, "microbatch_model", 2)

        # update the microbatch model to no longer have full_refresh=False config
        write_file(microbatch_model_sql, project.project_root, "models", "microbatch_model.sql")

        # running with full refresh should now pick up the 2020-01-01 data
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(["run", "--full-refresh"])
        self.assert_row_count(project, "microbatch_model", 3)


class TestMicrbobatchModelsRunWithSameCurrentTime(BaseMicrobatchTest):

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_yearly_model_sql,
            "second_microbatch_model.sql": microbatch_yearly_model_downstream_sql,
        }

    def test_microbatch(self, project) -> None:
        run_dbt(["run"])

        run_results = get_artifact(project.project_root, "target", "run_results.json")
        microbatch_model_last_batch = run_results["results"][1]["batch_results"]["successful"][-1]
        second_microbatch_model_last_batch = run_results["results"][2]["batch_results"][
            "successful"
        ][-1]

        # they should have the same last batch because they are using the _same_ "current_time"
        assert microbatch_model_last_batch == second_microbatch_model_last_batch


class TestMicrobatchModelSkipped(BaseMicrobatchTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_invalid_sql,
            "microbatch_model.sql": microbatch_model_sql,
        }

    def test_microbatch_model_skipped(self, project) -> None:
        run_dbt(["run"], expect_pass=False)

        run_results = get_artifact(project.project_root, "target", "run_results.json")

        microbatch_result = run_results["results"][1]
        assert microbatch_result["status"] == "skipped"
        assert microbatch_result["batch_results"] is None


class TestMicrobatchCanRunParallelOrSequential(BaseMicrobatchTest):
    @pytest.fixture
    def batch_exc_catcher(self) -> EventCatcher:
        return EventCatcher(MicrobatchExecutionDebug)

    def test_microbatch(
        self, mocker: MockerFixture, project, batch_exc_catcher: EventCatcher
    ) -> None:
        mocked_srip = mocker.patch("dbt.task.run.MicrobatchBatchRunner.should_run_in_parallel")

        # Should be run in parallel
        mocked_srip.return_value = True
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            _ = run_dbt(["run"], callbacks=[batch_exc_catcher.catch])

        assert len(batch_exc_catcher.caught_events) > 1
        some_batches_run_concurrently = False
        for caugh_event in batch_exc_catcher.caught_events:
            if "is being run concurrently" in caugh_event.data.msg:  # type: ignore
                some_batches_run_concurrently = True
                break
        assert some_batches_run_concurrently, "Found no batches being run concurrently!"

        # reset caught events
        batch_exc_catcher.caught_events = []

        # Should _not_ run in parallel
        mocked_srip.return_value = False
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            _ = run_dbt(["run"], callbacks=[batch_exc_catcher.catch])

        assert len(batch_exc_catcher.caught_events) > 1
        some_batches_run_concurrently = False
        for caugh_event in batch_exc_catcher.caught_events:
            if "is being run concurrently" in caugh_event.data.msg:  # type: ignore
                some_batches_run_concurrently = True
                break
        assert not some_batches_run_concurrently, "Found a batch being run concurrently!"


class TestFirstAndLastBatchAlwaysSequential(BaseMicrobatchTest):
    @pytest.fixture
    def batch_exc_catcher(self) -> EventCatcher:
        return EventCatcher(
            event_to_catch=MicrobatchExecutionDebug,
            predicate=lambda event: "is being run" in event.data.msg,
        )

    def test_microbatch(
        self, mocker: MockerFixture, project, batch_exc_catcher: EventCatcher
    ) -> None:
        mocked_srip = mocker.patch("dbt.task.run.MicrobatchBatchRunner.should_run_in_parallel")

        # Should be run in parallel
        mocked_srip.return_value = True
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            _ = run_dbt(["run"], callbacks=[batch_exc_catcher.catch])

        assert len(batch_exc_catcher.caught_events) > 1

        first_batch_event = batch_exc_catcher.caught_events[0]
        last_batch_event = batch_exc_catcher.caught_events[-1]

        for event in [first_batch_event, last_batch_event]:
            assert "is being run sequentially" in event.data.msg  # type: ignore

        for event in batch_exc_catcher.caught_events[1:-1]:
            assert "is being run concurrently" in event.data.msg  # type: ignore


class TestFirstBatchRunsPreHookLastBatchRunsPostHook(BaseMicrobatchTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_with_pre_and_post_sql,
        }

    @pytest.fixture
    def batch_log_catcher(self) -> EventCatcher:
        def pre_or_post_hook(event) -> bool:
            return "execute: True" in event.data.msg and (
                "pre-hook" in event.data.msg or "post-hook" in event.data.msg
            )

        return EventCatcher(event_to_catch=JinjaLogDebug, predicate=pre_or_post_hook)

    def test_microbatch(
        self, mocker: MockerFixture, project, batch_log_catcher: EventCatcher
    ) -> None:
        with patch_microbatch_end_time("2020-01-04 13:57:00"):
            _ = run_dbt(["run"], callbacks=[batch_log_catcher.catch])

        # There should be two logs as the pre-hook and post-hook should
        # both only be run once
        assert len(batch_log_catcher.caught_events) == 2

        for event in batch_log_catcher.caught_events:
            # batch id that should be firing pre-hook
            if "20200101" in event.data.msg:  # type: ignore
                assert "pre-hook" in event.data.msg  # type: ignore

            # batch id that should be firing the post-hook
            if "20200104" in event.data.msg:  # type: ignore
                assert "post-hook" in event.data.msg  # type: ignore


class TestWhenOnlyOneBatchRunBothPostAndPreHooks(BaseMicrobatchTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_with_pre_and_post_sql,
        }

    @pytest.fixture
    def batch_log_catcher(self) -> EventCatcher:
        def pre_or_post_hook(event) -> bool:
            return "execute: True" in event.data.msg and (
                "pre-hook" in event.data.msg or "post-hook" in event.data.msg
            )

        return EventCatcher(event_to_catch=JinjaLogDebug, predicate=pre_or_post_hook)

    @pytest.fixture
    def generic_exception_catcher(self) -> EventCatcher:
        return EventCatcher(event_to_catch=GenericExceptionOnRun)

    def test_microbatch(
        self,
        project,
        batch_log_catcher: EventCatcher,
        generic_exception_catcher: EventCatcher,
    ) -> None:
        with patch_microbatch_end_time("2020-01-01 13:57:00"):
            _ = run_dbt(
                ["run"], callbacks=[batch_log_catcher.catch, generic_exception_catcher.catch]
            )

        # There should be two logs as the pre-hook and post-hook should
        # both only be run once
        assert len(batch_log_catcher.caught_events) == 2

        assert "20200101" in batch_log_catcher.caught_events[0].data.msg  # type: ignore
        assert "pre-hook" in batch_log_catcher.caught_events[0].data.msg  # type: ignore
        assert "20200101" in batch_log_catcher.caught_events[1].data.msg  # type: ignore
        assert "post-hook" in batch_log_catcher.caught_events[1].data.msg  # type: ignore

        # we had a bug where having only one batch caused a generic exception
        assert len(generic_exception_catcher.caught_events) == 0


class TestCanSilenceInvalidConcurrentBatchesConfigWarning(BaseMicrobatchTest):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_force_concurrent_batches_sql,
        }

    @pytest.fixture
    def event_catcher(self) -> EventCatcher:
        return EventCatcher(event_to_catch=InvalidConcurrentBatchesConfig)

    def test_microbatch(
        self,
        project,
        event_catcher: EventCatcher,
    ) -> None:
        # This test works because postgres doesn't support concurrent batch execution
        # If the postgres adapter starts supporting concurrent batch execution we'll
        # need to start mocking the return value of `adapter.supports()`

        with patch_microbatch_end_time("2020-01-01 13:57:00"):
            _ = run_dbt(["run"], callbacks=[event_catcher.catch])
        # We didn't silence the warning, so we get it
        assert len(event_catcher.caught_events) == 1

        # Clear caught events
        event_catcher.caught_events = []

        # Run again with silencing
        with patch_microbatch_end_time("2020-01-01 13:57:00"):
            _ = run_dbt(
                ["run", "--warn-error-options", "{'silence': ['InvalidConcurrentBatchesConfig']}"],
                callbacks=[event_catcher.catch],
            )
        # Because we silenced the warning, it shouldn't get fired
        assert len(event_catcher.caught_events) == 0


single_batch_microbatch_model_sql = """
{{
    config(
        materialized='incremental',
        incremental_strategy='microbatch',
        unique_key='tmp',
        event_time='tmp',
        begin=modules.datetime.datetime.now(),
        lookback=0,
        batch_size='day',
        meta={'param': 'invalid_param'},
        pre_hook=[
            validate_param('param1')
        ]
    )
}}

SELECT current_date as tmp
"""

validate_param_macro_sql = """
{% macro validate_param(valid_param) %}
    {% set current_param = config.get('meta')['param'] %}

    {% if execute %}
        {% if current_param != valid_param %}
            {% set exceptions_message = "Invalid param: " ~ current_param ~ ", valid param: " ~ valid_param %}
            {% do exceptions.raise_compiler_error(exceptions_message) %}
        {% endif %}
    {% endif %}
{% endmacro %}
"""


class TestCompilationErrorOnSingleBatchRun(BaseMicrobatchTest):
    @pytest.fixture(scope="class")
    def models(self) -> Dict[str, str]:
        return {
            "microbatch_model.sql": single_batch_microbatch_model_sql,
        }

    @pytest.fixture(scope="class")
    def macros(self) -> Dict[str, str]:
        return {
            "validate_param.sql": validate_param_macro_sql,
        }

    def test_microbatch(self, project) -> None:
        _, console_output = run_dbt_and_capture(["run"], expect_pass=False)
        assert "Completed with 1 error, 0 partial successes, and 0 warnings" in console_output


microbatch_model_all_failing_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', event_time='event_time', batch_size='day', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0)) }}
 invalid_sql
select * from {{ ref('input_model') }}
"""


class TestMicrobatchRetryUsesOriginalInvocationTime(BaseMicrobatchTest):
    """When all batches fail and dbt retry is run later, the retry should
    recompute batches using the original invocation time, not the current time."""

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_all_failing_sql,
        }

    def test_retry_uses_original_invocation_time(self, project):
        # Run with end time of 2020-01-03 — produces 3 batches, all fail
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(["run"], expect_pass=False)

        run_results = get_artifact(project.project_root, "target", "run_results.json")
        microbatch_result = next(
            r for r in run_results["results"] if "microbatch_model" in r["unique_id"]
        )
        assert microbatch_result["status"] == "error"

        # Simulate the original run happening at 2020-01-03 by updating
        # invocation_started_at in the saved run_results.json
        run_results_path = os.path.join(project.project_root, "target", "run_results.json")
        run_results["metadata"]["invocation_started_at"] = "2020-01-03T13:57:00Z"
        with open(run_results_path, "w") as f:
            json.dump(run_results, f)

        # Fix the model so retry can succeed
        write_file(
            microbatch_model_sql,
            project.project_root,
            "models",
            "microbatch_model.sql",
        )

        # Run retry WITHOUT patching end time. The fix ensures retry
        # uses the original invocation time from run_results.json (2020-01-03),
        # so only 3 batches are produced. Without the fix, it would use
        # the current time and produce many more batches.
        batch_catcher = EventCatcher(event_to_catch=LogBatchResult)
        run_dbt(["retry"], callbacks=[batch_catcher.catch])

        # Should only have 3 batches (2020-01-01, 2020-01-02, 2020-01-03)
        assert len(batch_catcher.caught_events) == 3
        self.assert_row_count(project, "microbatch_model", 3)


class TestMicrobatchBuildRetryUsesOriginalInvocationTime(BaseMicrobatchTest):
    """Same as above but using dbt build + retry to verify the issubclass
    check ensures BuildTask also gets original invocation time behavior."""

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_all_failing_sql,
        }

    def test_build_retry_uses_original_invocation_time(self, project):
        # Run build with end time of 2020-01-03 — produces 3 batches, all fail
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(["build"], expect_pass=False)

        run_results = get_artifact(project.project_root, "target", "run_results.json")
        microbatch_result = next(
            r for r in run_results["results"] if "microbatch_model" in r["unique_id"]
        )
        assert microbatch_result["status"] == "error"

        # Simulate the original build happening at 2020-01-03 by updating
        # invocation_started_at in the saved run_results.json
        run_results_path = os.path.join(project.project_root, "target", "run_results.json")
        run_results["metadata"]["invocation_started_at"] = "2020-01-03T13:57:00Z"
        with open(run_results_path, "w") as f:
            json.dump(run_results, f)

        # Fix the model so retry can succeed
        write_file(
            microbatch_model_sql,
            project.project_root,
            "models",
            "microbatch_model.sql",
        )

        # Retry should use the original invocation time, not "now"
        batch_catcher = EventCatcher(event_to_catch=LogBatchResult)
        run_dbt(["retry"], callbacks=[batch_catcher.catch])

        # Should only have 3 batches (2020-01-01, 2020-01-02, 2020-01-03)
        assert len(batch_catcher.caught_events) == 3
        self.assert_row_count(project, "microbatch_model", 3)


microbatch_model_contracted_sql = """
{{ config(materialized='incremental', incremental_strategy='microbatch', unique_key='id', on_schema_change='fail', event_time='event_time', batch_size='day', begin=modules.datetime.datetime(2020, 1, 1, 0, 0, 0)) }}
select id, cast(id as text) as label, event_time from {{ ref('input_model') }}
"""

microbatch_model_contracted_yml = """
models:
  - name: microbatch_model
    config:
      contract:
        enforced: true
    columns:
      - name: id
        data_type: integer
      - name: label
        data_type: string
      - name: event_time
        data_type: timestamp
"""


class TestMicrobatchContractAliasTypes(BaseMicrobatchTest):
    """A contracted microbatch model translates data_type aliases (string -> text) in every batch."""

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": input_model_sql,
            "microbatch_model.sql": microbatch_model_contracted_sql,
            "schema.yml": microbatch_model_contracted_yml,
        }

    def test_run_with_contract_alias_types(self, project):
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(["run"])
        self.assert_row_count(project, "microbatch_model", 3)
