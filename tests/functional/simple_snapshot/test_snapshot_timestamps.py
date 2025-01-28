import pytest

from dbt.tests.util import run_dbt, run_dbt_and_capture

create_source_sql = """
create table {database}.{schema}.source_users (
    id INTEGER,
    first_name VARCHAR(50),
    last_name VARCHAR(50),
    email VARCHAR(50),
    gender VARCHAR(50),
    ip_address VARCHAR(20),
    updated_time TIMESTAMP WITH TIME ZONE
);
insert into {database}.{schema}.source_users (id, first_name, last_name, email, gender, ip_address, updated_time) values
(1, 'Judith', 'Kennedy', '(not provided)', 'Female', '54.60.24.128', '2015-12-24 12:19:28'),
(2, 'Arthur', 'Kelly', '(not provided)', 'Male', '62.56.24.215', '2015-10-28 16:22:15'),
(3, 'Rachel', 'Moreno', 'rmoreno2@msu.edu', 'Female', '31.222.249.23', '2016-04-05 02:05:30');
"""

model_users_sql = """
select * from {{ source('test_source', 'source_users') }}
"""

snapshot_sql = """
{% snapshot users_snapshot %}

select * from {{ ref('users') }}

{% endsnapshot %}
"""

source_schema_yml = """
sources:
  - name: test_source
    loader: custom
    schema: "{{ target.schema }}"
    tables:
      - name: source_users
        loaded_at_field: updated_time
"""

snapshot_schema_yml = """
snapshots:
  - name: users_snapshot
    config:
      target_schema: "{{ target.schema }}"
      strategy: timestamp
      unique_key: id
      updated_at: updated_time
"""


class TestSnapshotConfig:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "users.sql": model_users_sql,
            "source_schema.yml": source_schema_yml,
            "snapshot_schema.yml": snapshot_schema_yml,
        }

    @pytest.fixture(scope="class")
    def snapshots(self):
        return {"snapshot.sql": snapshot_sql}

    def test_timestamp_snapshot(self, project):
        project.run_sql(create_source_sql)
        run_dbt(["run"])
        results, log_output = run_dbt_and_capture(["snapshot"])
        assert len(results) == 1
        assert "Please update snapshot config" in log_output
