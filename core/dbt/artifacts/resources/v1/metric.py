import time
from dataclasses import dataclass, field
from typing import Any, Dict, List, Literal, Optional, Union

from metricflow_semantic_interfaces.references import MeasureReference, MetricReference
from metricflow_semantic_interfaces.type_enums import (
    AggregationType,
    ConversionCalculationType,
    MetricType,
    PeriodAggregation,
    TimeGranularity,
)

from dbt.artifacts.resources.base import GraphResource
from dbt.artifacts.resources.types import NodeType
from dbt.artifacts.resources.v1.components import DependsOn, RefArgs
from dbt.artifacts.resources.v1.semantic_layer_components import (
    MeasureAggregationParameters,
    NonAdditiveDimension,
    SourceFileMetadata,
    WhereFilterIntersection,
)
from dbt.artifacts.resources.v1.config import list_str, metas
from dbt_common.contracts.config.base import BaseConfig, CompareBehavior, MergeBehavior
from dbt_common.contracts.config.metadata import ShowBehavior
from dbt_common.dataclass_schema import dbtClassMixin

"""
The following classes are dataclasses which are used to construct the Metric
node in dbt-core. Additionally, these classes need to at a minimum support
what is specified in their protocol definitions in dbt-semantic-interfaces.
Their protocol definitions can be found here:
https://github.com/dbt-labs/dbt-semantic-interfaces/blob/main/metricflow_semantic_interfaces/protocols/metric.py
"""


@dataclass
class MetricInputMeasure(dbtClassMixin):
    name: str
    filter: Optional[WhereFilterIntersection] = None
    alias: Optional[str] = None
    join_to_timespine: bool = False
    fill_nulls_with: Optional[int] = None

    def measure_reference(self) -> MeasureReference:
        return MeasureReference(element_name=self.name)

    def post_aggregation_measure_reference(self) -> MeasureReference:
        return MeasureReference(element_name=self.alias or self.name)


@dataclass
class MetricTimeWindow(dbtClassMixin):
    count: int
    granularity: str

    @property
    def window_string(self) -> str:  # noqa: D
        return f"{self.count} {self.granularity}"

    @property
    def is_standard_granularity(self) -> bool:  # noqa: D
        return self.granularity.casefold() in {item.value.casefold() for item in TimeGranularity}


@dataclass
class MetricInput(dbtClassMixin):
    name: str
    filter: Optional[WhereFilterIntersection] = None
    alias: Optional[str] = None
    offset_window: Optional[MetricTimeWindow] = None
    offset_to_grain: Optional[str] = None

    def as_reference(self) -> MetricReference:
        return MetricReference(element_name=self.name)

    def post_aggregation_reference(self) -> MetricReference:
        return MetricReference(element_name=self.alias or self.name)


@dataclass
class ConstantPropertyInput(dbtClassMixin):
    base_property: str
    conversion_property: str


@dataclass
class ConversionTypeParams(dbtClassMixin):
    entity: str
    base_measure: Optional[MetricInputMeasure] = None
    conversion_measure: Optional[MetricInputMeasure] = None
    base_metric: Optional[MetricInput] = None
    conversion_metric: Optional[MetricInput] = None
    calculation: ConversionCalculationType = ConversionCalculationType.CONVERSION_RATE
    window: Optional[MetricTimeWindow] = None
    constant_properties: Optional[List[ConstantPropertyInput]] = None


@dataclass
class CumulativeTypeParams(dbtClassMixin):
    window: Optional[MetricTimeWindow] = None
    grain_to_date: Optional[str] = None
    period_agg: PeriodAggregation = PeriodAggregation.FIRST
    metric: Optional[MetricInput] = None


@dataclass
class MetricAggregationParams(dbtClassMixin):
    semantic_model: str
    agg: AggregationType
    agg_params: Optional[MeasureAggregationParameters] = None
    agg_time_dimension: Optional[str] = None
    non_additive_dimension: Optional[NonAdditiveDimension] = None


@dataclass
class MetricTypeParams(dbtClassMixin):
    # Only used in v1 Semantic YAML
    measure: Optional[MetricInputMeasure] = None
    # Only used in v1 Semantic YAML
    input_measures: List[MetricInputMeasure] = field(default_factory=list)

    numerator: Optional[MetricInput] = None
    denominator: Optional[MetricInput] = None
    expr: Optional[str] = None
    window: Optional[MetricTimeWindow] = None
    grain_to_date: Optional[TimeGranularity] = (
        None  # legacy, use cumulative_type_params.grain_to_date
    )
    metrics: Optional[List[MetricInput]] = None
    conversion_type_params: Optional[ConversionTypeParams] = None
    cumulative_type_params: Optional[CumulativeTypeParams] = None
    metric_aggregation_params: Optional[MetricAggregationParams] = None

    # Below this point, all fields are only used in v2 Semantic YAML
    fill_nulls_with: Optional[int] = None
    join_to_timespine: bool = False
    is_private: Optional[bool] = None  # populated by "hidden" field in YAML

    def get_semantic_model_name(self) -> Optional[str]:
        """Simple helper to avoid having to null check intermediate members."""
        return (
            self.metric_aggregation_params.semantic_model
            if self.metric_aggregation_params is not None
            else None
        )


@dataclass
class MetricConfig(BaseConfig):
    enabled: bool = True
    group: Optional[str] = field(
        default=None,
        metadata=CompareBehavior.Exclude.meta(),
    )
    meta: Dict[str, Any] = field(default_factory=dict, metadata=MergeBehavior.Update.meta())

    tags: Union[List[str], str] = field(
        default_factory=list_str,
        metadata=metas(ShowBehavior.Hide, MergeBehavior.Append, CompareBehavior.Exclude),
    )


@dataclass
class Metric(GraphResource):
    name: str
    description: str
    label: str
    type: MetricType
    type_params: MetricTypeParams
    filter: Optional[WhereFilterIntersection] = None
    metadata: Optional[SourceFileMetadata] = None
    time_granularity: Optional[str] = None
    resource_type: Literal[NodeType.Metric]
    config: MetricConfig = field(default_factory=MetricConfig)
    unrendered_config: Dict[str, Any] = field(default_factory=dict)
    sources: List[List[str]] = field(default_factory=list)
    depends_on: DependsOn = field(default_factory=DependsOn)
    refs: List[RefArgs] = field(default_factory=list)
    metrics: List[List[str]] = field(default_factory=list)
    created_at: float = field(default_factory=lambda: time.time())
    group: Optional[str] = None

    # These fields are only used in v1 metrics.
    meta: Dict[str, Any] = field(default_factory=dict, metadata=MergeBehavior.Update.meta())

    tags: List[str] = field(default_factory=list)

    @property
    def input_metrics(self) -> List[MetricInput]:
        return self.type_params.metrics or []

    @property
    def input_measures(self) -> List[MetricInputMeasure]:
        return self.type_params.input_measures

    @property
    def measure_references(self) -> List[MeasureReference]:
        return [x.measure_reference() for x in self.input_measures]
