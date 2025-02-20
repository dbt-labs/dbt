import os

import pytest

from dbt.tests.util import check_relations_equal, run_dbt

snapshot_actual_sql = """
{% snapshot snapshot_actual %}

    {{
        config(
            unique_key='id || ' ~ "'-'" ~ ' || first_name',
        )
    }}

    select * from {{target.database}}.{{target.schema}}.seed

{% endsnapshot %}
"""

snapshots_yml = """
snapshots:
  - name: snapshot_actual
    config:
      strategy: timestamp
      updated_at: updated_at
      hard_deletes: new_record
"""

ref_snapshot_sql = """
select * from {{ ref('snapshot_actual') }}
"""


invalidate_sql = """
-- update records 11 - 21. Change email and updated_at field
update {schema}.seed set
    updated_at = updated_at + interval '1 hour',
    email      =  case when id = 20 then 'pfoxj@creativecommons.org' else 'new_' || email end
where id >= 10 and id <= 20;


-- invalidate records 11 - 21
update {schema}.snapshot_expected set
    dbt_valid_to   = updated_at + interval '1 hour'
where id >= 10 and id <= 20;

"""

update_sql = """
-- insert v2 of the 11 - 21 records

insert into {database}.{schema}.snapshot_expected (
    id,
    first_name,
    last_name,
    email,
    gender,
    ip_address,
    updated_at,
    dbt_valid_from,
    dbt_valid_to,
    dbt_updated_at,
    dbt_scd_id,
    dbt_is_deleted
)

select
    id,
    first_name,
    last_name,
    email,
    gender,
    ip_address,
    updated_at,
    -- fields added by snapshotting
    updated_at as dbt_valid_from,
    null::timestamp as dbt_valid_to,
    updated_at as dbt_updated_at,
    md5(id || '-' || first_name || '|' || updated_at::text) as dbt_scd_id,
    'False' as dbt_is_deleted
from {database}.{schema}.seed
where id >= 10 and id <= 20;
"""

delete_sql = """
delete from {schema}.seed where id = 1
"""


class TestSnapshotNewRecordMode:
    @pytest.fixture(scope="class")
    def snapshots(self):
        return {"snapshot.sql": snapshot_actual_sql}

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "snapshots.yml": snapshots_yml,
            "ref_snapshot.sql": ref_snapshot_sql,
        }

    def test_snapshot_new_record_mode(self, project):
        path = os.path.join(project.test_data_dir, "seed_new_record_mode.sql")
        project.run_sql_file(path)
        results = run_dbt(["snapshot"])
        assert len(results) == 1

        project.run_sql(invalidate_sql)
        project.run_sql(update_sql)

        results = run_dbt(["snapshot"])
        assert len(results) == 1

        check_relations_equal(project.adapter, ["snapshot_actual", "snapshot_expected"])

        project.run_sql(delete_sql)

        results = run_dbt(["snapshot"])
        assert len(results) == 1

        # TODO: Further validate results.
