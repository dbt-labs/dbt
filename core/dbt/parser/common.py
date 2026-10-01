from dataclasses import dataclass
from typing import Any, Dict, Generic, List, Optional, TypeVar, Union

from metricflow_semantic_interfaces.type_enums import (
    DimensionType,
    EntityType,
    TimeGranularity,
)

from dbt.artifacts.resources import (
    ColumnConfig,
    ColumnDimension,
    ColumnEntity,
    ColumnInfo,
    NodeVersion,
)
from dbt.contracts.graph.nodes import UnpatchedSourceDefinition
from dbt.contracts.graph.unparsed import (
    HasColumnDocs,
    HasColumnProps,
    HasColumnTests,
    UnparsedAnalysisUpdate,
    UnparsedColumn,
    UnparsedColumnEntityV2,
    UnparsedDimensionV2,
    UnparsedExposure,
    UnparsedFunctionUpdate,
    UnparsedMacroUpdate,
    UnparsedModelUpdate,
    UnparsedNodeUpdate,
    UnparsedSingularTestUpdate,
)
from dbt.exceptions import ParsingError
from dbt.node_types import NodeType
from dbt.parser.search import FileBlock
from dbt_common.contracts.constraints import ColumnLevelConstraint, ConstraintType
from dbt_common.exceptions import DbtInternalError

schema_file_keys_to_resource_types = {
    "models": NodeType.Model,
    "seeds": NodeType.Seed,
    "snapshots": NodeType.Snapshot,
    "sources": NodeType.Source,
    "macros": NodeType.Macro,
    "analyses": NodeType.Analysis,
    "exposures": NodeType.Exposure,
    "metrics": NodeType.Metric,
    "semantic_models": NodeType.SemanticModel,
    "saved_queries": NodeType.SavedQuery,
    "functions": NodeType.Function,
}

resource_types_to_schema_file_keys = {
    v: k for (k, v) in schema_file_keys_to_resource_types.items()
}

schema_file_keys = list(schema_file_keys_to_resource_types.keys())


def normalize_tags(*sources: Union[List[str], str, None]) -> List[str]:
    """Return a deduplicated list of tags from all sources, preserving order."""
    tags: List[str] = []
    for source in sources:
        if source is None:
            continue
        tags.extend([source] if isinstance(source, str) else source)
    return list(dict.fromkeys(tags))


def trimmed(inp: str) -> str:
    if len(inp) < 50:
        return inp
    return inp[:44] + "..." + inp[-3:]


TestDef = Union[str, Dict[str, Any]]


Target = TypeVar(
    "Target",
    UnparsedNodeUpdate,
    UnparsedMacroUpdate,
    UnparsedAnalysisUpdate,
    UnpatchedSourceDefinition,
    UnparsedExposure,
    UnparsedModelUpdate,
    UnparsedFunctionUpdate,
    UnparsedSingularTestUpdate,
)


ColumnTarget = TypeVar(
    "ColumnTarget",
    UnparsedModelUpdate,
    UnparsedNodeUpdate,
    UnparsedAnalysisUpdate,
    UnpatchedSourceDefinition,
)

Versioned = TypeVar("Versioned", bound=UnparsedModelUpdate)

Testable = TypeVar("Testable", UnparsedNodeUpdate, UnpatchedSourceDefinition, UnparsedModelUpdate)


@dataclass
class YamlBlock(FileBlock):
    data: Dict[str, Any]

    @classmethod
    def from_file_block(cls, src: FileBlock, data: Dict[str, Any]):
        return cls(
            file=src.file,
            data=data,
        )


@dataclass
class TargetBlock(YamlBlock, Generic[Target]):
    target: Target

    @property
    def name(self):
        return self.target.name

    @property
    def columns(self):
        return []

    @property
    def data_tests(self) -> List[TestDef]:
        return []

    @property
    def tests(self) -> List[TestDef]:
        return []

    @classmethod
    def from_yaml_block(cls, src: YamlBlock, target: Target) -> "TargetBlock[Target]":
        return cls(
            file=src.file,
            data=src.data,
            target=target,
        )


@dataclass
class TargetColumnsBlock(TargetBlock[ColumnTarget], Generic[ColumnTarget]):
    @property
    def columns(self):
        if self.target.columns is None:
            return []
        else:
            return self.target.columns


@dataclass
class TestBlock(TargetColumnsBlock[Testable], Generic[Testable]):
    @property
    def data_tests(self) -> List[TestDef]:
        if self.target.data_tests is None:
            return []
        else:
            return self.target.data_tests

    @property
    def quote_columns(self) -> Optional[bool]:
        return self.target.quote_columns

    @classmethod
    def from_yaml_block(cls, src: YamlBlock, target: Testable) -> "TestBlock[Testable]":
        return cls(
            file=src.file,
            data=src.data,
            target=target,
        )


@dataclass
class VersionedTestBlock(TestBlock, Generic[Versioned]):
    @property
    def columns(self):
        if not self.target.versions:
            return super().columns
        else:
            raise DbtInternalError(".columns for VersionedTestBlock with versions")

    @property
    def data_tests(self) -> List[TestDef]:
        if not self.target.versions:
            return super().data_tests
        else:
            raise DbtInternalError(".data_tests for VersionedTestBlock with versions")

    @classmethod
    def from_yaml_block(cls, src: YamlBlock, target: Versioned) -> "VersionedTestBlock[Versioned]":
        return cls(
            file=src.file,
            data=src.data,
            target=target,
        )


@dataclass
class GenericTestBlock(TestBlock[Testable], Generic[Testable]):
    data_test: Dict[str, Any]
    column_name: Optional[str]
    tags: List[str]
    version: Optional[NodeVersion]

    @classmethod
    def from_test_block(
        cls,
        src: TestBlock,
        data_test: Dict[str, Any],
        column_name: Optional[str],
        tags: List[str],
        version: Optional[NodeVersion],
    ) -> "GenericTestBlock":
        return cls(
            file=src.file,
            data=src.data,
            target=src.target,
            data_test=data_test,
            column_name=column_name,
            tags=tags,
            version=version,
        )


class ParserRef:
    """A helper object to hold parse-time references."""

    def __init__(self) -> None:
        self.column_info: Dict[str, ColumnInfo] = {}

    def _add(self, column: HasColumnProps) -> None:
        tags: List[str] = getattr(column, "tags", [])
        quote: Optional[bool] = None
        granularity: Optional[TimeGranularity] = None
        dimension: Optional[Union[DimensionType, ColumnDimension]] = None
        entity: Optional[Union[EntityType, ColumnEntity]] = None
        if isinstance(column, UnparsedColumn):
            quote = column.quote
            granularity = TimeGranularity(column.granularity) if column.granularity else None
            if isinstance(column.dimension, UnparsedDimensionV2):
                dimension = ColumnDimension(
                    name=column.dimension.name or column.name,
                    type=(DimensionType(column.dimension.type)),
                    description=column.dimension.description or column.description,
                    label=column.dimension.label,
                    is_partition=column.dimension.is_partition,
                    config=column.dimension.config or {},
                    validity_params=(
                        ColumnDimension.ColumnDimensionValidityParams(
                            is_start=column.dimension.validity_params.is_start,
                            is_end=column.dimension.validity_params.is_end,
                        )
                        if column.dimension.validity_params
                        else None
                    ),
                )
            elif isinstance(column.dimension, str):
                dimension = DimensionType(column.dimension)

            if isinstance(column.entity, UnparsedColumnEntityV2):
                entity = ColumnEntity(
                    name=column.entity.name or column.name,
                    type=EntityType(column.entity.type),
                    description=column.entity.description or column.description,
                    label=column.entity.label,
                    config=column.entity.config or {},
                )
            elif isinstance(column.entity, str):
                entity = EntityType(column.entity)

        if any(
            c
            for c in column.constraints
            if "type" not in c or not ConstraintType.is_valid(c["type"])
        ):
            raise ParsingError(f"Invalid constraint type on column {column.name}")

        # Merge meta and tags from column and config
        column_config_meta = (
            column.config["meta"] if isinstance(column.config.get("meta"), dict) else {}
        )
        column_config_tags = []
        if "tags" in column.config:
            if isinstance(column.config["tags"], list):
                column_config_tags = column.config["tags"]
            elif isinstance(column.config["tags"], str):
                column_config_tags = [column.config["tags"]]

        column_meta = {**column.meta, **column_config_meta}
        column_tags = list(set(tags + column_config_tags))
        self.column_info[column.name] = ColumnInfo(
            name=column.name,
            description=column.description,
            data_type=column.data_type,
            constraints=[ColumnLevelConstraint.from_dict(c) for c in column.constraints],
            meta=column_meta,
            tags=column_tags,
            quote=quote,
            _extra=column.extra,
            granularity=granularity,
            dimension=dimension,
            entity=entity,
            config=ColumnConfig(meta=column_meta, tags=column_tags),
        )

    @classmethod
    def from_target(cls, target: Union[HasColumnDocs, HasColumnTests]) -> "ParserRef":
        refs = cls()
        for column in target.columns:
            refs._add(column)
        return refs

    @classmethod
    def from_versioned_target(cls, target: Versioned, version: NodeVersion) -> "ParserRef":
        refs = cls()
        for base_column in target.get_columns_for_version(version):
            refs._add(base_column)
        return refs
