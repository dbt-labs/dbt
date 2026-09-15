import time
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Sequence

from metricflow_semantic_interfaces.references import (
    DimensionReference,
    EntityReference,
    LinkableElementReference,
    MeasureReference,
    SemanticModelReference,
    TimeDimensionReference,
)
from metricflow_semantic_interfaces.type_enums import (
    AggregationType,
    DataType,
    DimensionType,
    EntityType,
    MetricType,
    TimeGranularity,
)

from dbt.artifacts.resources import SourceFileMetadata
from dbt.artifacts.resources.base import GraphResource
from dbt.artifacts.resources.v1.components import DependsOn, RefArgs
from dbt.artifacts.resources.v1.metric import Metric
from dbt.artifacts.resources.v1.semantic_layer_components import (
    MeasureAggregationParameters,
    NonAdditiveDimension,
)
from dbt_common.contracts.config.base import BaseConfig, CompareBehavior, MergeBehavior
from dbt_common.dataclass_schema import dbtClassMixin

"""
The classes in this file are dataclasses which are used to construct the Semantic
Model node in dbt-core. Additionally, these classes need to at a minimum support
what is specified in their protocol definitions in metricflow-semantic-interfaces.
Their protocol definitions can be found here:
https://github.com/dbt-labs/metricflow/blob/main/metricflow_semantic_interfaces/protocols/semantic_model.py
"""


@dataclass
class SemanticLayerElementConfig(dbtClassMixin):
    meta: Dict[str, Any] = field(
        default_factory=dict,
        metadata=MergeBehavior.Update.meta(),
    )


@dataclass
class Defaults(dbtClassMixin):
    agg_time_dimension: Optional[str] = None


@dataclass
class NodeRelation(dbtClassMixin):
    alias: str
    schema_name: str  # TODO: Could this be called simply "schema" so we could reuse StateRelation?
    database: Optional[str] = None
    relation_name: Optional[str] = ""


# ====================================
# Dimension objects
# Dimension protocols: https://github.com/dbt-labs/metricflow/blob/main/metricflow_semantic_interfaces/protocols/dimension.py
# ====================================


@dataclass
class DimensionValidityParams(dbtClassMixin):
    is_start: bool = False
    is_end: bool = False


@dataclass
class DimensionTypeParams(dbtClassMixin):
    time_granularity: TimeGranularity
    validity_params: Optional[DimensionValidityParams] = None


@dataclass
class Dimension(dbtClassMixin):
    name: str
    type: DimensionType
    description: Optional[str] = None
    label: Optional[str] = None
    is_partition: bool = False
    type_params: Optional[DimensionTypeParams] = None
    expr: Optional[str] = None
    metadata: Optional[SourceFileMetadata] = None
    config: Optional[SemanticLayerElementConfig] = None
    datatype: Optional[DataType] = None

    @property
    def reference(self) -> DimensionReference:
        return DimensionReference(element_name=self.name)

    @property
    def time_dimension_reference(self) -> Optional[TimeDimensionReference]:
        if self.type == DimensionType.TIME:
            return TimeDimensionReference(element_name=self.name)
        else:
            return None

    @property
    def validity_params(self) -> Optional[DimensionValidityParams]:
        if self.type_params:
            return self.type_params.validity_params
        else:
            return None


# ====================================
# Entity objects
# Entity protocols: https://github.com/dbt-labs/metricflow/blob/main/metricflow_semantic_interfaces/protocols/entity.py
# ====================================


@dataclass
class Entity(dbtClassMixin):
    name: str
    type: EntityType
    description: Optional[str] = None
    label: Optional[str] = None
    role: Optional[str] = None
    expr: Optional[str] = None
    config: Optional[SemanticLayerElementConfig] = None

    @property
    def reference(self) -> EntityReference:
        return EntityReference(element_name=self.name)

    @property
    def is_linkable_entity_type(self) -> bool:
        return self.type in (EntityType.PRIMARY, EntityType.UNIQUE, EntityType.NATURAL)


# ====================================
# Measure object
# Measure protocols: https://github.com/dbt-labs/metricflow/blob/main/metricflow_semantic_interfaces/protocols/measure.py
# ====================================


@dataclass
class Measure(dbtClassMixin):
    name: str
    agg: AggregationType
    description: Optional[str] = None
    label: Optional[str] = None
    create_metric: bool = False
    expr: Optional[str] = None
    agg_params: Optional[MeasureAggregationParameters] = None
    non_additive_dimension: Optional[NonAdditiveDimension] = None
    agg_time_dimension: Optional[str] = None
    config: Optional[SemanticLayerElementConfig] = None

    @property
    def reference(self) -> MeasureReference:
        return MeasureReference(element_name=self.name)


# ====================================
# SemanticModel final parts
# ====================================


@dataclass
class SemanticModelConfig(BaseConfig):
    enabled: bool = True
    group: Optional[str] = field(
        default=None,
        metadata=CompareBehavior.Exclude.meta(),
    )
    meta: Dict[str, Any] = field(
        default_factory=dict,
        metadata=MergeBehavior.Update.meta(),
    )


@dataclass
class SemanticModel(GraphResource):
    model: str
    node_relation: Optional[NodeRelation]
    description: Optional[str] = None
    label: Optional[str] = None
    defaults: Optional[Defaults] = None
    entities: Sequence[Entity] = field(default_factory=list)
    measures: Sequence[Measure] = field(default_factory=list)
    dimensions: Sequence[Dimension] = field(default_factory=list)
    metadata: Optional[SourceFileMetadata] = None
    depends_on: DependsOn = field(default_factory=DependsOn)
    refs: List[RefArgs] = field(default_factory=list)
    created_at: float = field(default_factory=lambda: time.time())
    config: SemanticModelConfig = field(default_factory=SemanticModelConfig)
    unrendered_config: Dict[str, Any] = field(default_factory=dict)
    primary_entity: Optional[str] = None
    group: Optional[str] = None

    @property
    def entity_references(self) -> List[LinkableElementReference]:
        return [entity.reference for entity in self.entities]

    @property
    def dimension_references(self) -> List[LinkableElementReference]:
        return [dimension.reference for dimension in self.dimensions]

    @property
    def measure_references(self) -> List[MeasureReference]:
        return [measure.reference for measure in self.measures]

    @property
    def has_validity_dimensions(self) -> bool:
        return any([dim.validity_params is not None for dim in self.dimensions])

    @property
    def validity_start_dimension(self) -> Optional[Dimension]:
        validity_start_dims = [
            dim for dim in self.dimensions if dim.validity_params and dim.validity_params.is_start
        ]
        if not validity_start_dims:
            return None
        return validity_start_dims[0]

    @property
    def validity_end_dimension(self) -> Optional[Dimension]:
        validity_end_dims = [
            dim for dim in self.dimensions if dim.validity_params and dim.validity_params.is_end
        ]
        if not validity_end_dims:
            return None
        return validity_end_dims[0]

    @property
    def partitions(self) -> List[Dimension]:  # noqa: D
        return [dim for dim in self.dimensions or [] if dim.is_partition]

    @property
    def partition(self) -> Optional[Dimension]:
        partitions = self.partitions
        if not partitions:
            return None
        return partitions[0]

    @property
    def reference(self) -> SemanticModelReference:
        return SemanticModelReference(semantic_model_name=self.name)

    def checked_agg_time_dimension_for_measure(
        self, measure_reference: MeasureReference
    ) -> TimeDimensionReference:
        measure: Optional[Measure] = None
        for measure in self.measures:
            if measure.reference == measure_reference:
                measure = measure

        assert (
            measure is not None
        ), f"No measure with name ({measure_reference.element_name}) in semantic_model with name ({self.name})"

        default_agg_time_dimension = (
            self.defaults.agg_time_dimension if self.defaults is not None else None
        )

        agg_time_dimension_name = measure.agg_time_dimension or default_agg_time_dimension
        assert agg_time_dimension_name is not None, (
            f"Aggregation time dimension for measure {measure.name} on semantic model {self.name} is not set! "
            "To fix this either specify a default `agg_time_dimension` for the semantic model or define an "
            "`agg_time_dimension` on the measure directly."
        )
        return TimeDimensionReference(element_name=agg_time_dimension_name)

    def checked_agg_time_dimension_for_simple_metric(
        self, metric: Metric
    ) -> TimeDimensionReference:
        assert (
            metric.type == MetricType.SIMPLE
        ), "Only simple metrics can have an agg time dimension."
        metric_agg_params = metric.type_params.metric_aggregation_params
        # There are validations elsewhere to check this for metrics and provide messaging for it.
        assert metric_agg_params, "Simple metrics must have metric_aggregation_params."
        # This indicates a validation bug / dev error, not a user error that should appear
        # in a user's YAML.
        assert (
            metric_agg_params.semantic_model == self.name
        ), "Cannot retrieve the agg time dimension for a metric from a different model "
        f"than the one that the metric belongs to. Metric `{metric.name}` belongs to model "
        f"`{metric_agg_params.semantic_model}`, but we requested the agg time dimension from model `{self.name}`."

        metric_time_dimension_name = None
        if (
            metric.type_params
            and metric.type_params.metric_aggregation_params
            and metric.type_params.metric_aggregation_params.agg_time_dimension
        ):
            metric_time_dimension_name = (
                metric.type_params.metric_aggregation_params.agg_time_dimension
            )

        default_agg_time_dimension = (
            self.defaults.agg_time_dimension if self.defaults is not None else None
        )
        agg_time_dimension_name = metric_time_dimension_name or default_agg_time_dimension

        assert agg_time_dimension_name is not None, (
            f"Aggregation time dimension for metric {metric.name} is not set! This should either be set directly on "
            f"the metric specification in the model, or else defaulted to the time dimension in the data "
            f"source containing the metric."
        )
        return TimeDimensionReference(element_name=agg_time_dimension_name)

    @property
    def primary_entity_reference(self) -> Optional[EntityReference]:
        return (
            EntityReference(element_name=self.primary_entity)
            if self.primary_entity is not None
            else None
        )
