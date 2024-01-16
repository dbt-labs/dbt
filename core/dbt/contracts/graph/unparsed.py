import datetime
import re

from dbt import deprecations
from dbt_common.contracts.config.properties import (
    AdditionalPropertiesAllowed,
    AdditionalPropertiesMixin,
)
from dbt_common.exceptions import DbtInternalError, CompilationError
from dbt_common.dataclass_schema import (
    dbtClassMixin,
    StrEnum,
    ExtensibleDbtClassMixin,
    ValidationError,
)
from dbt.node_types import NodeType
from dbt.contracts.graph.semantic_models import (
    Defaults,
    DimensionValidityParams,
    MeasureAggregationParameters,
)
from dbt.contracts.util import (
    Mergeable,
    Replaceable,
)

# trigger the PathEncoder
import dbt_common.helper_types  # noqa:F401
from dbt.exceptions import ParsingError

from dbt_semantic_interfaces.type_enums import ConversionCalculationType

from dataclasses import dataclass, field
from datetime import timedelta
from pathlib import Path
from typing import Optional, List, Union, Dict, Any, Sequence, Literal


@dataclass
class UnparsedBaseNode(dbtClassMixin, Replaceable):
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
class Docs(dbtClassMixin, Replaceable):
    show: bool = True
    node_color: Optional[str] = None


@dataclass
class HasColumnProps(AdditionalPropertiesMixin, ExtensibleDbtClassMixin, Replaceable):
    name: str
    description: str = ""
    meta: Dict[str, Any] = field(default_factory=dict)
    data_type: Optional[str] = None
    constraints: List[Dict[str, Any]] = field(default_factory=list)
    docs: Docs = field(default_factory=Docs)
    _extra: Dict[str, Any] = field(default_factory=dict)


TestDef = Union[Dict[str, Any], str]


@dataclass
class HasColumnAndTestProps(HasColumnProps):
    data_tests: List[TestDef] = field(default_factory=list)
    tests: List[TestDef] = field(default_factory=list)


@dataclass
class UnparsedColumn(HasColumnAndTestProps):
    quote: Optional[bool] = None
    tags: List[str] = field(default_factory=list)


@dataclass
class HasColumnDocs(dbtClassMixin, Replaceable):
    columns: Sequence[HasColumnProps] = field(default_factory=list)


@dataclass
class HasColumnTests(dbtClassMixin, Replaceable):
    columns: Sequence[UnparsedColumn] = field(default_factory=list)


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


NodeVersion = Union[str, float]
UnitTestModelVersion: Dict[str, List[NodeVersion]]


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
class UnparsedNodeUpdate(HasConfig, HasColumnTests, HasColumnAndTestProps, HasYamlMetadata):
    quote_columns: Optional[bool] = None
    access: Optional[str] = None


@dataclass
class UnparsedModelUpdate(UnparsedNodeUpdate):
    quote_columns: Optional[bool] = None
    access: Optional[str] = None
    latest_version: Optional[NodeVersion] = None
    versions: Sequence[UnparsedVersion] = field(default_factory=list)
    deprecation_date: Optional[datetime.datetime] = None

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
class MacroArgument(dbtClassMixin):
    name: str
    type: Optional[str] = None
    description: str = ""


@dataclass
class UnparsedMacroUpdate(HasConfig, HasColumnProps, HasYamlMetadata):
    arguments: List[MacroArgument] = field(default_factory=list)


class TimePeriod(StrEnum):
    minute = "minute"
    hour = "hour"
    day = "day"

    def plural(self) -> str:
        return str(self) + "s"


@dataclass
class Time(dbtClassMixin, Mergeable):
    count: Optional[int] = None
    period: Optional[TimePeriod] = None

    def exceeded(self, actual_age: float) -> bool:
        if self.period is None or self.count is None:
            return False
        kwargs: Dict[str, int] = {self.period.plural(): self.count}
        difference = timedelta(**kwargs).total_seconds()
        return actual_age > difference

    def __bool__(self):
        return self.count is not None and self.period is not None


@dataclass
class FreshnessThreshold(dbtClassMixin, Mergeable):
    warn_after: Optional[Time] = field(default_factory=Time)
    error_after: Optional[Time] = field(default_factory=Time)
    filter: Optional[str] = None

    def status(self, age: float) -> "dbt.artifacts.results.FreshnessStatus":  # type: ignore # noqa F821
        from dbt.artifacts.results import FreshnessStatus

        if self.error_after and self.error_after.exceeded(age):
            return FreshnessStatus.Error
        elif self.warn_after and self.warn_after.exceeded(age):
            return FreshnessStatus.Warn
        else:
            return FreshnessStatus.Pass

    def __bool__(self):
        return bool(self.warn_after) or bool(self.error_after)


@dataclass
class ExternalPartition(AdditionalPropertiesAllowed, Replaceable):
    name: str = ""
    description: str = ""
    data_type: str = ""
    meta: Dict[str, Any] = field(default_factory=dict)

    def __post_init__(self):
        if self.name == "" or self.data_type == "":
            raise CompilationError("External partition columns must have names and data types")


@dataclass
class ExternalTable(AdditionalPropertiesAllowed, Mergeable):
    location: Optional[str] = None
    file_format: Optional[str] = None
    row_format: Optional[str] = None
    tbl_properties: Optional[str] = None
    partitions: Optional[Union[List[str], List[ExternalPartition]]] = None

    def __bool__(self):
        return self.location is not None


@dataclass
class Quoting(dbtClassMixin, Mergeable):
    database: Optional[bool] = None
    schema: Optional[bool] = None
    identifier: Optional[bool] = None
    column: Optional[bool] = None


@dataclass
class UnparsedSourceTableDefinition(HasColumnTests, HasColumnAndTestProps):
    config: Dict[str, Any] = field(default_factory=dict)
    loaded_at_field: Optional[str] = None
    identifier: Optional[str] = None
    quoting: Quoting = field(default_factory=Quoting)
    freshness: Optional[FreshnessThreshold] = field(default_factory=FreshnessThreshold)
    external: Optional[ExternalTable] = None
    tags: List[str] = field(default_factory=list)

    def __post_serialize__(self, dct):
        dct = super().__post_serialize__(dct)
        if "freshness" not in dct and self.freshness is None:
            dct["freshness"] = None
        return dct


@dataclass
class UnparsedSourceDefinition(dbtClassMixin, Replaceable):
    name: str
    description: str = ""
    meta: Dict[str, Any] = field(default_factory=dict)
    database: Optional[str] = None
    schema: Optional[str] = None
    loader: str = ""
    quoting: Quoting = field(default_factory=Quoting)
    freshness: Optional[FreshnessThreshold] = field(default_factory=FreshnessThreshold)
    loaded_at_field: Optional[str] = None
    tables: List[UnparsedSourceTableDefinition] = field(default_factory=list)
    tags: List[str] = field(default_factory=list)
    config: Dict[str, Any] = field(default_factory=dict)

    @property
    def yaml_key(self) -> "str":
        return "sources"

    def __post_serialize__(self, dct):
        dct = super().__post_serialize__(dct)
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
    identifier: Optional[str] = None
    quoting: Quoting = field(default_factory=Quoting)
    freshness: Optional[FreshnessThreshold] = field(default_factory=FreshnessThreshold)
    external: Optional[ExternalTable] = None
    tags: Optional[List[str]] = None
    data_tests: Optional[List[TestDef]] = None
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
class SourcePatch(dbtClassMixin, Replaceable):
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
class UnparsedDocumentation(dbtClassMixin, Replaceable):
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


class ExposureType(StrEnum):
    Dashboard = "dashboard"
    Notebook = "notebook"
    Analysis = "analysis"
    ML = "ml"
    Application = "application"


class MaturityType(StrEnum):
    Low = "low"
    Medium = "medium"
    High = "high"


@dataclass
class Owner(AdditionalPropertiesAllowed, Replaceable):
    email: Optional[str] = None
    name: Optional[str] = None


@dataclass
class UnparsedExposure(dbtClassMixin, Replaceable):
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
class MetricFilter(dbtClassMixin, Replaceable):
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
    filter: Optional[Union[str, List[str]]] = None
    alias: Optional[str] = None
    join_to_timespine: bool = False
    fill_nulls_with: Optional[int] = None


@dataclass
class UnparsedMetricInput(dbtClassMixin):
    name: str
    filter: Optional[Union[str, List[str]]] = None
    alias: Optional[str] = None
    offset_window: Optional[str] = None
    offset_to_grain: Optional[str] = None  # str is really a TimeGranularity Enum


@dataclass
class ConstantPropertyInput(dbtClassMixin):
    base_property: str
    conversion_property: str


@dataclass
class UnparsedConversionTypeParams(dbtClassMixin):
    base_measure: Union[UnparsedMetricInputMeasure, str]
    conversion_measure: Union[UnparsedMetricInputMeasure, str]
    entity: str
    calculation: str = (
        ConversionCalculationType.CONVERSION_RATE.value
    )  # ConversionCalculationType Enum
    window: Optional[str] = None
    constant_properties: Optional[List[ConstantPropertyInput]] = None


@dataclass
class UnparsedMetricTypeParams(dbtClassMixin):
    measure: Optional[Union[UnparsedMetricInputMeasure, str]] = None
    numerator: Optional[Union[UnparsedMetricInput, str]] = None
    denominator: Optional[Union[UnparsedMetricInput, str]] = None
    expr: Optional[Union[str, bool]] = None
    window: Optional[str] = None
    grain_to_date: Optional[str] = None  # str is really a TimeGranularity Enum
    metrics: Optional[List[Union[UnparsedMetricInput, str]]] = None
    conversion_type_params: Optional[UnparsedConversionTypeParams] = None


@dataclass
class UnparsedMetric(dbtClassMixin):
    name: str
    label: str
    type: str
    type_params: UnparsedMetricTypeParams
    description: str = ""
    filter: Optional[Union[str, List[str]]] = None
    # metadata: Optional[Unparsedetadata] = None # TODO
    meta: Dict[str, Any] = field(default_factory=dict)
    tags: List[str] = field(default_factory=list)
    config: Dict[str, Any] = field(default_factory=dict)

    @classmethod
    def validate(cls, data):
        super(UnparsedMetric, cls).validate(data)
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
            if not (re.match(r"[\w-]+$", data["name"])):
                errors.append("must contain only letters, numbers and underscores")

            if errors:
                raise ParsingError(
                    f"The metric name '{data['name']}' is invalid.  It {', '.join(e for e in errors)}"
                )


@dataclass
class UnparsedGroup(dbtClassMixin, Replaceable):
    name: str
    owner: Owner

    @classmethod
    def validate(cls, data):
        super(UnparsedGroup, cls).validate(data)
        if data["owner"].get("name") is None and data["owner"].get("email") is None:
            raise ValidationError("Group owner must have at least one of 'name' or 'email'.")


#
# semantic interfaces unparsed objects
#


@dataclass
class UnparsedEntity(dbtClassMixin):
    name: str
    type: str  # EntityType enum
    description: Optional[str] = None
    label: Optional[str] = None
    role: Optional[str] = None
    expr: Optional[str] = None


@dataclass
class UnparsedNonAdditiveDimension(dbtClassMixin):
    name: str
    window_choice: str  # AggregationType enum
    window_groupings: List[str] = field(default_factory=list)


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


@dataclass
class UnparsedDimensionTypeParams(dbtClassMixin):
    time_granularity: str  # TimeGranularity enum
    validity_params: Optional[DimensionValidityParams] = None


@dataclass
class UnparsedDimension(dbtClassMixin):
    name: str
    type: str  # actually an enum
    description: Optional[str] = None
    label: Optional[str] = None
    is_partition: bool = False
    type_params: Optional[UnparsedDimensionTypeParams] = None
    expr: Optional[str] = None


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
    primary_entity: Optional[str] = None


@dataclass
class UnparsedQueryParams(dbtClassMixin):
    metrics: List[str] = field(default_factory=list)
    group_by: List[str] = field(default_factory=list)
    where: Optional[Union[str, List[str]]] = None


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


class UnitTestFormat(StrEnum):
    CSV = "csv"
    Dict = "dict"


@dataclass
class UnitTestInputFixture(dbtClassMixin):
    input: str
    rows: Optional[Union[str, List[Dict[str, Any]]]] = None
    format: UnitTestFormat = UnitTestFormat.Dict
    fixture: Optional[str] = None


@dataclass
class UnitTestOutputFixture(dbtClassMixin):
    rows: Optional[Union[str, List[Dict[str, Any]]]] = None
    format: UnitTestFormat = UnitTestFormat.Dict
    fixture: Optional[str] = None


@dataclass
class UnitTestOverrides(dbtClassMixin):
    macros: Dict[str, Any] = field(default_factory=dict)
    vars: Dict[str, Any] = field(default_factory=dict)
    env_vars: Dict[str, Any] = field(default_factory=dict)


@dataclass
class UnitTestNodeVersion(dbtClassMixin):
    include: Optional[List[NodeVersion]] = None
    exclude: Optional[List[NodeVersion]] = None


@dataclass
class UnparsedUnitTest(dbtClassMixin):
    name: str
    model: str  # name of the model being unit tested
    given: Sequence[UnitTestInputFixture]
    expect: UnitTestOutputFixture
    description: str = ""
    versions: Optional[UnitTestNodeVersion] = None
    overrides: Optional[UnitTestOverrides] = None
    config: Dict[str, Any] = field(default_factory=dict)

    @classmethod
    def validate(cls, data):
        super(UnparsedUnitTest, cls).validate(data)
        if data.get("version", None):
            if data["version"].get("include") and data["version"].get("exclude"):
                raise ValidationError("Unit tests can not both include and exclude versions.")


@dataclass
class UnitTestPatch(dbtClassMixin, Replaceable):
    name: str
    model: str
    given: Sequence[UnitTestInputFixture]
    expect: UnitTestOutputFixture
    path: Path = field(
        metadata=dict(description="The path to the patch-defining yml file"),
    )
    config: Dict[str, Any] = field(default_factory=dict)
    overrides: Optional[UnitTestOverrides] = None
    versions: Optional[UnitTestNodeVersion] = None
    description: Optional[str] = None
    schema: Optional[str] = None

    def to_patch_dict(self) -> Dict[str, Any]:
        dct = self.to_dict(omit_none=True)
        remove_keys = ("name", "overrides", "path")
        for key in remove_keys:
            if key in dct:
                del dct[key]

        return dct
