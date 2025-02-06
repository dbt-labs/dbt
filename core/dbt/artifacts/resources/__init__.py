from dbt.artifacts.resources.base import BaseResource, Docs, FileHash, GraphResource
from dbt.artifacts.resources.v1.analysis import Analysis
from dbt.artifacts.resources.v1.catalog import Catalog, CatalogIntegration, Catalogs

# alias to latest resource definitions
from dbt.artifacts.resources.v1.components import (
    ColumnInfo,
    CompiledResource,
    Contract,
    DeferRelation,
    DependsOn,
    FreshnessThreshold,
    HasRelationMetadata,
    InjectedCTE,
    NodeVersion,
    ParsedResource,
    ParsedResourceMandatory,
    Quoting,
    RefArgs,
    Time,
)
from dbt.artifacts.resources.v1.config import (
    Hook,
    NodeAndTestConfig,
    NodeConfig,
    TestConfig,
)
from dbt.artifacts.resources.v1.documentation import Documentation
from dbt.artifacts.resources.v1.exposure import (
    Exposure,
    ExposureConfig,
    ExposureType,
    MaturityType,
)
from dbt.artifacts.resources.v1.generic_test import GenericTest, TestMetadata
from dbt.artifacts.resources.v1.group import Group
from dbt.artifacts.resources.v1.hook import HookNode
from dbt.artifacts.resources.v1.macro import Macro, MacroArgument, MacroDependsOn
from dbt.artifacts.resources.v1.metric import (
    ConstantPropertyInput,
    ConversionTypeParams,
    CumulativeTypeParams,
    Metric,
    MetricConfig,
    MetricInput,
    MetricInputMeasure,
    MetricTimeWindow,
    MetricTypeParams,
)
from dbt.artifacts.resources.v1.model import (
    Model,
    ModelConfig,
    ModelFreshness,
    TimeSpine,
)
from dbt.artifacts.resources.v1.owner import Owner
from dbt.artifacts.resources.v1.saved_query import (
    Export,
    ExportConfig,
    QueryParams,
    SavedQuery,
    SavedQueryConfig,
    SavedQueryMandatory,
)
from dbt.artifacts.resources.v1.seed import Seed, SeedConfig
from dbt.artifacts.resources.v1.semantic_layer_components import (
    FileSlice,
    SourceFileMetadata,
    WhereFilter,
    WhereFilterIntersection,
)
from dbt.artifacts.resources.v1.semantic_model import (
    Defaults,
    Dimension,
    DimensionTypeParams,
    DimensionValidityParams,
    Entity,
    Measure,
    MeasureAggregationParameters,
    NodeRelation,
    NonAdditiveDimension,
    SemanticModel,
    SemanticModelConfig,
)
from dbt.artifacts.resources.v1.singular_test import SingularTest
from dbt.artifacts.resources.v1.snapshot import Snapshot, SnapshotConfig
from dbt.artifacts.resources.v1.source_definition import (
    ExternalPartition,
    ExternalTable,
    ParsedSourceMandatory,
    SourceConfig,
    SourceDefinition,
)
from dbt.artifacts.resources.v1.sql_operation import SqlOperation
from dbt.artifacts.resources.v1.unit_test_definition import (
    UnitTestConfig,
    UnitTestDefinition,
    UnitTestFormat,
    UnitTestInputFixture,
    UnitTestNodeVersions,
    UnitTestOutputFixture,
    UnitTestOverrides,
)
