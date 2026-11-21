"""End-to-end coverage for microbatch `begin` coercion under `--use-v2-parser`
(dbt-labs/dbt#16582).

The v2 path loads manifest.json directly and bypasses
`ManifestLoader.check_valid_microbatch_config`. After JSON deserialization a
`begin` config is always a plain string; without
`validate_and_coerce_microbatch_configs`, the first run / `--full-refresh`
crashes in `MicrobatchBuilder.truncate_timestamp` with
`'str' object has no attribute 'year'`.

These tests reuse `V2ParserFixture`'s fake parser (a native-parsed
manifest.json re-served as if a v2 binary wrote it), so the load path is the
same one `--use-v2-parser` takes in production.
"""

from datetime import datetime

import pytest

from dbt.tests.util import patch_microbatch_end_time, relation_from_name, run_dbt
from tests.functional.v2_parser.test_v2_parser_branch import V2ParserFixture

INPUT_MODEL_SQL = """
{{ config(materialized='table', event_time='event_time') }}

select 1 as id, TIMESTAMP '2020-01-01 00:00:00-0' as event_time
union all
select 2 as id, TIMESTAMP '2020-01-02 00:00:00-0' as event_time
union all
select 3 as id, TIMESTAMP '2020-01-03 00:00:00-0' as event_time
"""

# Quoted string `begin` -- the exact config shape that stays a `str` through
# Jinja and, after the v2 JSON round-trip, through Manifest deserialization.
MICROBATCH_MODEL_STRING_BEGIN_SQL = """
{{ config(
    materialized='incremental',
    incremental_strategy='microbatch',
    unique_key='id',
    event_time='event_time',
    batch_size='day',
    begin='2020-01-01',
    lookback=0
) }}
select * from {{ ref('input_model') }}
"""


class TestV2ParserMicrobatchBeginCoercion(V2ParserFixture):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "input_model.sql": INPUT_MODEL_SQL,
            "microbatch_model.sql": MICROBATCH_MODEL_STRING_BEGIN_SQL,
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "flags": {
                "require_batched_execution_for_custom_microbatch_strategy": True,
            }
        }

    def test_v2_parse_coerces_string_begin_to_datetime(self, project, fake_parser):
        """After `--use-v2-parser parse`, `begin` must be a real `datetime`
        (not left as the JSON string that came off the v2-shaped manifest)."""
        results = run_dbt(
            [
                "--use-v2-parser",
                f"--v2-parser={fake_parser}",
                "parse",
            ]
        )
        node = results.nodes["model.test.microbatch_model"]
        assert isinstance(node.config.begin, datetime)
        assert node.config.begin == datetime(2020, 1, 1)

    def test_v2_full_refresh_succeeds_with_string_begin(self, project, fake_parser):
        """The original crash: first run / `--full-refresh` under
        `--use-v2-parser` with a string `begin` must complete successfully
        rather than raising `'str' object has no attribute 'year'`."""
        with patch_microbatch_end_time("2020-01-03 13:57:00"):
            run_dbt(
                [
                    "--use-v2-parser",
                    f"--v2-parser={fake_parser}",
                    "run",
                    "--full-refresh",
                ]
            )

        relation = relation_from_name(project.adapter, "microbatch_model")
        result = project.run_sql(f"select count(*) as num_rows from {relation}", fetch="one")
        assert result[0] == 3
