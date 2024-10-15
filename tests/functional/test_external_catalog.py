import pytest
import yaml
from dbt_config.catalog_config import ExternalCatalog

from dbt.tests.util import run_dbt, write_file


@pytest.fixture(scope="class", autouse=True)
def dbt_catalog_config(project_root):
    config = {
        "catalogs": [
            {
                "name": "my_external_catalog",
                "type": "iceberg",
                "configuration": {
                    "table_format": "iceberg",
                    "catalog_namespace": "dbt",
                    "internal_namespace": {
                        "database": "my_db",
                        "schema": "my_schema",
                    },
                    "external_location": "s3://my-bucket/my-path",
                },
                "management": {
                    "enabled": True,
                    "create_if_not_exists": False,
                    "alter_if_different": False,
                    "read_only": True,
                    "refresh": "on-start",
                },
            }
        ],
    }
    write_file(yaml.safe_dump(config), project_root, "catalog.yml")


class TestCatalogConfig:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "model.sql": "select 1 as id from {{ source('my_external_catalog', 'my_table') }}",
        }

    def test_supplying_external_catalog(self, project):
        manifest = run_dbt(["parse"])
        assert manifest.catalogs != {}
        assert manifest.nodes["model.test.model"].sources == [["my_external_catalog", "my_table"]]
        ExternalCatalog.model_validate_json(manifest.catalogs["my_external_catalog"])
