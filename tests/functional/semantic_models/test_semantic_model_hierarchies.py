import json
import os
from typing import List

import pytest

from dbt.artifacts.resources import DimensionHierarchy, DimensionHierarchyLevel
from dbt.contracts.graph.manifest import Manifest
from dbt.tests.util import run_dbt
from dbt_common.events.base_types import BaseEvent
from tests.functional.assertions.test_runner import dbtTestRunner
from tests.functional.semantic_models.fixtures import simple_metricflow_time_spine_sql

employees_sql = """
select
  1 as id,
  10 as store_id,
  'e1' as employee_code,
  cast(null as text) as manager_code,
  current_timestamp as created_at
"""

stores_sql = """
select
  10 as id,
  'US' as country,
  'West' as sales_region,
  'Seattle' as city
"""

semantic_model_hierarchies_yml = """
version: 2

semantic_models:
  - name: employees
    model: ref('employees')
    entities:
      - name: employee
        type: primary
        expr: id
      - name: store
        type: foreign
        expr: store_id
    dimensions:
      - name: employee_code
        type: categorical
      - name: manager_code
        type: categorical
      - name: created_at
        type: time
        type_params:
          time_granularity: day
    measures:
      - name: headcount
        agg: count
        expr: id
        agg_time_dimension: created_at
        create_metric: true
    hierarchies:
      - name: sales_geography
        levels: [store__country, store__sales_region, store__city]
      - name: reporting_line
        levels:
          - dimension: employee_code
            parent: manager_code

  - name: stores
    model: ref('stores')
    entities:
      - name: store
        type: primary
        expr: id
    dimensions:
      - name: country
        type: categorical
      - name: sales_region
        type: categorical
      - name: city
        type: categorical
"""

semantic_model_hierarchies_without_metrics_yml = semantic_model_hierarchies_yml.replace(
    "create_metric: true", "create_metric: false"
)

people_without_hierarchies_yml = """
version: 2

semantic_models:
  - name: people
    model: ref('stores')
    entities:
      - name: store
        type: primary
        expr: id
    dimensions:
      - name: city
        type: categorical
"""


class TestSemanticModelHierarchies:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "employees.sql": employees_sql,
            "stores.sql": stores_sql,
            "metricflow_time_spine.sql": simple_metricflow_time_spine_sql,
            "semantic_models.yml": semantic_model_hierarchies_yml,
        }

    def test_hierarchies_parsed(self, project):
        manifest = run_dbt(["parse"])
        assert isinstance(manifest, Manifest)

        semantic_model = manifest.semantic_models["semantic_model.test.employees"]
        assert semantic_model.hierarchies == [
            DimensionHierarchy(
                name="sales_geography",
                levels=[
                    DimensionHierarchyLevel(dimension="store__country"),
                    DimensionHierarchyLevel(dimension="store__sales_region"),
                    DimensionHierarchyLevel(dimension="store__city"),
                ],
            ),
            DimensionHierarchy(
                name="reporting_line",
                levels=[DimensionHierarchyLevel(dimension="employee_code", parent="manager_code")],
            ),
        ]

        with open(os.path.join(project.project_root, "target", "semantic_manifest.json")) as fp:
            semantic_manifest = json.load(fp)
        employees = next(
            sm for sm in semantic_manifest["semantic_models"] if sm["name"] == "employees"
        )
        assert employees["hierarchies"] == [
            {
                "name": "sales_geography",
                "levels": [
                    {"dimension": "store__country", "parent": None},
                    {"dimension": "store__sales_region", "parent": None},
                    {"dimension": "store__city", "parent": None},
                ],
            },
            {
                "name": "reporting_line",
                "levels": [{"dimension": "employee_code", "parent": "manager_code"}],
            },
        ]


class TestSemanticModelHierarchyWithUnresolvableLevel:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "employees.sql": employees_sql,
            "stores.sql": stores_sql,
            "metricflow_time_spine.sql": simple_metricflow_time_spine_sql,
            "semantic_models.yml": semantic_model_hierarchies_yml.replace(
                "store__sales_region", "store__state"
            ),
        }

    def test_unresolvable_level_fails_parse(self, project):
        events: List[BaseEvent] = []
        runner = dbtTestRunner(callbacks=[events.append])
        result = runner.invoke(["parse"])
        assert not result.success

        validation_errors = [e for e in events if e.info.name == "SemanticValidationFailure"]
        assert any(
            "Level `store__state` in hierarchy `sales_geography` does not resolve" in e.info.msg
            for e in validation_errors
        )


class TestSemanticModelHierarchyWithUnresolvableLevelWithoutMetrics:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "employees.sql": employees_sql,
            "stores.sql": stores_sql,
            "metricflow_time_spine.sql": simple_metricflow_time_spine_sql,
            "semantic_models.yml": semantic_model_hierarchies_without_metrics_yml.replace(
                "store__sales_region", "store__state"
            ),
        }

    def test_unresolvable_level_fails_parse(self, project):
        assert "create_metric: true" not in semantic_model_hierarchies_without_metrics_yml
        events: List[BaseEvent] = []
        runner = dbtTestRunner(callbacks=[events.append])
        result = runner.invoke(["parse"])
        assert not result.success

        validation_errors = [e for e in events if e.info.name == "SemanticValidationFailure"]
        assert any(
            "Level `store__state` in hierarchy `sales_geography` does not resolve" in e.info.msg
            for e in validation_errors
        )


class TestSemanticModelsWithoutHierarchiesMetricsOrTimeSpine:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "stores.sql": stores_sql,
            "semantic_models.yml": people_without_hierarchies_yml,
        }

    def test_parse_without_writing_artifacts(self, project):
        result = dbtTestRunner().invoke(["--no-write-json", "parse"])
        assert result.success
