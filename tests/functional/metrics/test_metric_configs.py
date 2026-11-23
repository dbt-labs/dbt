import pytest

from dbt.artifacts.resources import MetricConfig
from dbt.events.types import ValidationWarning
from dbt.exceptions import CompilationError, ParsingError
from dbt.tests.util import get_manifest, run_dbt, update_config_file
from dbt_common.dataclass_schema import ValidationError
from dbt_common.events.event_catcher import EventCatcher
from tests.functional.metrics.fixtures import (
    disabled_metric_level_schema_yml,
    enabled_metric_level_schema_yml,
    invalid_config_metric_yml,
    metricflow_time_spine_sql,
    metricflow_time_spine_yml,
    models_people_metrics_meta_top_yml,
    models_people_metrics_sql,
    models_people_metrics_shared_tag_yml,
    models_people_metrics_tags_yml,
    models_people_metrics_top_level_tags_yml,
    models_people_metrics_yml,
    models_people_sql,
    semantic_model_people_yml,
)


class MetricConfigTests:
    @pytest.fixture(scope="class", autouse=True)
    def setUp(self):
        pytest.expected_config = MetricConfig(
            enabled=True,
        )


# Test enabled config in dbt_project.yml
class TestMetricEnabledConfigProjectLevel(MetricConfigTests):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "time_spine.yml": metricflow_time_spine_yml,
            "semantic_model_people.yml": semantic_model_people_yml,
            "schema.yml": models_people_metrics_yml,
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "metrics": {
                "test": {
                    "average_tenure_minus_people": {
                        "enabled": False,
                    },
                }
            }
        }

    def test_enabled_metric_config_dbt_project(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        assert "metric.test.average_tenure_minus_people" not in manifest.metrics

        new_enabled_config = {
            "metrics": {
                "test": {
                    "average_tenure_minus_people": {
                        "enabled": True,
                    },
                }
            }
        }
        update_config_file(new_enabled_config, project.project_root, "dbt_project.yml")
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        assert "metric.test.average_tenure_minus_people" in manifest.metrics
        assert "metric.test.collective_tenure" in manifest.metrics


# Test enabled config at metrics level in yml file
class TestConfigYamlMetricLevel(MetricConfigTests):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "time_spine.yml": metricflow_time_spine_yml,
            "semantic_model_people.yml": semantic_model_people_yml,
            "schema.yml": disabled_metric_level_schema_yml,
        }

    def test_metric_config_yaml_metric_level(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        assert "metric.test.number_of_people" not in manifest.metrics
        assert "metric.test.collective_tenure" in manifest.metrics


# Test inheritence - set configs at project and metric level - expect metric level to win
class TestMetricConfigsInheritence(MetricConfigTests):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "time_spine.yml": metricflow_time_spine_yml,
            "semantic_model_people.yml": semantic_model_people_yml,
            "schema.yml": enabled_metric_level_schema_yml,
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"metrics": {"enabled": False}}

    def test_metrics_all_configs(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        # This should be overridden
        assert "metric.test.number_of_people" in manifest.metrics
        # This should stay disabled
        assert "metric.test.collective_tenure" not in manifest.metrics

        config_test_table = manifest.metrics.get("metric.test.number_of_people").config

        assert isinstance(config_test_table, MetricConfig)
        assert config_test_table == pytest.expected_config


# Test CompilationError if a model references a disabled metric
class TestDisabledMetricRef(MetricConfigTests):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "time_spine.yml": metricflow_time_spine_yml,
            "semantic_model_people.yml": semantic_model_people_yml,
            "people_metrics.sql": models_people_metrics_sql,
            "schema.yml": models_people_metrics_yml,
        }

    def test_disabled_metric_ref_model(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        assert "metric.test.number_of_people" in manifest.metrics
        assert "metric.test.collective_tenure" in manifest.metrics
        assert "model.test.people_metrics" in manifest.nodes
        assert "metric.test.average_tenure" in manifest.metrics
        assert "metric.test.average_tenure_minus_people" in manifest.metrics

        new_enabled_config = {
            "metrics": {
                "test": {
                    "number_of_people": {
                        "enabled": False,
                    },
                    "average_tenure_minus_people": {
                        "enabled": False,
                    },
                    "average_tenure": {
                        "enabled": False,
                    },
                }
            }
        }

        update_config_file(new_enabled_config, project.project_root, "dbt_project.yml")
        with pytest.raises(CompilationError):
            run_dbt(["parse"])


# Test invalid metric configs
class TestInvalidMetric(MetricConfigTests):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "time_spine.yml": metricflow_time_spine_yml,
            "semantic_model_people.yml": semantic_model_people_yml,
            "schema.yml": invalid_config_metric_yml,
        }

    def test_invalid_config_metric(self, project):
        with pytest.raises(ValidationError) as excinfo:
            run_dbt(["parse"])
        expected_msg = "'True and False' is not of type 'boolean'"
        assert expected_msg in str(excinfo.value)


class TestDisabledMetric(MetricConfigTests):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "time_spine.yml": metricflow_time_spine_yml,
            "semantic_model_people.yml": semantic_model_people_yml,
            "schema.yml": models_people_metrics_yml,
        }

    def test_disabling_upstream_metric_errors(self, project):
        run_dbt(["parse"])  # shouldn't error out yet

        new_enabled_config = {
            "metrics": {
                "test": {
                    "number_of_people": {
                        "enabled": False,
                    },
                }
            }
        }

        update_config_file(new_enabled_config, project.project_root, "dbt_project.yml")
        with pytest.raises(ParsingError) as excinfo:
            run_dbt(["parse"])
            expected_msg = (
                "The metric `number_of_people` is disabled and thus cannot be referenced."
            )
            assert expected_msg in str(excinfo.value)


# Test meta config in dbt_project.yml
class TestMetricMetaConfigProjectLevel(MetricConfigTests):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "time_spine.yml": metricflow_time_spine_yml,
            "semantic_model_people.yml": semantic_model_people_yml,
            "schema.yml": models_people_metrics_yml,
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "metrics": {
                "test": {
                    "average_tenure_minus_people": {
                        "+meta": {"project_field": "project_value"},
                    },
                }
            }
        }

    def test_meta_metric_config_dbt_project(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        assert "metric.test.average_tenure_minus_people" in manifest.metrics
        # for backwards compatibility the config level meta gets copied to the top level meta
        assert manifest.metrics.get("metric.test.average_tenure_minus_people").config.meta == {
            "project_field": "project_value"
        }
        assert manifest.metrics.get("metric.test.average_tenure_minus_people").meta == {
            "project_field": "project_value"
        }


# Test setting config at config level
class TestMetricMetaConfigLevel(MetricConfigTests):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "time_spine.yml": metricflow_time_spine_yml,
            "semantic_model_people.yml": semantic_model_people_yml,
            "schema.yml": models_people_metrics_yml,
        }

    def test_meta_metric_config_yaml(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        assert "metric.test.number_of_people" in manifest.metrics
        assert manifest.metrics.get("metric.test.number_of_people").config.meta == {
            "my_meta_config": "config"
        }
        assert manifest.metrics.get("metric.test.number_of_people").meta == {
            "my_meta_config": "config"
        }


# Test setting config at metric level- expect to exist in config after parsing
class TestMetricMetaTopLevel(MetricConfigTests):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "time_spine.yml": metricflow_time_spine_yml,
            "semantic_model_people.yml": semantic_model_people_yml,
            "schema.yml": models_people_metrics_meta_top_yml,
        }

    def test_meta_metric_config_yaml(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        assert "metric.test.number_of_people" in manifest.metrics
        # for backwards compatibility the config level meta gets copied to the top level meta
        assert manifest.metrics.get("metric.test.number_of_people").config.meta != {
            "my_meta_top": "top"
        }
        assert manifest.metrics.get("metric.test.number_of_people").meta == {"my_meta_top": "top"}


# Test tags config in dbt_project.yml
class TestMetricTagsConfigProjectLevel(MetricConfigTests):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "semantic_model_people.yml": semantic_model_people_yml,
            "schema.yml": models_people_metrics_yml,
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "metrics": {
                "test": {
                    "+tags": ["project_tag"],
                }
            }
        }

    def test_tags_metric_config_dbt_project(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        for metric_id in [
            "metric.test.number_of_people",
            "metric.test.collective_tenure",
            "metric.test.average_tenure",
            "metric.test.average_tenure_minus_people",
        ]:
            metric = manifest.metrics.get(metric_id)
            assert metric is not None, f"{metric_id} not found in manifest"
            assert isinstance(metric.tags, list), f"{metric_id}.tags should be a list"
            assert "project_tag" in metric.tags, f"{metric_id} missing project_tag in tags"
            assert "project_tag" in metric.config.tags, f"{metric_id} missing project_tag in config.tags"


# Test tags config in yaml config block
class TestMetricTagsConfigYamlLevel(MetricConfigTests):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "semantic_model_people.yml": semantic_model_people_yml,
            "schema.yml": models_people_metrics_tags_yml,
        }

    def test_tags_metric_config_yaml(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        metric = manifest.metrics.get("metric.test.number_of_people")
        assert metric is not None
        assert isinstance(metric.tags, list)
        assert metric.tags == ["yaml_tag"]
        assert metric.config.tags == ["yaml_tag"]


# Test tags merging from both dbt_project.yml and yaml config block
class TestMetricTagsConfigMerge(MetricConfigTests):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "semantic_model_people.yml": semantic_model_people_yml,
            "schema.yml": models_people_metrics_tags_yml,
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "metrics": {
                "test": {
                    "+tags": ["project_tag"],
                }
            }
        }

    def test_tags_merge(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        metric = manifest.metrics.get("metric.test.number_of_people")
        assert metric is not None
        assert isinstance(metric.tags, list)
        assert "project_tag" in metric.tags
        assert "yaml_tag" in metric.tags
        assert "project_tag" in metric.config.tags
        assert "yaml_tag" in metric.config.tags

    def test_tags_deduplicated(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        metric = manifest.metrics.get("metric.test.number_of_people")
        assert metric is not None
        assert len(metric.tags) == len(set(metric.tags)), "Duplicate tags found in metric.tags"
        assert len(metric.config.tags) == len(set(metric.config.tags))


# Test that when the same tag appears in both +tags (dbt_project.yml) and config.tags (YAML),
# it appears only once after normalization.
class TestMetricTagsConfigDeduplication(MetricConfigTests):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "semantic_model_people.yml": semantic_model_people_yml,
            "schema.yml": models_people_metrics_shared_tag_yml,
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "metrics": {
                "test": {
                    "+tags": ["shared_tag"],
                }
            }
        }

    def test_same_tag_deduplicated(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        metric = manifest.metrics.get("metric.test.number_of_people")
        assert metric is not None
        assert metric.tags == ["shared_tag"], f"Expected ['shared_tag'], got {metric.tags}"
        assert metric.config.tags == ["shared_tag"]


class TestMetricTopLevelTagsWarning(MetricConfigTests):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "semantic_model_people.yml": semantic_model_people_yml,
            "schema.yml": models_people_metrics_top_level_tags_yml,
        }

    def test_top_level_tags_emits_warning_and_tags_are_ignored(self, project):
        catcher = EventCatcher(event_to_catch=ValidationWarning)
        run_dbt(["parse"], callbacks=[catcher.catch])
        manifest = get_manifest(project.project_root)
        metric = manifest.metrics.get("metric.test.number_of_people")
        assert metric is not None
        assert "top_level_tag" not in metric.tags
        assert "top_level_tag" not in metric.config.tags
        assert len(catcher.caught_events) >= 1
        warning_messages = [str(e.data) for e in catcher.caught_events]
        assert any(
            "top-level" in msg and "number_of_people" in msg and "ignored" in msg
            for msg in warning_messages
        )
