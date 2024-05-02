import pytest
from dbt.exceptions import DuplicateResourceNameError
from dbt.tests.util import run_dbt, get_manifest, write_file
from tests.functional.exposures.fixtures import (
    models_sql,
    second_model_sql,
    simple_exposure_yml,
    source_schema_yml,
    metrics_schema_yml,
    semantic_models_schema_yml,
    metricflow_time_spine_sql,
    duplicate_exposure_yml,
)


class TestBasicExposures:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "exposure.yml": simple_exposure_yml,
            "model.sql": models_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "second_model.sql": second_model_sql,
            "schema.yml": source_schema_yml,
            "semantic_models.yml": semantic_models_schema_yml,
            "metrics.yml": metrics_schema_yml,
        }

    def test_names_with_spaces(self, project):
        run_dbt(["run"])
        manifest = get_manifest(project.project_root)
        exposure_ids = list(manifest.exposures.keys())
        expected_exposure_ids = [
            "exposure.test.simple_exposure",
            "exposure.test.notebook_exposure",
        ]
        assert exposure_ids == expected_exposure_ids
        assert manifest.exposures["exposure.test.simple_exposure"].label == "simple exposure label"

    def test_depends_on(self, project):
        run_dbt(["run"])
        manifest = get_manifest(project.project_root)
        exposure_depends_on = manifest.exposures["exposure.test.simple_exposure"].depends_on.nodes
        expected_exposure_depends_on = [
            "source.test.test_source.test_table",
            "model.test.model",
            "metric.test.metric",
        ]
        assert sorted(exposure_depends_on) == sorted(expected_exposure_depends_on)

    def test_duplicate_name(self, project):
        results = run_dbt(["run"])
        assert len(results) == 3

        # add a duplicate exposure and run without partial parsing, expect DuplicateResourceNameError
        write_file(
            simple_exposure_yml + duplicate_exposure_yml,
            project.project_root,
            "models",
            "exposure.yml",
        )
        with pytest.raises(DuplicateResourceNameError):
            run_dbt(["run", "--no-partial-parse"])

        # remove duplicate exposure and rerun, expect success
        write_file(simple_exposure_yml, project.project_root, "models", "exposure.yml")
        run_dbt(["run"])
        assert len(results) == 3

        # add back the duplicate exposure and try running with partial parsing
        # right now this doesnt raise an error but probably should
        write_file(
            simple_exposure_yml + duplicate_exposure_yml,
            project.project_root,
            "models",
            "exposure.yml",
        )
        with pytest.raises(DuplicateResourceNameError):
            run_dbt(["run"])
