import os

import pytest

from dbt.exceptions import TargetNotFoundError
from dbt.tests.util import get_manifest, rm_file, run_dbt, write_file
from tests.functional.exposures.dimension_fixtures import (
    customers_sql,
    exposure_yml,
    metricflow_time_spine_sql,
    orders_sql,
    osi_document_json,
    semantic_models_yml,
    time_spine_with_custom_granularity_yml,
)

CUSTOMERS_SM = "semantic_model.test.customers_sm"
ORDERS_SM = "semantic_model.test.orders_sm"
EXPOSURE_ID = "exposure.test.dashboard"

customers_only, orders_only = semantic_models_yml.split("\n  - name: orders_sm")
customers_semantic_model_yml = customers_only + "\n"
orders_semantic_model_yml = "version: 2\n\nsemantic_models:\n  - name: orders_sm" + orders_only


def semantic_layer_files(*specifiers):
    return {
        "customers.sql": customers_sql,
        "orders.sql": orders_sql,
        "metricflow_time_spine.sql": metricflow_time_spine_sql,
        "time_spines.yml": time_spine_with_custom_granularity_yml,
        "semantic_models.yml": semantic_models_yml,
        "exposure.yml": exposure_yml(*specifiers),
    }


def split_semantic_layer_files(*specifiers):
    files = semantic_layer_files(*specifiers)
    del files["semantic_models.yml"]
    files["customers_sm.yml"] = customers_semantic_model_yml
    files["orders_sm.yml"] = orders_semantic_model_yml
    return files


def parse_exposure(project):
    run_dbt(["parse"])
    manifest = get_manifest(project.project_root)
    return manifest.exposures[EXPOSURE_ID]


def write_model_file(project, content, name):
    write_file(content, project.project_root, "models", name)


class TestHigherPrioritySemanticModelReplacesUnchangedDependency:
    @pytest.fixture(scope="class")
    def models(self):
        files = split_semantic_layer_files("customer__country")
        files["customers_sm.yml"] = customers_semantic_model_yml.replace(
            "      - name: country\n", "      - name: nation\n"
        )
        files["orders_sm.yml"] = orders_semantic_model_yml.replace(
            "      - name: status\n        type: categorical\n",
            "      - name: status\n        type: categorical\n"
            "      - name: country\n        type: categorical\n",
        )
        return files

    def test_dependency_moves_to_primary_owner(self, project):
        assert parse_exposure(project).depends_on.nodes == [ORDERS_SM]

        write_model_file(project, customers_semantic_model_yml, "customers_sm.yml")
        assert parse_exposure(project).depends_on.nodes == [CUSTOMERS_SM]


class TestDeletingSemanticModelThatOnlyProvidesPathEntityFails:
    @pytest.fixture(scope="class")
    def models(self):
        return split_semantic_layer_files("order__customer__country")

    def test_deletion_then_restore(self, project):
        assert parse_exposure(project).depends_on.nodes == [CUSTOMERS_SM]

        rm_file(project.project_root, "models", "orders_sm.yml")
        with pytest.raises(TargetNotFoundError, match="depends on a dimension named"):
            run_dbt(["parse"])

        write_model_file(project, orders_semantic_model_yml, "orders_sm.yml")
        assert parse_exposure(project).depends_on.nodes == [CUSTOMERS_SM]


class TestDeletingOsiFileFails:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "customers.sql": customers_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "exposure.yml": exposure_yml("customer_id__country"),
        }

    def test_deletion_then_restore(self, project):
        osi_dir = os.path.join(project.project_root, "osi")
        os.makedirs(osi_dir, exist_ok=True)
        source = f"{project.database}.{project.test_schema}.customers"
        write_file(osi_document_json % {"source": source}, osi_dir, "customers.json")
        assert parse_exposure(project).depends_on.nodes == ["semantic_model.test.osi_customers"]

        rm_file(osi_dir, "customers.json")
        with pytest.raises(TargetNotFoundError, match="depends on a dimension named"):
            run_dbt(["parse"])

        write_file(osi_document_json % {"source": source}, osi_dir, "customers.json")
        assert parse_exposure(project).depends_on.nodes == ["semantic_model.test.osi_customers"]


class TestDisablingTimeSpineModelFails:
    @pytest.fixture(scope="class")
    def models(self):
        return semantic_layer_files("order__ordered_at__fiscal_quarter")

    def test_disable_then_restore(self, project):
        assert parse_exposure(project).depends_on.nodes == [ORDERS_SM]

        write_model_file(
            project,
            time_spine_with_custom_granularity_yml.replace(
                "  - name: metricflow_time_spine\n",
                "  - name: metricflow_time_spine\n    config:\n      enabled: false\n",
            ),
            "time_spines.yml",
        )
        with pytest.raises(TargetNotFoundError, match="depends on a dimension named"):
            run_dbt(["parse"])

        write_model_file(project, time_spine_with_custom_granularity_yml, "time_spines.yml")
        assert parse_exposure(project).depends_on.nodes == [ORDERS_SM]
