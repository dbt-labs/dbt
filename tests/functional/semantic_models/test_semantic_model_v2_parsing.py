import pytest
from metricflow_semantic_interfaces.type_enums import (
    AggregationType,
    ConversionCalculationType,
    DimensionType,
    EntityType,
    MetricType,
    PeriodAggregation,
)

from core.dbt.contracts.graph.semantic_manifest import SemanticManifest
from dbt.contracts.graph.manifest import Manifest
from tests.functional.assertions.test_runner import dbtTestRunner
from tests.functional.semantic_models.fixtures import (
    base_schema_yml_v2,
    base_schema_yml_v2_with_custom_sm_name,
    derived_semantics_with_doc_jinja_yml,
    derived_semantics_yml,
    fct_revenue_sql,
    metricflow_time_spine_sql,
    schema_yml_v2_conversion_metric_missing_base_metric,
    schema_yml_v2_cumulative_metric_missing_input_metric,
    schema_yml_v2_metric_with_doc_jinja,
    schema_yml_v2_metric_with_filter_dimension_jinja,
    schema_yml_v2_metric_with_input_metrics_filter_dimension_jinja,
    schema_yml_v2_metric_with_numerator_filter_dimension_jinja,
    schema_yml_v2_metrics_with_hidden,
    schema_yml_v2_simple_metric_on_model_1,
    schema_yml_v2_standalone_metrics,
    schema_yml_v2_standalone_metrics_with_doc_jinja,
    schema_yml_v2_standalone_simple_metric,
    semantic_model_config_does_not_exist,
    semantic_model_descriptions,
    semantic_model_schema_yml_v2,
    semantic_model_schema_yml_v2_default_values,
    semantic_model_schema_yml_v2_disabled,
    semantic_model_schema_yml_v2_entity_without_name,
    semantic_model_schema_yml_v2_false_config,
    semantic_model_schema_yml_v2_primary_entity_only_on_model,
    semantic_model_schema_yml_v2_renamed,
    semantic_model_schema_yml_v2_with_primary_entity_only_on_column,
    semantic_model_test_groups_yml,
    simple_metricflow_time_spine_sql,
    versioned_model_semantic_schema_yml_v2,
    versioned_model_v1_sql,
    versioned_model_v2_sql,
)


class TestSemanticModelParsingWorks:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": semantic_model_schema_yml_v2,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_semantic_model_parsing(self, project) -> None:
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        manifest = result.result
        assert len(manifest.semantic_models) == 1
        semantic_model = manifest.semantic_models["semantic_model.test.fct_revenue"]
        assert semantic_model.name == "fct_revenue"
        assert semantic_model.node_relation.alias == "fct_revenue"
        assert (
            semantic_model.node_relation.relation_name
            == f'"dbt"."{project.test_schema}"."fct_revenue"'
        )
        assert (
            semantic_model.description
            == "This is the model fct_revenue. It should be able to use doc blocks"
        )
        assert semantic_model.config.enabled is True
        assert semantic_model.config.group is None
        assert semantic_model.config.meta == {}

        # Dimensions

        assert len(semantic_model.dimensions) == 3
        dimensions = {dimension.name: dimension for dimension in semantic_model.dimensions}
        id_dim = dimensions["id_dim"]
        assert id_dim.type == DimensionType.CATEGORICAL
        assert id_dim.description == "This is the id column dim."
        assert id_dim.label == "ID Dimension"
        assert id_dim.is_partition is True
        assert id_dim.config.meta == {"component_level": "dimension_override"}
        # dimension name "id_dim" differs from column name "id", so expr must
        # be set to the column name for MetricFlow to query the correct column.
        assert id_dim.expr == "id"
        second_dim = dimensions["second_dim"]
        assert second_dim.type == DimensionType.TIME
        assert second_dim.description == "This is the second column (dim)."
        assert second_dim.label == "Second Dimension"
        assert second_dim.is_partition is False
        assert second_dim.config.meta == {}
        assert second_dim.type_params.validity_params.is_start is True
        assert second_dim.type_params.validity_params.is_end is True
        # dimension name "second_dim" differs from column name "second_col",
        # so expr must be set to the column name.
        assert second_dim.expr == "second_col"
        col_with_default_dimensions = dimensions["col_with_default_dimensions"]
        assert col_with_default_dimensions.type == DimensionType.CATEGORICAL
        assert (
            col_with_default_dimensions.description
            == "This is the column with default dimension settings."
        )
        assert col_with_default_dimensions.label is None
        assert col_with_default_dimensions.is_partition is False
        assert col_with_default_dimensions.config.meta == {}
        assert col_with_default_dimensions.validity_params is None
        # dimension name matches column name, so expr should not be set.
        assert col_with_default_dimensions.expr is None

        # Entities
        assert len(semantic_model.entities) == 3
        entities = {entity.name: entity for entity in semantic_model.entities}
        primary_entity = entities["id_entity"]
        assert primary_entity.type == EntityType.PRIMARY
        assert primary_entity.description == "This is the id entity, and it is the primary entity."
        assert primary_entity.label == "ID Entity"
        assert primary_entity.config.meta == {"component_level": "entity_override"}
        # entity name "id_entity" differs from column name "id", so expr must
        # be set to the column name for MetricFlow to query the correct column.
        assert primary_entity.expr == "id"

        foreign_id_col = entities["foreign_id_col"]
        assert foreign_id_col.type == EntityType.FOREIGN
        assert foreign_id_col.description == "This is a foreign id column."
        assert foreign_id_col.label is None
        assert foreign_id_col.config.meta == {}
        # entity name matches column name, so expr should not be set.
        assert foreign_id_col.expr is None

        col_with_default_entity_testing_default_desc = entities[
            "col_with_default_entity_testing_default_desc"
        ]
        assert col_with_default_entity_testing_default_desc.type == EntityType.NATURAL
        assert (
            col_with_default_entity_testing_default_desc.description
            == "This is the column with default dimension settings."
        )
        assert col_with_default_entity_testing_default_desc.label is None
        assert col_with_default_entity_testing_default_desc.config.meta == {}
        # entity name differs from column name, so expr must be set.
        assert col_with_default_entity_testing_default_desc.expr == "col_with_default_dimensions"

        # No measures in v2 YAML
        assert len(semantic_model.measures) == 0
        assert len(manifest.metrics) == 0
        # TODO: Dimensions are not parsed yet (for those attached to model columns)
        # TODO: Dimensions are not parsed yet (for those defined in derived semantics)


class TestSemanticModelConfigOverrides:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": semantic_model_test_groups_yml + semantic_model_schema_yml_v2_renamed,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_semantic_model_parsing(self, project) -> None:
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        manifest = result.result
        assert len(manifest.semantic_models) == 1
        semantic_model = manifest.semantic_models["semantic_model.test.renamed_semantic_model"]

        assert semantic_model.node_relation.alias == "fct_revenue"
        assert (
            semantic_model.node_relation.relation_name
            == f'"dbt"."{project.test_schema}"."fct_revenue"'
        )

        assert semantic_model.config.enabled is True
        assert semantic_model.config.group == "finance"
        assert semantic_model.config.meta == {"meta_tag_1": "this_meta"}


class TestSemanticModelConfigDefaultValues:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": semantic_model_schema_yml_v2_default_values,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_semantic_model_parsing_defaults(self, project) -> None:
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        manifest = result.result

        assert len(manifest.semantic_models) == 1
        semantic_model = list(manifest.semantic_models.values())[0]

        # With no custom name, alias should be based on model name (default test. + model name)
        assert semantic_model.node_relation.alias == "fct_revenue"

        assert (
            semantic_model.description
            == "This is the model fct_revenue. It should be able to use doc blocks"
        )

        # Should use default config values
        assert semantic_model.config.enabled is True
        assert semantic_model.config.group is None
        assert semantic_model.config.meta == {}


class TestSemanticModelConfigDoesNotExistPassesWithoutParsingSemanticModel:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": semantic_model_config_does_not_exist,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_semantic_model_parsing(self, project) -> None:
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        manifest = result.result
        assert len(manifest.semantic_models) == 0


class TestSemanticModelDisabledConfigIsNotParsed:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": semantic_model_schema_yml_v2_disabled,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_semantic_model_parsing(self, project) -> None:
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        manifest = result.result
        assert len(manifest.semantic_models) == 0


class TestSemanticModelFalseConfigIsNotParsed:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": semantic_model_schema_yml_v2_false_config,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_semantic_model_parsing(self, project) -> None:
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        manifest = result.result
        assert len(manifest.semantic_models) == 0


class TestMetricOnModelParsingWorks:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": base_schema_yml_v2 + schema_yml_v2_simple_metric_on_model_1,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_metric_on_model_parsing(self, project):
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        manifest = result.result

        semantic_model = manifest.semantic_models["semantic_model.test.fct_revenue"]
        assert semantic_model.defaults.agg_time_dimension == "second_dim"

        metrics = manifest.metrics
        semantic_manifest = SemanticManifest(manifest)
        semantic_manifest_metrics = {
            metric.name: metric
            for metric in semantic_manifest._get_pydantic_semantic_manifest().metrics
        }
        assert len(metrics) == 5

        simple_metric = metrics["metric.test.simple_metric"]
        assert simple_metric.name == "simple_metric"
        assert simple_metric.description == "This is our first simple metric."
        assert simple_metric.type == MetricType.SIMPLE
        assert simple_metric.type_params.metric_aggregation_params.agg == AggregationType.COUNT
        assert simple_metric.type_params.metric_aggregation_params.semantic_model == "fct_revenue"
        assert "semantic_model.test.fct_revenue" in simple_metric.depends_on.nodes
        assert (
            simple_metric.type_params.metric_aggregation_params.agg_time_dimension == "second_dim"
        )

        simple_metric_pydantic = semantic_manifest_metrics["simple_metric"]
        assert simple_metric_pydantic.name == "simple_metric"
        assert simple_metric_pydantic.description == "This is our first simple metric."
        assert simple_metric_pydantic.type == MetricType.SIMPLE
        assert (
            simple_metric_pydantic.type_params.metric_aggregation_params.agg
            == AggregationType.COUNT
        )
        assert (
            simple_metric_pydantic.type_params.metric_aggregation_params.semantic_model
            == "fct_revenue"
        )
        assert (
            simple_metric_pydantic.type_params.metric_aggregation_params.agg_time_dimension
            == "second_dim"
        )
        # No 'depends_on' in the pydantic metric

        simple_metric_2 = metrics["metric.test.simple_metric_2"]
        assert simple_metric_2.name == "simple_metric_2"
        assert simple_metric_2.description == "This is our second simple metric."
        assert simple_metric_2.type == MetricType.SIMPLE
        assert simple_metric_2.type_params.metric_aggregation_params.agg == AggregationType.COUNT
        assert (
            simple_metric_2.type_params.metric_aggregation_params.semantic_model == "fct_revenue"
        )
        assert "semantic_model.test.fct_revenue" in simple_metric_2.depends_on.nodes
        assert simple_metric_2.type_params.metric_aggregation_params.agg_time_dimension == "ds"

        simple_metric_2_pydantic = semantic_manifest_metrics["simple_metric_2"]
        assert simple_metric_2_pydantic.name == "simple_metric_2"
        assert simple_metric_2_pydantic.description == "This is our second simple metric."
        assert simple_metric_2_pydantic.type == MetricType.SIMPLE
        assert (
            simple_metric_2_pydantic.type_params.metric_aggregation_params.agg
            == AggregationType.COUNT
        )
        assert (
            simple_metric_2_pydantic.type_params.metric_aggregation_params.semantic_model
            == "fct_revenue"
        )
        assert (
            simple_metric_2_pydantic.type_params.metric_aggregation_params.agg_time_dimension
            == "ds"
        )

        # No 'depends_on' in the pydantic metric
        percentile_metric = metrics["metric.test.percentile_metric"]
        assert percentile_metric.name == "percentile_metric"
        assert percentile_metric.description == "This is our percentile metric."
        assert percentile_metric.type == MetricType.SIMPLE
        assert (
            percentile_metric.type_params.metric_aggregation_params.agg
            == AggregationType.PERCENTILE
        )
        assert (
            percentile_metric.type_params.metric_aggregation_params.semantic_model == "fct_revenue"
        )
        assert (
            percentile_metric.type_params.metric_aggregation_params.agg_params.percentile == 0.99
        )
        assert (
            percentile_metric.type_params.metric_aggregation_params.agg_params.use_discrete_percentile
            is True
        )
        assert (
            percentile_metric.type_params.metric_aggregation_params.agg_params.use_approximate_percentile
            is False
        )
        assert "semantic_model.test.fct_revenue" in percentile_metric.depends_on.nodes
        assert (
            percentile_metric.type_params.metric_aggregation_params.agg_time_dimension
            == "second_dim"
        )

        percentile_metric_pydantic = semantic_manifest_metrics["percentile_metric"]
        assert percentile_metric_pydantic.name == "percentile_metric"
        assert percentile_metric_pydantic.description == "This is our percentile metric."
        assert percentile_metric_pydantic.type == MetricType.SIMPLE
        assert (
            percentile_metric_pydantic.type_params.metric_aggregation_params.agg
            == AggregationType.PERCENTILE
        )
        assert (
            percentile_metric_pydantic.type_params.metric_aggregation_params.semantic_model
            == "fct_revenue"
        )
        assert (
            percentile_metric_pydantic.type_params.metric_aggregation_params.agg_params.percentile
            == 0.99
        )
        assert (
            percentile_metric_pydantic.type_params.metric_aggregation_params.agg_params.use_discrete_percentile
            is True
        )
        assert (
            percentile_metric_pydantic.type_params.metric_aggregation_params.agg_params.use_approximate_percentile
            is False
        )
        assert (
            percentile_metric_pydantic.type_params.metric_aggregation_params.agg_time_dimension
            == "second_dim"
        )

        cumulative_metric = metrics["metric.test.cumulative_metric"]
        assert cumulative_metric.name == "cumulative_metric"
        assert cumulative_metric.description == "This is our cumulative metric."
        assert cumulative_metric.type == MetricType.CUMULATIVE
        assert cumulative_metric.type_params.cumulative_type_params.grain_to_date == "day"
        assert (
            cumulative_metric.type_params.cumulative_type_params.period_agg
            == PeriodAggregation.FIRST
        )
        assert cumulative_metric.type_params.cumulative_type_params.metric.name == "simple_metric"
        assert "metric.test.simple_metric" in cumulative_metric.depends_on.nodes
        assert cumulative_metric.type_params.metric_aggregation_params is None

        cumulative_metric_pydantic = semantic_manifest_metrics["cumulative_metric"]
        assert cumulative_metric_pydantic.name == "cumulative_metric"
        assert cumulative_metric_pydantic.description == "This is our cumulative metric."
        assert cumulative_metric_pydantic.type == MetricType.CUMULATIVE
        assert cumulative_metric_pydantic.type_params.cumulative_type_params.grain_to_date == "day"
        assert (
            cumulative_metric_pydantic.type_params.cumulative_type_params.period_agg
            == PeriodAggregation.FIRST
        )
        assert (
            cumulative_metric_pydantic.type_params.cumulative_type_params.metric.name
            == "simple_metric"
        )
        assert cumulative_metric_pydantic.type_params.metric_aggregation_params is None

        conversion_metric = metrics["metric.test.conversion_metric"]
        assert conversion_metric.name == "conversion_metric"
        assert conversion_metric.description == "This is our conversion metric."
        assert conversion_metric.type == MetricType.CONVERSION
        assert conversion_metric.type_params.conversion_type_params.entity == "id_entity"
        assert (
            conversion_metric.type_params.conversion_type_params.calculation
            is ConversionCalculationType.CONVERSION_RATE
        )
        assert (
            conversion_metric.type_params.conversion_type_params.base_metric.name
            == "simple_metric"
        )
        assert (
            conversion_metric.type_params.conversion_type_params.conversion_metric.name
            == "simple_metric_2"
        )
        assert "metric.test.simple_metric" in conversion_metric.depends_on.nodes
        assert "metric.test.simple_metric_2" in conversion_metric.depends_on.nodes
        assert conversion_metric.type_params.metric_aggregation_params is None

        conversion_metric_pydantic = semantic_manifest_metrics["conversion_metric"]
        assert conversion_metric_pydantic.name == "conversion_metric"
        assert conversion_metric_pydantic.description == "This is our conversion metric."
        assert conversion_metric_pydantic.type == MetricType.CONVERSION
        assert conversion_metric_pydantic.type_params.conversion_type_params.entity == "id_entity"
        assert (
            conversion_metric_pydantic.type_params.conversion_type_params.calculation
            is ConversionCalculationType.CONVERSION_RATE
        )
        assert (
            conversion_metric_pydantic.type_params.conversion_type_params.base_metric.name
            == "simple_metric"
        )
        assert (
            conversion_metric_pydantic.type_params.conversion_type_params.conversion_metric.name
            == "simple_metric_2"
        )
        assert conversion_metric_pydantic.type_params.metric_aggregation_params is None


class TestMetricHiddenMapsToIsPrivate:
    """Test that a metric's 'hidden' field in YAML is reflected as 'is_private' on the parsed metric."""

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": base_schema_yml_v2 + schema_yml_v2_metrics_with_hidden,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_metric_hidden_yaml_maps_to_is_private(self, project):
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        manifest = result.result
        metrics = manifest.metrics
        assert len(metrics) == 2

        public_metric = metrics["metric.test.public_metric"]
        assert public_metric.type_params.is_private is False

        private_metric = metrics["metric.test.private_metric"]
        assert private_metric.type_params.is_private is True


class TestStandaloneMetricParsingSimpleMetricFails:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": base_schema_yml_v2 + schema_yml_v2_standalone_simple_metric,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_standalone_metric_parsing(self, project):
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert not result.success
        assert (
            "simple metrics in v2 YAML must be attached to semantic_model" in result.exception.msg
        )


class TestStandaloneMetricParsingWorks:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": base_schema_yml_v2
            + schema_yml_v2_simple_metric_on_model_1
            + schema_yml_v2_standalone_metrics,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_included_metric_parsing(self, project):
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        manifest = result.result

        semantic_model = manifest.semantic_models["semantic_model.test.fct_revenue"]
        assert semantic_model.defaults.agg_time_dimension == "second_dim"

        metrics = manifest.metrics
        semantic_manifest = SemanticManifest(manifest)
        semantic_manifest_metrics = {
            metric.name: metric
            for metric in semantic_manifest._get_pydantic_semantic_manifest().metrics
        }
        assert len(metrics) == 6

        metric = metrics["metric.test.standalone_conversion_metric"]
        assert metric.name == "standalone_conversion_metric"
        assert metric.description == "This is our standalone conversion metric."
        assert metric.type == MetricType.CONVERSION
        assert metric.type_params.conversion_type_params.entity == "id_entity"
        assert (
            metric.type_params.conversion_type_params.calculation
            == ConversionCalculationType.CONVERSION_RATE
        )
        assert metric.type_params.conversion_type_params.base_metric.name == "simple_metric"
        assert (
            metric.type_params.conversion_type_params.conversion_metric.name == "simple_metric_2"
        )
        assert set(metric.depends_on.nodes) == set(
            [
                "metric.test.simple_metric",
                "metric.test.simple_metric_2",
            ]
        )
        assert metric.type_params.metric_aggregation_params is None
        assert metric.filter.where_filters[0].where_sql_template == "id > 0"

        metric_pydantic = semantic_manifest_metrics["standalone_conversion_metric"]
        assert metric_pydantic.name == "standalone_conversion_metric"
        assert metric_pydantic.description == "This is our standalone conversion metric."
        assert metric_pydantic.type == MetricType.CONVERSION
        assert metric_pydantic.type_params.conversion_type_params.entity == "id_entity"
        assert (
            metric_pydantic.type_params.conversion_type_params.calculation
            == ConversionCalculationType.CONVERSION_RATE
        )
        assert (
            metric_pydantic.type_params.conversion_type_params.base_metric.name == "simple_metric"
        )
        assert (
            metric_pydantic.type_params.conversion_type_params.conversion_metric.name
            == "simple_metric_2"
        )
        assert metric_pydantic.type_params.metric_aggregation_params is None
        assert metric_pydantic.filter.where_filters[0].where_sql_template == "id > 0"


class TestCumulativeMetricNoInputMetricFails:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": base_schema_yml_v2
            + schema_yml_v2_cumulative_metric_missing_input_metric,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_cumulative_metric_no_input_metric_parsing_fails(self, project) -> None:
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert not result.success
        assert "input_metric is required for cumulative metrics." in str(result.exception)


class TestConversionMetricNoBaseMetricFails:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": base_schema_yml_v2 + schema_yml_v2_conversion_metric_missing_base_metric,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_conversion_metric_no_base_metric_parsing_fails(self, project) -> None:
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert not result.success
        assert "base_metric is required for conversion metrics." in str(result.exception)


class TestDerivedSemanticsWithDocJinjaParsingWorks:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": semantic_model_schema_yml_v2 + derived_semantics_with_doc_jinja_yml,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "docs.md": semantic_model_descriptions,
        }

    def test_derived_semantics_doc_jinja_parsing(self, project) -> None:
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        manifest = result.result
        assert len(manifest.semantic_models) == 1
        semantic_model = manifest.semantic_models["semantic_model.test.fct_revenue"]
        entities = {entity.name: entity for entity in semantic_model.entities}
        assert entities["derived_id_entity"].description == "qux"
        dimensions = {dimension.name: dimension for dimension in semantic_model.dimensions}
        assert dimensions["derived_id_dimension"].description == "bar"


class TestDerivedSemanticsParsingWorks:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": semantic_model_schema_yml_v2 + derived_semantics_yml,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_derived_semantics_parsing(self, project) -> None:
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        manifest = result.result
        assert len(manifest.semantic_models) == 1
        semantic_model = manifest.semantic_models["semantic_model.test.fct_revenue"]
        entities = {entity.name: entity for entity in semantic_model.entities}
        assert (
            len(entities) == 5
        )  # length is so long because it is column entities + derived entities

        id_entity = entities["derived_id_entity"]
        assert id_entity.type == EntityType.FOREIGN
        assert id_entity.description == "This is the id entity, and it is the primary entity."
        assert id_entity.expr == "id + foreign_id_col"
        assert id_entity.config.meta == {"test_label_thing": "derived_entity_1"}

        id_entity_with_no_optional_fields = entities["derived_id_entity_with_no_optional_fields"]
        assert id_entity_with_no_optional_fields.type == EntityType.FOREIGN
        assert id_entity_with_no_optional_fields.expr == "id + foreign_id_col"
        assert id_entity_with_no_optional_fields.config.meta == {}

        dimensions = {dimension.name: dimension for dimension in semantic_model.dimensions}
        assert len(dimensions) == 4  # includes non-derived dimensions
        assert dimensions["derived_id_dimension"].type == DimensionType.CATEGORICAL
        assert dimensions["derived_id_dimension"].expr == "id"
        assert dimensions["derived_id_dimension"].config.meta == {}
        assert dimensions["derived_id_dimension"].type_params.validity_params.is_start is True
        assert dimensions["derived_id_dimension"].type_params.validity_params.is_end is True


class TestSemanticModelWithPrimaryEntityOnlyOnColumn:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": semantic_model_schema_yml_v2_with_primary_entity_only_on_column,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_primary_entity_type_is_id_entity(self, project):
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        manifest = result.result
        assert len(manifest.semantic_models) == 1
        semantic_model = list(manifest.semantic_models.values())[0]
        entities = {entity.name: entity for entity in semantic_model.entities}
        primary_entity = [
            entity for entity in entities.values() if entity.type == EntityType.PRIMARY
        ]
        assert len(primary_entity) == 1
        primary_entity = primary_entity[0]
        assert primary_entity.name == "id_entity"
        assert semantic_model.primary_entity is None


class TestSemanticModelWithPrimaryEntityOnlyOnModel:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": semantic_model_schema_yml_v2_primary_entity_only_on_model,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_primary_entities_empty(self, project):
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        manifest = result.result
        assert len(manifest.semantic_models) == 1
        semantic_model = list(manifest.semantic_models.values())[0]
        entities = {entity.name: entity for entity in semantic_model.entities}
        primary_entity = [
            entity for entity in entities.values() if entity.type == EntityType.PRIMARY
        ]
        assert len(primary_entity) == 0
        assert semantic_model.primary_entity == "id_entity"


class TestSimpleSemanticModelWithMetricWithDocJinja:

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": base_schema_yml_v2
            + schema_yml_v2_simple_metric_on_model_1
            + schema_yml_v2_metric_with_doc_jinja,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "docs.md": semantic_model_descriptions,
        }

    def test_simple_metric_with_doc_jinja_parsing(self, project):
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        manifest = result.result
        assert len(manifest.semantic_models) == 1
        metric = manifest.metrics["metric.test.simple_metric_with_doc_jinja"]
        assert metric.description == "describe away!"


class TestSimpleSemanticModelWithFilterWithFilterDimensionJinja:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": base_schema_yml_v2
            + schema_yml_v2_simple_metric_on_model_1
            + schema_yml_v2_metric_with_filter_dimension_jinja,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "docs.md": semantic_model_descriptions,
        }

    def test_simple_metric_with_filter_with_filter_dimension_jinja_parsing(self, project):
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        manifest = result.result
        assert len(manifest.semantic_models) == 1
        metric = manifest.metrics["metric.test.simple_metric_with_filter_dimension_jinja"]
        assert (
            metric.filter.where_filters[0].where_sql_template
            == "{{ Dimension('id_entity__id_dim') }} > 0 and {{ TimeDimension('id_entity__id_dim', 'day') }} > '2020-01-01'"
        )


class TestTopLevelSemanticsMetricWithDocJinja:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": base_schema_yml_v2
            + schema_yml_v2_simple_metric_on_model_1
            + schema_yml_v2_standalone_metrics_with_doc_jinja,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "docs.md": semantic_model_descriptions,
        }

    def test_top_level_metric_with_doc_jinja_parsing(self, project):
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        manifest = result.result
        assert len(manifest.semantic_models) == 1
        metric = manifest.metrics["metric.test.standalone_conversion_metric"]
        assert metric.description == "describe away!"
        assert (
            metric.filter.where_filters[0].where_sql_template
            == "{{ Dimension('id_entity__id_dim') }} > 0"
        )

        semantic_manifest = SemanticManifest(manifest)
        semantic_manifest_metrics = {
            metric.name: metric
            for metric in semantic_manifest._get_pydantic_semantic_manifest().metrics
        }
        assert (
            semantic_manifest_metrics["standalone_conversion_metric"]
            .filter.where_filters[0]
            .where_sql_template
            == "{{ Dimension('id_entity__id_dim') }} > 0"
        )


class TestDerivedMetricWithInputMetricsFilterDimensionJinja:
    """Test that {{ Dimension(...) }} jinja in input_metrics[].filter is not rendered at parse time."""

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": base_schema_yml_v2
            + schema_yml_v2_metric_with_input_metrics_filter_dimension_jinja,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_input_metrics_filter_jinja_not_rendered(self, project):
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        manifest = result.result
        metric = manifest.metrics["metric.test.derived_metric_with_jinja_filter"]
        assert metric.type == MetricType.DERIVED
        # The input metric filter should preserve the Dimension jinja template
        offset_input = [m for m in metric.type_params.metrics if m.alias == "offset_metric"][0]
        assert "{{ Dimension('id_entity__id_dim') }} > 0" in (
            offset_input.filter.where_filters[0].where_sql_template
        )


class TestRatioMetricWithNumeratorFilterDimensionJinja:
    """Test that {{ Dimension(...) }} jinja in numerator.filter is not rendered at parse time."""

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": base_schema_yml_v2
            + schema_yml_v2_metric_with_numerator_filter_dimension_jinja,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_numerator_filter_jinja_not_rendered(self, project):
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        manifest = result.result
        metric = manifest.metrics["metric.test.ratio_metric_with_jinja_filter"]
        assert metric.type == MetricType.RATIO
        # The numerator filter should preserve the Dimension jinja template
        assert "{{ Dimension('id_entity__id_dim') }} > 0" in (
            metric.type_params.numerator.filter.where_filters[0].where_sql_template
        )


class TestMetricOnModelWithCustomSemanticModelName:
    """Test that metrics correctly reference the custom semantic model name when
    semantic_model.name is set, rather than using the dbt model name."""

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": base_schema_yml_v2_with_custom_sm_name
            + schema_yml_v2_simple_metric_on_model_1,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_metrics_use_custom_semantic_model_name(self, project):
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        manifest = result.result

        # Semantic model should be registered under the custom name
        assert "semantic_model.test.custom_semantic_model" in manifest.semantic_models
        semantic_model = manifest.semantic_models["semantic_model.test.custom_semantic_model"]
        assert semantic_model.name == "custom_semantic_model"
        # But it still points to the fct_revenue model
        assert semantic_model.node_relation.alias == "fct_revenue"

        # Simple metrics should reference the custom semantic model name
        simple_metric = manifest.metrics["metric.test.simple_metric"]
        assert (
            simple_metric.type_params.metric_aggregation_params.semantic_model
            == "custom_semantic_model"
        )
        assert "semantic_model.test.custom_semantic_model" in simple_metric.depends_on.nodes

        simple_metric_2 = manifest.metrics["metric.test.simple_metric_2"]
        assert (
            simple_metric_2.type_params.metric_aggregation_params.semantic_model
            == "custom_semantic_model"
        )
        assert "semantic_model.test.custom_semantic_model" in simple_metric_2.depends_on.nodes

        # Conversion metric should pass validation (it references simple metrics
        # which are linked to the custom-named semantic model)
        conversion_metric = manifest.metrics["metric.test.conversion_metric"]
        assert conversion_metric.type == MetricType.CONVERSION
        assert (
            conversion_metric.type_params.conversion_type_params.base_metric.name
            == "simple_metric"
        )
        assert (
            conversion_metric.type_params.conversion_type_params.conversion_metric.name
            == "simple_metric_2"
        )

        # Verify semantic manifest validation also passes
        semantic_manifest = SemanticManifest(manifest)
        semantic_manifest_metrics = {
            metric.name: metric
            for metric in semantic_manifest._get_pydantic_semantic_manifest().metrics
        }
        assert (
            semantic_manifest_metrics[
                "simple_metric"
            ].type_params.metric_aggregation_params.semantic_model
            == "custom_semantic_model"
        )


class TestMetricOnModelWithoutCustomSemanticModelName:
    """Test that metrics correctly use the dbt model name as the semantic model name
    when no custom semantic_model.name is set (semantic_model: true)."""

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": base_schema_yml_v2 + schema_yml_v2_simple_metric_on_model_1,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_metrics_use_model_name_as_semantic_model_name(self, project):
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        manifest = result.result

        # Semantic model should be registered under the model name
        assert "semantic_model.test.fct_revenue" in manifest.semantic_models

        # Simple metrics should reference the model name
        simple_metric = manifest.metrics["metric.test.simple_metric"]
        assert simple_metric.type_params.metric_aggregation_params.semantic_model == "fct_revenue"
        assert "semantic_model.test.fct_revenue" in simple_metric.depends_on.nodes

        # Conversion metric should pass validation
        conversion_metric = manifest.metrics["metric.test.conversion_metric"]
        assert conversion_metric.type == MetricType.CONVERSION
        assert (
            conversion_metric.type_params.conversion_type_params.base_metric.name
            == "simple_metric"
        )

        # Verify semantic manifest validation also passes
        semantic_manifest = SemanticManifest(manifest)
        semantic_manifest_metrics = {
            metric.name: metric
            for metric in semantic_manifest._get_pydantic_semantic_manifest().metrics
        }
        assert (
            semantic_manifest_metrics[
                "simple_metric"
            ].type_params.metric_aggregation_params.semantic_model
            == "fct_revenue"
        )


# TODO DI-4605: add enforcement and a test for when there are validity params with no column granularity


class TestV2SemanticModelPartialParsingChanged:
    """
    Regression test for DI-3697: partial parsing must not create duplicate v2 semantic models
    when the model YAML entry is changed.
    """

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": semantic_model_schema_yml_v2,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_v2_semantic_model_partial_parsing_changed(self, project):
        from dbt.tests.util import write_file

        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        assert len(result.result.semantic_models) == 1

        # Modify a column dimension label — triggers partial parsing of the model entry
        modified = semantic_model_schema_yml_v2.replace(
            'label: "ID Dimension"', 'label: "Updated ID Dimension"'
        )
        write_file(modified, project.project_root, "models", "schema.yml")

        result = runner.invoke(["parse"])
        assert result.success, result.exception
        # Before the fix this would be 2 (duplicate), not 1
        assert len(result.result.semantic_models) == 1


class TestV2SemanticModelPartialParsingDisabled:
    """
    Regression test for DI-3697: partial parsing must clean up a v2 semantic model
    when semantic_model is changed from true to false on the model entry.
    """

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": semantic_model_schema_yml_v2,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_v2_semantic_model_partial_parsing_disabled(self, project):
        from dbt.tests.util import write_file

        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        assert len(result.result.semantic_models) == 1

        # Disable the v2 semantic model
        disabled = semantic_model_schema_yml_v2.replace(
            "semantic_model: true", "semantic_model: false"
        )
        write_file(disabled, project.project_root, "models", "schema.yml")

        result = runner.invoke(["parse"])
        assert result.success, result.exception
        assert len(result.result.semantic_models) == 0


class TestSemanticModelEntityWithoutName:
    """Regression test for DI-4133: entity: {type: foreign} without an explicit 'name'
    must parse successfully and default the entity name to the column name."""

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": semantic_model_schema_yml_v2_entity_without_name,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_entity_without_name_defaults_to_column_name(self, project):
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success, result.exception
        manifest = result.result
        assert len(manifest.semantic_models) == 1
        semantic_model = list(manifest.semantic_models.values())[0]
        entities = {entity.name: entity for entity in semantic_model.entities}

        # Column "id" with entity: {type: primary} — name should default to "id"
        assert "id" in entities
        assert entities["id"].type == EntityType.PRIMARY
        assert entities["id"].expr is None  # name matches column, no expr needed

        # Column "foreign_key" with entity: {type: foreign} — name should default to "foreign_key"
        assert "foreign_key" in entities
        assert entities["foreign_key"].type == EntityType.FOREIGN
        assert entities["foreign_key"].expr is None  # name matches column, no expr needed


# TODO DI-4603: add enforcement and a test for a TIME type dimension and a column that has no granularity set


class TestSemanticModelOnVersionedModel:
    """Regression test for dbt-core#15294.

    An inline (v2) semantic model declared on a *versioned* model — whose
    columns live under versions[].columns — used to be silently dropped from
    the manifest (no error, no warning). It should be parsed exactly once, for
    the latest version (which `ref()` resolves to).
    """

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "customers_v1.sql": versioned_model_v1_sql,
            "customers_v2.sql": versioned_model_v2_sql,
            "metricflow_time_spine.sql": simple_metricflow_time_spine_sql,
            "schema.yml": versioned_model_semantic_schema_yml_v2,
        }

    def test_semantic_model_on_versioned_model(self, project) -> None:
        runner = dbtTestRunner()
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        manifest = result.result

        # Exactly one semantic model — not zero (the bug) and not one-per-version.
        assert len(manifest.semantic_models) == 1
        semantic_model = manifest.semantic_models["semantic_model.test.customers"]
        assert semantic_model.name == "customers"
        # The semantic model targets the model as resolved by ref() (latest version).
        assert semantic_model.model == "ref('customers')"

        # Entity + time dimension were resolved from the versioned columns.
        entities = {entity.name: entity for entity in semantic_model.entities}
        assert "customer" in entities
        assert entities["customer"].type == EntityType.PRIMARY
        dimensions = {dimension.name: dimension for dimension in semantic_model.dimensions}
        assert "created_at" in dimensions
        assert dimensions["created_at"].type == DimensionType.TIME

        # The model-level metric is also produced (it was dropped by the same bug).
        assert "metric.test.customers_count" in manifest.metrics
