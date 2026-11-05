import datetime
import re
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
from typing import Any, Dict, List, Literal, Optional, Sequence, Union

from metricflow_semantic_interfaces.type_enums import (
    ConversionCalculationType,
    DimensionType,
    PeriodAggregation,
)
from typing_extensions import override

# trigger the PathEncoder
import dbt_common.helper_types  # noqa:F401
from dbt import deprecations
from dbt.artifacts.resources import (
    ConstantPropertyInput,
    Defaults,
    DimensionValidityParams,
    Docs,
    ExposureType,
    ExternalTable,
    FreshnessThreshold,
    FunctionArgument,
    FunctionReturns,
    MacroArgument,
    MaturityType,
    MeasureAggregationParameters,
    NodeVersion,
    Owner,
    Quoting,
    TimeSpine,
    UnitTestInputFixture,
    UnitTestNodeVersions,
    UnitTestOutputFixture,
    UnitTestOverrides,
    list_str,
    metas,
)
from dbt.exceptions import ParsingError
from dbt.node_types import NodeType
from dbt_common.contracts.config.base import CompareBehavior, MergeBehavior
from dbt_common.contracts.config.metadata import ShowBehavior
from dbt_common.contracts.config.properties import AdditionalPropertiesMixin
from dbt_common.contracts.util import Mergeable
from dbt_common.dataclass_schema import (
    ExtensibleDbtClassMixin,
    StrEnum,
    ValidationError,
    dbtClassMixin,
)
from dbt_common.exceptions import DbtInternalError


@dataclass
class UnparsedBaseNode(dbtClassMixin):
    package_name: str
    path: str
    original_file_path: str

    @property
    def file_id(self):
        return f"{self.package_name}://{self.original_file_path}"


@dataclass
class HasCode(dbtClassMixin):
    raw_code: str
    language: str

    @property
    def empty(self):
        return not self.raw_code.strip()


@dataclass
class UnparsedMacro(UnparsedBaseNode, HasCode):
    resource_type: Literal[NodeType.Macro]


@dataclass
class UnparsedGenericTest(UnparsedBaseNode, HasCode):
    resource_type: Literal[NodeType.Macro]


@dataclass
class UnparsedNode(UnparsedBaseNode, HasCode):
    name: str
    resource_type: NodeType

    @property
    def search_name(self):
        return self.name


@dataclass
class UnparsedRunHook(UnparsedNode):
    resource_type: Literal[NodeType.Operation]
    index: Optional[int] = None


@dataclass
class HasColumnProps(AdditionalPropertiesMixin, ExtensibleDbtClassMixin):
    name: str
    description: str = ""
    meta: Dict[str, Any] = field(default_factory=dict)
    data_type: Optional[str] = None
    constraints: List[Dict[str, Any]] = field(default_factory=list)
    docs: Docs = field(default_factory=Docs)
    config: Dict[str, Any] = field(default_factory=dict)
    _extra: Dict[str, Any] = field(default_factory=dict)


TestDef = Union[Dict[str, Any], str]


@dataclass
class HasColumnAndTestProps(HasColumnProps):
    data_tests: List[TestDef] = field(default_factory=list)
    tests: List[TestDef] = field(
        default_factory=list
    )  # back compat for previous name of 'data_tests'


@dataclass
class HasColumnDocs(dbtClassMixin):
    columns: Sequence[HasColumnProps] = field(default_factory=list)


@dataclass
class HasYamlMetadata(dbtClassMixin):
    original_file_path: str
    yaml_key: str
    package_name: str

    @property
    def file_id(self):
        return f"{self.package_name}://{self.original_file_path}"


@dataclass
class HasConfig:
    config: Dict[str, Any] = field(default_factory=dict)


@dataclass
class UnparsedDimensionBase(dbtClassMixin):
    # Should we be limiting name length or otherwise validating this?
    type: str  # actually a DimensionType enum value
    name: Optional[str] = None
    description: Optional[str] = None
    label: Optional[str] = None
    is_partition: bool = False
    config: Dict[str, Any] = field(default_factory=dict)


@dataclass
class UnparsedDimensionTypeParams(dbtClassMixin):
    """Used for dbt Semantic Layer dimensions (v1 YAML)."""

    time_granularity: str  # TimeGranularity enum
    validity_params: Optional[DimensionValidityParams] = None


@dataclass(kw_only=True)
class UnparsedDimension(UnparsedDimensionBase):
    """Used for dbt Semantic Layer dimensions (v1 YAML)."""

    name: str
    type_params: Optional[UnparsedDimensionTypeParams] = None
    expr: Optional[str] = None


@dataclass
class UnparsedDimensionHierarchyLevel(dbtClassMixin):
    dimension: str
    parent: Optional[str] = None


@dataclass
class UnparsedDimensionHierarchy(dbtClassMixin):
    name: str
    levels: List[Union[UnparsedDimensionHierarchyLevel, str]]
    label: Optional[str] = None


@dataclass
class UnparsedDimensionV2(UnparsedDimensionBase):
    """Used for dbt Semantic Layer dimensions (v2 YAML)."""

    validity_params: Optional[DimensionValidityParams] = None


@dataclass(kw_only=True)
class UnparsedDerivedDimensionV2(UnparsedDimensionV2):
    """Used for dbt Semantic Layer derived dimensions (v2 YAML)."""

    name: str
    expr: str
    granularity: Optional[str] = None  # str is really a TimeGranularity Enum

    @classmethod
    @override
    def validate(cls, data):
        super().validate(data)
        # validity_params may only be set when the derived dimension has a granularity
        if data.get("validity_params") is not None and not data.get("granularity"):
            dim_name = data.get("name")
            raise ValidationError(
                f"Derived dimension {dim_name} has validity_params, "
                "so it must specify a granularity."
            )


@dataclass
class UnparsedEntityBase(dbtClassMixin):
    type: str  # EntityType enum
    name: Optional[str] = None
    description: Optional[str] = None
    label: Optional[str] = None
    config: Dict[str, Any] = field(default_factory=dict)


@dataclass(kw_only=True)
class UnparsedEntity(UnparsedEntityBase):
    """Used for dbt Semantic Layer entities (v1 YAML only)."""

    name: str
    role: Optional[str] = None
    expr: Optional[str] = None


@dataclass
class UnparsedColumnEntityV2(UnparsedEntityBase):
    """Used for dbt Semantic Layer column entities (v2 YAML)."""

    pass


# kw_only allows this child to define required fields
@dataclass(kw_only=True)
class UnparsedDerivedEntityV2(UnparsedEntityBase):
    """Used for dbt Semantic Layer derived entities (v2 YAML)."""

    expr: str


@dataclass
class UnparsedColumn(HasConfig, HasColumnAndTestProps):
    quote: Optional[bool] = None
    tags: List[str] = field(default_factory=list)
    granularity: Optional[str] = None  # str is really a TimeGranularity Enum
    # Note 1: Dimension str is a DimensionType enum value
    # Note 2: Don't ask me why, but str must come after UnparsedDimensionV2 here or else
    # this will be read as a dict object instead of a UnparsedDimensionV2 object
    # Only used in v2 semantic layer.
    dimension: Union[UnparsedDimensionV2, str, None] = None
    # UnparsedColumnEntityV2 must come before str to parse correctly.  str is assumed to be EntityType enum value
    # Only used in v2 semantic layer.
    entity: Union[UnparsedColumnEntityV2, str, None] = None

    @classmethod
    @override
    def validate(cls, data):
        super().validate(data)
        if (dimension := data.get("dimension")) is not None:
            if isinstance(dimension, dict):
                dim_type_str = dimension.get("type")
                dim_name = dimension.get("name")
            else:
                dim_type_str = dimension
                dim_name = data.get("name")
            dim_type = DimensionType(dim_type_str) if dim_type_str is not None else None
            if dim_type is DimensionType.TIME and not data.get("granularity"):
                raise ValidationError(
                    f"Dimension {dim_name} is a time dimension attached to "
                    f"column {data.get('name')}, "
                    "so that column must specify a granularity."
                )
            # validity_params may only be set when the column has a granularity
            if (
                isinstance(dimension, dict)
                and dimension.get("validity_params") is not None
                and not data.get("granularity")
            ):
                dim_name = dimension.get("name") or data.get("name")
                raise ValidationError(
                    f"Dimension {dim_name} has validity_params attached to "
                    f"column {data.get('name')}, "
                    "so that column must specify a granularity."
                )


@dataclass
class HasColumnTests(dbtClassMixin):
    columns: Sequence[UnparsedColumn] = field(default_factory=list)


@dataclass
class MetricFilter(dbtClassMixin):
    field: str
    operator: str
    # TODO : Can we make this Any?
    value: str


class MetricTimePeriod(StrEnum):
    day = "day"
    week = "week"
    month = "month"
    year = "year"

    def plural(self) -> str:
        return str(self) + "s"


@dataclass
class MetricTime(dbtClassMixin, Mergeable):
    count: Optional[int] = None
    period: Optional[MetricTimePeriod] = None

    def __bool__(self):
        return self.count is not None and self.period is not None


@dataclass
class UnparsedMetricInputMeasure(dbtClassMixin):
    name: str
    # Note: `Union` must be the outermost part of the type annotation for serialization to work properly.
    filter: Union[str, List[str], None] = None
    alias: Optional[str] = None
    join_to_timespine: bool = False
    fill_nulls_with: Optional[int] = None


@dataclass
class UnparsedMetricInput(dbtClassMixin):
    name: str
    # Note: `Union` must be the outermost part of the type annotation for serialization to work properly.
    filter: Union[str, List[str], None] = None
    alias: Optional[str] = None
    offset_window: Optional[str] = None
    offset_to_grain: Optional[str] = None


@dataclass
class UnparsedConversionTypeParams(dbtClassMixin):
    """Only used in v1 Semantic YAML"""

    base_measure: Union[UnparsedMetricInputMeasure, str]
    conversion_measure: Union[UnparsedMetricInputMeasure, str]
    entity: str
    calculation: str = (
        ConversionCalculationType.CONVERSION_RATE.value
    )  # ConversionCalculationType Enum
    window: Optional[str] = None
    constant_properties: Optional[List[ConstantPropertyInput]] = None


@dataclass
class UnparsedCumulativeTypeParams(dbtClassMixin):
    """Only used in v1 Semantic YAML"""

    window: Optional[str] = None
    grain_to_date: Optional[str] = None
    period_agg: str = PeriodAggregation.FIRST.value


@dataclass
class UnparsedMetricTypeParams(dbtClassMixin):
    """Used on v1 Semantic Metric YAML."""

    measure: Optional[Union[UnparsedMetricInputMeasure, str]] = None
    numerator: Optional[Union[UnparsedMetricInput, str]] = None
    denominator: Optional[Union[UnparsedMetricInput, str]] = None
    expr: Optional[Union[str, bool]] = None
    window: Optional[str] = None
    grain_to_date: Optional[str] = None  # str is really a TimeGranularity Enum
    metrics: Optional[List[Union[UnparsedMetricInput, str]]] = None
    conversion_type_params: Optional[UnparsedConversionTypeParams] = None
    cumulative_type_params: Optional[UnparsedCumulativeTypeParams] = None


@dataclass(kw_only=True)
class UnparsedMetricBase(dbtClassMixin):

    name: str
    type: str = "simple"
    label: Optional[str] = None  # in v1 this is required, but in v2 it is optional
    description: str = ""
    # Note: `Union` must be the outermost part of the type annotation for serialization to work properly.
    filter: Union[str, List[str], None] = None
    time_granularity: Optional[str] = None

    config: Dict[str, Any] = field(default_factory=dict)

    @classmethod
    def validate(cls, data):
        super().validate(data)
        if "name" in data:
            errors = []
            if " " in data["name"]:
                errors.append("cannot contain spaces")
            # This handles failing queries due to too long metric names.
            # It only occurs in BigQuery and Snowflake (Postgres/Redshift truncate)
            if len(data["name"]) > 250:
                errors.append("cannot contain more than 250 characters")
            if not (re.match(r"^[A-Za-z]", data["name"])):
                errors.append("must begin with a letter")
            if not (re.match(r"[\w]+$", data["name"])):
                errors.append("must contain only letters, numbers and underscores")

            if errors:
                raise ValidationError(
                    f"The metric name '{data['name']}' is invalid.  It {', '.join(e for e in errors)}"
                )


@dataclass(kw_only=True)
class UnparsedMetric(UnparsedMetricBase):
    """Old-style YAML metric; prefer UnparsedMetricV2 instead as of late 2025."""

    label: str

    type_params: UnparsedMetricTypeParams  # old-style YAML
    # metadata: Optional[Unparsedetadata] = None # TODO
    meta: Dict[str, Any] = field(default_factory=dict)
    tags: List[str] = field(default_factory=list)


@dataclass
class UnparsedNonAdditiveDimensionV2(dbtClassMixin):
    name: str
    window_agg: str  # AggregationType enum
    group_by: List[str] = field(default_factory=list)


@dataclass
class UnparsedMetricV2(UnparsedMetricBase):
    hidden: bool = False
    agg: Optional[str] = None

    percentile: Optional[float] = None
    percentile_type: Optional[str] = None

    join_to_timespine: Optional[bool] = None
    fill_nulls_with: Optional[int] = None
    expr: Optional[Union[str, int]] = None

    non_additive_dimension: Optional[UnparsedNonAdditiveDimensionV2] = None
    agg_time_dimension: Optional[str] = None

    # For cumulative metrics
    window: Optional[str] = None
    grain_to_date: Optional[str] = None
    period_agg: str = PeriodAggregation.FIRST.value
    input_metric: Optional[Union[UnparsedMetricInput, str]] = None

    # For ratio metrics
    numerator: Optional[Union[UnparsedMetricInput, str]] = None
    denominator: Optional[Union[UnparsedMetricInput, str]] = None

    # For derived metrics
    input_metrics: Optional[List[Union[UnparsedMetricInput, str]]] = None

    # For conversion metrics
    entity: Optional[str] = None
    calculation: Optional[str] = None
    base_metric: Optional[Union[UnparsedMetricInput, str]] = None
    conversion_metric: Optional[Union[UnparsedMetricInput, str]] = None
    constant_properties: Optional[List[ConstantPropertyInput]] = None

    @classmethod
    @override
    def validate(cls, data):
        super().validate(data)
        if data["type"] == "simple" and data.get("agg") is None:
            raise ValidationError("Simple metrics must have an agg param.")


@dataclass
class UnparsedVersion(dbtClassMixin):
    v: NodeVersion
    defined_in: Optional[str] = None
    description: str = ""
    access: Optional[str] = None
    config: Dict[str, Any] = field(default_factory=dict)
    constraints: List[Dict[str, Any]] = field(default_factory=list)
    docs: Docs = field(default_factory=Docs)
    data_tests: Optional[List[TestDef]] = None
    tests: Optional[List[TestDef]] = None  # back compat for previous name of 'data_tests'
    columns: Sequence[Union[dbt_common.helper_types.IncludeExclude, UnparsedColumn]] = field(
        default_factory=list
    )
    deprecation_date: Optional[datetime.datetime] = None

    def __lt__(self, other):
        try:
            return float(self.v) < float(other.v)
        except ValueError:
            return str(self.v) < str(other.v)

    @property
    def include_exclude(self) -> dbt_common.helper_types.IncludeExclude:
        return self._include_exclude

    @property
    def unparsed_columns(self) -> List:
        return self._unparsed_columns

    @property
    def formatted_v(self) -> str:
        return f"v{self.v}"

    def __post_init__(self):
        has_include_exclude = False
        self._include_exclude = dbt_common.helper_types.IncludeExclude(include="*")
        self._unparsed_columns = []
        for column in self.columns:
            if isinstance(column, dbt_common.helper_types.IncludeExclude):
                if not has_include_exclude:
                    self._include_exclude = column
                    has_include_exclude = True
                else:
                    raise ParsingError("version can have at most one include/exclude element")
            else:
                self._unparsed_columns.append(column)

        self.deprecation_date = normalize_date(self.deprecation_date)


@dataclass
class UnparsedAnalysisUpdate(HasConfig, HasColumnDocs, HasColumnProps, HasYamlMetadata):
    access: Optional[str] = None


@dataclass
class UnparsedSingularTestUpdate(HasConfig, HasColumnProps, HasYamlMetadata):
    pass


@dataclass
class UnparsedNodeUpdate(HasConfig, HasColumnTests, HasColumnAndTestProps, HasYamlMetadata):
    quote_columns: Optional[bool] = None
    access: Optional[str] = None


@dataclass
class UnparsedDerivedSemantics(dbtClassMixin):
    entities: List[UnparsedDerivedEntityV2] = field(default_factory=list)
    dimensions: List[UnparsedDerivedDimensionV2] = field(default_factory=list)


@dataclass
class UnparsedSemanticResourceConfig(dbtClassMixin):
    meta: Dict[str, Any] = field(default_factory=dict)


@dataclass(kw_only=True)
class UnparsedSemanticModelConfig(dbtClassMixin):
    name: Optional[str] = None
    enabled: bool = True
    group: Optional[str] = None
    config: Optional[UnparsedSemanticResourceConfig] = None

    @classmethod
    @override
    def validate(cls, data: Any) -> None:
        if isinstance(data, dict):
            allowed = set(cls.__dataclass_fields__.keys())
            extra = set(data.keys()) - allowed
            if extra:
                raise ValidationError(
                    f"Unknown field(s) in semantic_model config: {', '.join(sorted(extra))}. "
                    f"Valid fields are: {', '.join(sorted(allowed))}."
                )
        super().validate(data)


@dataclass
class UnparsedModelUpdate(UnparsedNodeUpdate):
    quote_columns: Optional[bool] = None
    access: Optional[str] = None
    latest_version: Optional[NodeVersion] = None
    versions: Sequence[UnparsedVersion] = field(default_factory=list)
    deprecation_date: Optional[datetime.datetime] = None
    time_spine: Optional[TimeSpine] = None
    # True indicates that the semantic model is enabeld and will have it's values populated
    # directly from the model.
    # Using an UnparsedSemanticModelConfig object allows user to override some of the
    # values instead.
    semantic_model: Union[UnparsedSemanticModelConfig, bool, None] = None
    primary_entity: Optional[str] = None
    agg_time_dimension: Optional[str] = None
    metrics: Optional[List[UnparsedMetricV2]] = None
    derived_semantics: Optional[UnparsedDerivedSemantics] = None

    def __post_init__(self) -> None:
        if self.latest_version:
            version_values = [version.v for version in self.versions]
            if self.latest_version not in version_values:
                raise ParsingError(
                    f"latest_version: {self.latest_version} is not one of model '{self.name}' versions: {version_values} "
                )

        seen_versions = set()
        for version in self.versions:
            if str(version.v) in seen_versions:
                raise ParsingError(
                    f"Found duplicate version: '{version.v}' in versions list of model '{self.name}'"
                )
            seen_versions.add(str(version.v))

        self._version_map = {version.v: version for version in self.versions}

        self.deprecation_date = normalize_date(self.deprecation_date)

        if self.time_spine:
            columns = (
                self.get_columns_for_version(self.latest_version)
                if self.latest_version
                else self.columns
            )
            column_names_to_columns = {column.name: column for column in columns}
            if self.time_spine.standard_granularity_column not in column_names_to_columns:
                raise ParsingError(
                    f"Time spine standard granularity column must be defined on the model. Got invalid "
                    f"column name '{self.time_spine.standard_granularity_column}' for model '{self.name}'. Valid names"
                    f"{' for latest version' if self.latest_version else ''}: {list(column_names_to_columns.keys())}."
                )
            standard_column = column_names_to_columns[self.time_spine.standard_granularity_column]
            if not standard_column.granularity:
                raise ParsingError(
                    f"Time spine standard granularity column must have a granularity defined. Please add one for "
                    f"column '{self.time_spine.standard_granularity_column}' in model '{self.name}'."
                )
            custom_granularity_columns_not_found = []
            for custom_granularity in self.time_spine.custom_granularities:
                column_name = (
                    custom_granularity.column_name
                    if custom_granularity.column_name
                    else custom_granularity.name
                )
                if column_name not in column_names_to_columns:
                    custom_granularity_columns_not_found.append(column_name)
            if custom_granularity_columns_not_found:
                raise ParsingError(
                    "Time spine custom granularity columns do not exist in the model. "
                    f"Columns not found: {custom_granularity_columns_not_found}; "
                    f"Available columns: {list(column_names_to_columns.keys())}"
                )

    def get_columns_for_version(self, version: NodeVersion) -> List[UnparsedColumn]:
        if version not in self._version_map:
            raise DbtInternalError(
                f"get_columns_for_version called for version '{version}' not in version map"
            )

        version_columns = []
        unparsed_version = self._version_map[version]
        for base_column in self.columns:
            if unparsed_version.include_exclude.includes(base_column.name):
                version_columns.append(base_column)

        for column in unparsed_version.unparsed_columns:
            version_columns.append(column)

        return version_columns

    def get_tests_for_version(self, version: NodeVersion) -> List[TestDef]:
        if version not in self._version_map:
            raise DbtInternalError(
                f"get_tests_for_version called for version '{version}' not in version map"
            )
        unparsed_version = self._version_map[version]
        return (
            unparsed_version.data_tests
            if unparsed_version.data_tests is not None
            else self.data_tests
        )


@dataclass
class UnparsedMacroUpdate(HasConfig, HasColumnProps, HasYamlMetadata):
    arguments: List[MacroArgument] = field(default_factory=list)


@dataclass
class UnparsedSourceTableDefinition(HasColumnTests, HasColumnAndTestProps):
    config: Dict[str, Any] = field(default_factory=dict)
    loaded_at_field: Optional[str] = None
    loaded_at_field_present: Optional[bool] = None
    loaded_at_query: Optional[str] = None
    identifier: Optional[str] = None
    quoting: Quoting = field(default_factory=Quoting)
    freshness: Optional[FreshnessThreshold] = field(default_factory=FreshnessThreshold)
    external: Optional[ExternalTable] = None
    tags: List[str] = field(default_factory=list)

    def __post_serialize__(self, dct: Dict, context: Optional[Dict] = None):
        dct = super().__post_serialize__(dct, context)
        if "freshness" not in dct and self.freshness is None:
            dct["freshness"] = None
        return dct


@dataclass
class UnparsedSourceDefinition(dbtClassMixin):
    name: str
    description: str = ""
    meta: Dict[str, Any] = field(default_factory=dict)
    database: Optional[str] = None
    schema: Optional[str] = None
    loader: str = ""
    quoting: Quoting = field(default_factory=Quoting)
    freshness: Optional[FreshnessThreshold] = field(default_factory=FreshnessThreshold)
    loaded_at_field: Optional[str] = None
    loaded_at_field_present: Optional[bool] = None
    loaded_at_query: Optional[str] = None
    tables: List[UnparsedSourceTableDefinition] = field(default_factory=list)
    tags: List[str] = field(default_factory=list)
    config: Dict[str, Any] = field(default_factory=dict)
    unrendered_database: Optional[str] = None
    unrendered_schema: Optional[str] = None

    @classmethod
    def validate(cls, data):
        super(UnparsedSourceDefinition, cls).validate(data)

        if data.get("loaded_at_field", None) == "":
            raise ValidationError("loaded_at_field cannot be an empty string.")
        if "tables" in data:
            for table in data["tables"]:
                if table.get("loaded_at_field", None) == "":
                    raise ValidationError("loaded_at_field cannot be an empty string.")

    @property
    def yaml_key(self) -> "str":
        return "sources"

    def __post_serialize__(self, dct: Dict, context: Optional[Dict] = None):
        dct = super().__post_serialize__(dct, context)
        if "freshness" not in dct and self.freshness is None:
            dct["freshness"] = None
        return dct


@dataclass
class SourceTablePatch(dbtClassMixin):
    name: str
    description: Optional[str] = None
    meta: Optional[Dict[str, Any]] = None
    data_type: Optional[str] = None
    docs: Optional[Docs] = None
    loaded_at_field: Optional[str] = None
    loaded_at_field_present: Optional[bool] = None
    loaded_at_query: Optional[str] = None
    identifier: Optional[str] = None
    quoting: Quoting = field(default_factory=Quoting)
    freshness: Optional[FreshnessThreshold] = field(default_factory=FreshnessThreshold)
    external: Optional[ExternalTable] = None
    tags: Optional[List[str]] = None
    data_tests: Optional[List[TestDef]] = None
    tests: Optional[List[TestDef]] = None  # back compat for previous name of 'data_tests'
    columns: Optional[Sequence[UnparsedColumn]] = None

    def to_patch_dict(self) -> Dict[str, Any]:
        dct = self.to_dict(omit_none=True)
        remove_keys = "name"
        for key in remove_keys:
            if key in dct:
                del dct[key]

        if self.freshness is None:
            dct["freshness"] = None

        return dct


@dataclass
class SourcePatch(dbtClassMixin):
    name: str = field(
        metadata=dict(description="The name of the source to override"),
    )
    overrides: str = field(
        metadata=dict(description="The package of the source to override"),
    )
    path: Path = field(
        metadata=dict(description="The path to the patch-defining yml file"),
    )
    config: Dict[str, Any] = field(default_factory=dict)
    description: Optional[str] = None
    meta: Optional[Dict[str, Any]] = None
    database: Optional[str] = None
    schema: Optional[str] = None
    loader: Optional[str] = None
    quoting: Optional[Quoting] = None
    freshness: Optional[Optional[FreshnessThreshold]] = field(default_factory=FreshnessThreshold)
    loaded_at_field: Optional[str] = None
    loaded_at_field_present: Optional[bool] = None
    loaded_at_query: Optional[str] = None
    tables: Optional[List[SourceTablePatch]] = None
    tags: Optional[List[str]] = None

    def to_patch_dict(self) -> Dict[str, Any]:
        dct = self.to_dict(omit_none=True)
        remove_keys = ("name", "overrides", "tables", "path")
        for key in remove_keys:
            if key in dct:
                del dct[key]

        if self.freshness is None:
            dct["freshness"] = None

        return dct

    def get_table_named(self, name: str) -> Optional[SourceTablePatch]:
        if self.tables is not None:
            for table in self.tables:
                if table.name == name:
                    return table
        return None


@dataclass
class UnparsedDocumentation(dbtClassMixin):
    package_name: str
    path: str
    original_file_path: str

    @property
    def file_id(self):
        return f"{self.package_name}://{self.original_file_path}"

    @property
    def resource_type(self):
        return NodeType.Documentation


@dataclass
class UnparsedDocumentationFile(UnparsedDocumentation):
    file_contents: str


# can't use total_ordering decorator here, as str provides an ordering already
# and it's not the one we want.
class Maturity(StrEnum):
    low = "low"
    medium = "medium"
    high = "high"

    def __lt__(self, other):
        if not isinstance(other, Maturity):
            return NotImplemented
        order = (Maturity.low, Maturity.medium, Maturity.high)
        return order.index(self) < order.index(other)

    def __gt__(self, other):
        if not isinstance(other, Maturity):
            return NotImplemented
        return self != other and not (self < other)

    def __ge__(self, other):
        if not isinstance(other, Maturity):
            return NotImplemented
        return self == other or not (self < other)

    def __le__(self, other):
        if not isinstance(other, Maturity):
            return NotImplemented
        return self == other or self < other


@dataclass
class UnparsedExposure(dbtClassMixin):
    name: str
    type: ExposureType
    owner: Owner
    description: str = ""
    label: Optional[str] = None
    maturity: Optional[MaturityType] = None
    meta: Dict[str, Any] = field(default_factory=dict)
    tags: List[str] = field(default_factory=list)
    url: Optional[str] = None
    depends_on: List[str] = field(default_factory=list)
    config: Dict[str, Any] = field(default_factory=dict)

    @classmethod
    def validate(cls, data):
        super(UnparsedExposure, cls).validate(data)
        if "name" in data:
            # name can only contain alphanumeric chars and underscores
            if not (re.match(r"[\w-]+$", data["name"])):
                deprecations.warn("exposure-name", exposure=data["name"])

        if data["owner"].get("name") is None and data["owner"].get("email") is None:
            raise ValidationError("Exposure owner must have at least one of 'name' or 'email'.")


@dataclass
class UnparsedGroup(dbtClassMixin):
    name: str
    owner: Owner
    description: Optional[str] = None
    config: Dict[str, Any] = field(default_factory=dict)

    @classmethod
    def validate(cls, data):
        super(UnparsedGroup, cls).validate(data)
        if data["owner"].get("name") is None and data["owner"].get("email") is None:
            raise ValidationError("Group owner must have at least one of 'name' or 'email'.")
        # TODO DI-4413: the following are not strictly necessary (they will be handled
        #               in dsi validation), but they would be a better user experience
        #               if we did it at parse time.
        # TODO: validate that conversion metrics have base_metric, conversion_metric, and entity
        # TODO: validate that cumulative metrics have all required inputs here
        # TODO: validate that derived metrics have all required inputs here
        # TODO: validate that ratio metrics have all required inputs here
        # TODO: validate that simple metrics have all required inputs here


@dataclass
class UnparsedFunctionReturns(dbtClassMixin):
    returns: FunctionReturns


@dataclass
class UnparsedFunctionOverload(dbtClassMixin):
    """An overload definition within a function's YAML entry."""

    defined_in: str
    arguments: List[FunctionArgument] = field(default_factory=list)
    returns: Optional[FunctionReturns] = None
    description: Optional[str] = None


@dataclass
class UnparsedFunctionUpdate(HasConfig, HasColumnProps, HasYamlMetadata, UnparsedFunctionReturns):
    access: Optional[str] = None
    arguments: List[FunctionArgument] = field(default_factory=list)
    overloads: List[UnparsedFunctionOverload] = field(default_factory=list)


#
# semantic interfaces unparsed objects
#


@dataclass
class UnparsedNonAdditiveDimension(dbtClassMixin):
    name: str
    window_choice: str  # AggregationType enum
    window_groupings: List[str] = field(default_factory=list)


class PercentileType(str, Enum):
    DISCRETE = "discrete"
    CONTINUOUS = "continuous"


@dataclass
class UnparsedMeasure(dbtClassMixin):
    name: str
    agg: str  # actually an enum
    description: Optional[str] = None
    label: Optional[str] = None
    expr: Optional[Union[str, bool, int]] = None
    agg_params: Optional[MeasureAggregationParameters] = None
    non_additive_dimension: Optional[UnparsedNonAdditiveDimension] = None
    agg_time_dimension: Optional[str] = None
    create_metric: bool = False
    config: Dict[str, Any] = field(default_factory=dict)


@dataclass
class UnparsedSemanticModel(dbtClassMixin):
    name: str
    model: str  # looks like "ref(...)"
    config: Dict[str, Any] = field(default_factory=dict)
    description: Optional[str] = None
    label: Optional[str] = None
    defaults: Optional[Defaults] = None
    entities: List[UnparsedEntity] = field(default_factory=list)
    measures: List[UnparsedMeasure] = field(default_factory=list)
    dimensions: List[UnparsedDimension] = field(default_factory=list)
    hierarchies: List[UnparsedDimensionHierarchy] = field(default_factory=list)
    primary_entity: Optional[str] = None


@dataclass
class UnparsedQueryParams(dbtClassMixin):
    metrics: List[str] = field(default_factory=list)
    group_by: List[str] = field(default_factory=list)
    # Note: `Union` must be the outermost part of the type annotation for serialization to work properly.
    where: Union[str, List[str], None] = None
    order_by: List[str] = field(default_factory=list)
    limit: Optional[int] = None


@dataclass
class UnparsedExport(dbtClassMixin):
    """Configuration for writing query results to a table."""

    name: str
    config: Dict[str, Any] = field(default_factory=dict)


@dataclass
class UnparsedSavedQuery(dbtClassMixin):
    name: str
    query_params: UnparsedQueryParams
    description: Optional[str] = None
    label: Optional[str] = None
    exports: List[UnparsedExport] = field(default_factory=list)
    config: Dict[str, Any] = field(default_factory=dict)
    # Note: the order of the types is critical; it's the order that they will be checked against inputs.
    #       if reversed, a single-string tag like `tag: "good"` becomes ['g','o','o','d']
    tags: Union[str, List[str]] = field(
        default_factory=list_str,
        metadata=metas(ShowBehavior.Hide, MergeBehavior.Append, CompareBehavior.Exclude),
    )


def normalize_date(d: Optional[datetime.date]) -> Optional[datetime.datetime]:
    """Convert date to datetime (at midnight), and add local time zone if naive"""
    if d is None:
        return None

    # convert date to datetime
    dt = d if type(d) == datetime.datetime else datetime.datetime(d.year, d.month, d.day)

    if not dt.tzinfo:
        # date is naive, re-interpret as system time zone
        dt = dt.astimezone()

    return dt


@dataclass
class UnparsedUnitTest(dbtClassMixin):
    name: str
    model: str  # name of the model being unit tested
    given: Sequence[UnitTestInputFixture]
    expect: UnitTestOutputFixture
    description: str = ""
    overrides: Optional[UnitTestOverrides] = None
    config: Dict[str, Any] = field(default_factory=dict)
    versions: Optional[UnitTestNodeVersions] = None

    @classmethod
    def validate(cls, data):
        super(UnparsedUnitTest, cls).validate(data)
        if data.get("versions", None):
            if data["versions"].get("include") and data["versions"].get("exclude"):
                raise ValidationError("Unit tests can not both include and exclude versions.")
