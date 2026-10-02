import os

import pytest

from dbt.exceptions import ParsingError, TargetNotFoundError
from dbt.tests.util import get_manifest, run_dbt, write_file
from tests.functional.exposures.dimension_fixtures import (
    customers_sql,
    exposure_yml,
    metricflow_time_spine_sql,
    orders_sql,
    osi_document_json,
    semantic_models_yml,
    time_spine_with_custom_granularity_yml,
    v2_inline_schema_yml,
)

CUSTOMERS_SM = "semantic_model.test.customers_sm"
ORDERS_SM = "semantic_model.test.orders_sm"


def semantic_layer_files(*specifiers):
    return {
        "customers.sql": customers_sql,
        "orders.sql": orders_sql,
        "metricflow_time_spine.sql": metricflow_time_spine_sql,
        "time_spines.yml": time_spine_with_custom_granularity_yml,
        "semantic_models.yml": semantic_models_yml,
        "exposure.yml": exposure_yml(*specifiers),
    }


class TestExposureDimensionResolution:
    @pytest.fixture(scope="class")
    def models(self):
        files = semantic_layer_files("customer__country")
        files["exposure.yml"] = files["exposure.yml"].replace(
            "      - dimension('customer__country')",
            "      - ref('customers')\n"
            "      - dimension('customer__country')\n"
            "      - dimension('order__ordered_at__fiscal_quarter')",
        )
        return files

    def test_dimensions_resolve_into_lineage(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        manifest.build_parent_and_child_maps()
        exposure = manifest.exposures["exposure.test.dashboard"]

        assert exposure.dimensions == ["customer__country", "order__ordered_at__fiscal_quarter"]
        assert sorted(exposure.depends_on.nodes) == [
            "model.test.customers",
            CUSTOMERS_SM,
            ORDERS_SM,
        ]
        assert "exposure.test.dashboard" in manifest.child_map[CUSTOMERS_SM]
        assert "exposure.test.dashboard" in manifest.child_map[ORDERS_SM]


class TestUnknownDimension:
    @pytest.fixture(scope="class")
    def models(self):
        return semantic_layer_files("customer__does_not_exist")

    def test_parse_fails(self, project):
        with pytest.raises(TargetNotFoundError, match="depends on a dimension named"):
            run_dbt(["parse"])


class TestMetricTimeDimension:
    @pytest.fixture(scope="class")
    def models(self):
        return semantic_layer_files("metric_time__month")

    def test_parse_fails_naming_the_exposure(self, project):
        with pytest.raises(ParsingError, match="'metric_time' is not defined by any") as excinfo:
            run_dbt(["parse"])
        assert "exposure.yml" in str(excinfo.value)


class TestExposureDimensionsWithV2InlineSemanticModel:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "customers.sql": customers_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "schema.yml": v2_inline_schema_yml,
            "exposure.yml": exposure_yml("customer__country", "customer__signed_up_at__month"),
        }

    def test_resolves_to_inline_semantic_model(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        manifest.build_parent_and_child_maps()
        exposure = manifest.exposures["exposure.test.dashboard"]
        assert exposure.depends_on.nodes == ["semantic_model.test.customers"]
        assert "exposure.test.dashboard" in manifest.child_map["semantic_model.test.customers"]


class TestExposureDimensionsWithOsiSemanticModel:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "customers.sql": customers_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "exposure.yml": exposure_yml(
                "customer_id__country", "customer_id__signed_up_at__month"
            ),
        }

    @pytest.fixture(scope="class", autouse=True)
    def osi_document(self, project):
        source = f"{project.database}.{project.test_schema}.customers"
        osi_dir = os.path.join(project.project_root, "osi")
        os.makedirs(osi_dir, exist_ok=True)
        write_file(osi_document_json % {"source": source}, osi_dir, "customers.json")

    def test_resolves_to_osi_semantic_model(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        manifest.build_parent_and_child_maps()
        exposure = manifest.exposures["exposure.test.dashboard"]
        assert exposure.depends_on.nodes == ["semantic_model.test.osi_customers"]
        assert "exposure.test.dashboard" in manifest.child_map["semantic_model.test.osi_customers"]
        assert exposure.dimensions == [
            "customer_id__country",
            "customer_id__signed_up_at__month",
        ]
