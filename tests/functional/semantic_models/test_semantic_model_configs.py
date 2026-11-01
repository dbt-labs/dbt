import pytest

from dbt.artifacts.resources import SemanticModelConfig
from dbt.exceptions import ParsingError
from dbt.tests.util import get_manifest, run_dbt, update_config_file
from tests.functional.semantic_models.fixtures import (
    disabled_models_people_metrics_yml,
    disabled_semantic_model_people_yml,
    enabled_semantic_model_people_yml,
    groups_yml,
    metricflow_time_spine_sql,
    models_people_metrics_yml,
    models_people_sql,
    semantic_model_dimensions_entities_measures_meta_config,
    semantic_model_meta_clobbering_yml,
    semantic_model_people_yml,
    base_schema_yml_v2,
    fct_revenue_sql,
    schema_yml_v2_metric_with_config_tags,
)


# Test disabled config at semantic_models level in yaml file
class TestConfigYamlLevel:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "semantic_models.yml": disabled_semantic_model_people_yml,
            "people_metrics.yml": disabled_models_people_metrics_yml,
            "groups.yml": groups_yml,
        }

    def test_yaml_level(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        assert "semantic_model.test.semantic_people" not in manifest.semantic_models
        assert "semantic_model.test.semantic_people" in manifest.disabled

        assert "group.test.some_group" in manifest.groups
        assert "semantic_model.test.semantic_people" not in manifest.groups


# Test disabled config at semantic_models level with a still enabled metric
class TestDisabledConfigYamlLevelEnabledMetric:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "semantic_models.yml": disabled_semantic_model_people_yml,
            "people_metrics.yml": models_people_metrics_yml,
            "groups.yml": groups_yml,
        }

    def test_yaml_level(self, project):
        with pytest.raises(
            ParsingError,
            match="The measure `people` is referenced on disabled semantic model `semantic_people`.",
        ):
            run_dbt(["parse"])


# Test disabling semantic model config but not metric config in dbt_project.yml
class TestMismatchesConfigProjectLevel:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "semantic_models.yml": semantic_model_people_yml,
            "people_metrics.yml": models_people_metrics_yml,
            "groups.yml": groups_yml,
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "semantic-models": {
                "test": {
                    "enabled": True,
                }
            }
        }

    def test_project_level(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        assert "semantic_model.test.semantic_people" in manifest.semantic_models
        assert "group.test.some_group" in manifest.groups
        assert manifest.semantic_models["semantic_model.test.semantic_people"].group is None

        new_enabled_config = {
            "semantic-models": {
                "test": {
                    "enabled": False,
                }
            }
        }
        update_config_file(new_enabled_config, project.project_root, "dbt_project.yml")
        with pytest.raises(
            ParsingError,
            match="The measure `people` is referenced on disabled semantic model `semantic_people`.",
        ):
            run_dbt(["parse"])


# Test disabling semantic model and metric configs in dbt_project.yml
class TestConfigProjectLevel:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "semantic_models.yml": semantic_model_people_yml,
            "people_metrics.yml": models_people_metrics_yml,
            "groups.yml": groups_yml,
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "semantic-models": {
                "test": {
                    "enabled": True,
                }
            },
            "metrics": {
                "test": {
                    "enabled": True,
                }
            },
        }

    def test_project_level(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        assert "semantic_model.test.semantic_people" in manifest.semantic_models
        assert "group.test.some_group" in manifest.groups
        assert "group.test.some_other_group" in manifest.groups
        assert manifest.semantic_models["semantic_model.test.semantic_people"].group is None

        new_group_config = {
            "semantic-models": {
                "test": {
                    "group": "some_other_group",
                }
            },
        }
        update_config_file(new_group_config, project.project_root, "dbt_project.yml")
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)

        assert "semantic_model.test.semantic_people" in manifest.semantic_models
        assert "group.test.some_other_group" in manifest.groups
        assert "group.test.some_group" in manifest.groups
        assert (
            manifest.semantic_models["semantic_model.test.semantic_people"].group
            == "some_other_group"
        )

        new_enabled_config = {
            "semantic-models": {
                "test": {
                    "enabled": False,
                }
            },
            "metrics": {
                "test": {
                    "enabled": False,
                }
            },
        }
        update_config_file(new_enabled_config, project.project_root, "dbt_project.yml")
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)

        assert "semantic_model.test.semantic_people" not in manifest.semantic_models
        assert "semantic_model.test.semantic_people" in manifest.disabled

        assert "group.test.some_group" in manifest.groups
        assert "semantic_model.test.semantic_people" not in manifest.groups


# Test inheritence - set configs at project and semantic_model level - expect semantic_model level to win
class TestConfigsInheritence:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "semantic_models.yml": enabled_semantic_model_people_yml,
            "people_metrics.yml": models_people_metrics_yml,
            "groups.yml": groups_yml,
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"semantic-models": {"enabled": False}}

    def test_project_plus_yaml_level(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        assert "semantic_model.test.semantic_people" in manifest.semantic_models
        config_test_table = manifest.semantic_models.get(
            "semantic_model.test.semantic_people"
        ).config

        assert isinstance(config_test_table, SemanticModelConfig)


# test setting meta attributes in semantic model config
class TestMetaConfig:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "semantic_models.yml": enabled_semantic_model_people_yml,
            "people_metrics.yml": models_people_metrics_yml,
            "groups.yml": groups_yml,
        }

    def test_meta_config(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        sm_id = "semantic_model.test.semantic_people"
        assert sm_id in manifest.semantic_models
        sm_node = manifest.semantic_models[sm_id]
        meta_expected = {"my_meta": "testing", "my_other_meta": "testing more"}
        assert sm_node.config.meta == meta_expected


# test meta configs on semantic model components (dimensions, measures, entities)
class TestMetaConfigForComponents:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "semantic_models.yml": semantic_model_dimensions_entities_measures_meta_config,
            "people_metrics.yml": models_people_metrics_yml,
            "groups.yml": groups_yml,
        }

    def test_component_meta_configs(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        sm_id = "semantic_model.test.semantic_people"
        assert sm_id in manifest.semantic_models
        sm_node = manifest.semantic_models[sm_id]

        # Check dimension meta config
        favorite_color_dim = next(d for d in sm_node.dimensions if d.name == "favorite_color")
        assert favorite_color_dim.config.meta == {"dimension": "one"}

        # Check measure meta config
        years_tenure_measure = next(m for m in sm_node.measures if m.name == "years_tenure")
        assert years_tenure_measure.config.meta == {"measure": "two"}

        # Check entity meta config
        id_entity = next(e for e in sm_node.entities if e.name == "id")
        assert id_entity.config.meta == {"entity": "three"}


# test meta config clobbering behavior between semantic model and component levels
class TestMetaConfigClobbering:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "people.sql": models_people_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "semantic_models.yml": semantic_model_meta_clobbering_yml,
            "people_metrics.yml": models_people_metrics_yml,
            "groups.yml": groups_yml,
        }

    def test_meta_config_clobbering(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        sm_id = "semantic_model.test.semantic_people"
        assert sm_id in manifest.semantic_models
        sm_node = manifest.semantic_models[sm_id]

        # Check semantic model level meta config
        assert sm_node.config.meta == {
            "model_level": "should_be_inherited",
            "component_level": "should_be_overridden",
        }

        # Check dimension inherits model-level meta and overrides component-level meta
        favorite_color_dim = next(d for d in sm_node.dimensions if d.name == "favorite_color")
        assert favorite_color_dim.config.meta == {
            "model_level": "should_be_inherited",
            "component_level": "dimension_override",
        }

        # Check measure inherits model-level meta and overrides component-level meta
        years_tenure_measure = next(m for m in sm_node.measures if m.name == "years_tenure")
        assert years_tenure_measure.config.meta == {
            "model_level": "should_be_inherited",
            "component_level": "measure_override",
        }

        # Check entity inherits model-level meta and overrides component-level meta
        id_entity = next(e for e in sm_node.entities if e.name == "id")
        assert id_entity.config.meta == {
            "model_level": "should_be_inherited",
            "component_level": "entity_override",
        }

        # Check component without meta config still inherits model-level meta
        created_at_dim = next(d for d in sm_node.dimensions if d.name == "created_at")
        assert created_at_dim.config.meta == {
            "model_level": "should_be_inherited",
            "component_level": "should_be_overridden",
        }


class TestV2MetricTagsConfig:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "schema.yml": base_schema_yml_v2 + schema_yml_v2_metric_with_config_tags,
        }

    def test_v2_metric_config_tags(self, project):
        run_dbt(["parse"])
        manifest = get_manifest(project.project_root)
        metric = manifest.metrics.get("metric.test.simple_metric_with_tag")
        assert metric is not None, "V2 metric 'simple_metric_with_tag' not found in manifest"
        assert isinstance(metric.tags, list)
        assert metric.tags == ["v2_yaml_tag"], f"Expected ['v2_yaml_tag'], got {metric.tags}"
        assert metric.config.tags == ["v2_yaml_tag"]

