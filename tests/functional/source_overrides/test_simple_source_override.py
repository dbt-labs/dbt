from datetime import timezone, datetime, timedelta
import pytest

from dbt.tests.util import run_dbt, update_config_file, check_relations_equal
from dbt.tests.fixtures.project import write_project_files
from tests.functional.source_overrides.fixtures import (  # noqa: F401
    local_dependency,
    models__schema_yml,
    seeds__expected_result_csv,
    seeds__my_real_other_seed_csv,
    seeds__my_real_seed_csv,
)


class TestSourceOverride:
    @pytest.fixture(scope="class", autouse=True)
    def setUp(self, project_root, local_dependency):  # noqa: F811
        write_project_files(project_root, "local_dependency", local_dependency)

    @pytest.fixture(scope="class")
    def models(self):
        return {"schema.yml": models__schema_yml}

    @pytest.fixture(scope="class")
    def seeds(self):
        return {
            "expected_result.csv": seeds__expected_result_csv,
            "my_real_other_seed.csv": seeds__my_real_other_seed_csv,
            "my_real_seed.csv": seeds__my_real_seed_csv,
        }

    @pytest.fixture(scope="class")
    def packages(self):
        return {
            "packages": [
                {
                    "local": "local_dependency",
                },
            ]
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "seeds": {
                "localdep": {
                    "enabled": False,
                    "keep": {
                        "enabled": True,
                    },
                },
                "quote_columns": False,
            },
            "sources": {
                "localdep": {
                    "my_other_source": {
                        "enabled": False,
                    }
                }
            },
        }

    def _set_updated_at_to(self, insert_id, delta, project):
        insert_time = datetime.now(timezone.utc).replace(tzinfo=None) + delta
        timestr = insert_time.strftime("%Y-%m-%d %H:%M:%S")
        # favorite_color,id,first_name,email,ip_address,updated_at

        quoted_columns = ",".join(
            project.adapter.quote(c)
            for c in ("favorite_color", "id", "first_name", "email", "ip_address", "updated_at")
        )

        kwargs = {
            "schema": project.test_schema,
            "time": timestr,
            "id": insert_id,
            "source": project.adapter.quote("snapshot_freshness_base"),
            "quoted_columns": quoted_columns,
        }

        raw_code = """INSERT INTO {schema}.{source}
            ({quoted_columns})
        VALUES (
            'blue',{id},'Jake','abc@example.com','192.168.1.1','{time}'
        )""".format(
            **kwargs
        )

        project.run_sql(raw_code)

        return insert_id + 1

    def test_source_overrides(self, project):
        insert_id = 101

        run_dbt(["deps"])

        seed_results = run_dbt(["seed"])
        assert len(seed_results) == 5

        # There should be 7, as we disabled 1 test of the original 8
        test_results = run_dbt(["test"])
        assert len(test_results) == 7

        results = run_dbt(["run"])
        assert len(results) == 1

        check_relations_equal(project.adapter, ["expected_result", "my_model"])

        # set the updated_at field of this seed to last week
        insert_id = self._set_updated_at_to(insert_id, timedelta(days=-7), project)
        # if snapshot-freshness fails, freshness just didn't happen!
        results = run_dbt(["source", "snapshot-freshness"], expect_pass=False)
        # we disabled my_other_source, so we only run the one freshness check
        # in
        assert len(results) == 1
        # If snapshot-freshness passes, that means error_after was
        # applied from the source override but not the source table override
        insert_id = self._set_updated_at_to(insert_id, timedelta(days=-2), project)
        results = run_dbt(
            ["source", "snapshot-freshness"],
            expect_pass=False,
        )
        assert len(results) == 1

        insert_id = self._set_updated_at_to(insert_id, timedelta(hours=-12), project)
        results = run_dbt(["source", "snapshot-freshness"], expect_pass=True)
        assert len(results) == 1

        # update source to be enabled
        new_source_config = {
            "sources": {
                "localdep": {
                    "my_other_source": {
                        "enabled": True,
                    }
                }
            }
        }
        update_config_file(new_source_config, project.project_root, "dbt_project.yml")

        # enable my_other_source, snapshot freshness should fail due to the new
        # not-fresh source
        results = run_dbt(["source", "snapshot-freshness"], expect_pass=False)
        assert len(results) == 2
