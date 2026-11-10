"""`catalog.json` — mirrors the Rust `DbtCatalog`."""

from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional

from mashumaro.config import BaseConfig

from dbt.artifacts.schemas.base import ArtifactBase, BaseArtifactMetadata


@dataclass
class ColumnMetadata(ArtifactBase):
    # Rust renames this to `type` on the wire.
    data_type: Optional[str] = field(default=None, metadata={"alias": "type"})
    index: int = 0
    name: str = ""
    comment: Optional[str] = None

    class Config(BaseConfig):
        serialize_by_alias = True


@dataclass
class TableMetadata(ArtifactBase):
    # Rust renames this to `type` on the wire.
    materialization_type: Optional[str] = field(default=None, metadata={"alias": "type"})
    schema: str = ""
    name: str = ""
    database: Optional[str] = None
    comment: Optional[str] = None
    owner: Optional[str] = None

    class Config(BaseConfig):
        serialize_by_alias = True


@dataclass
class CatalogNodeStats(ArtifactBase):
    id: str = ""
    label: str = ""
    value: Any = None
    include: bool = False
    description: Optional[str] = None


@dataclass
class CatalogTable(ArtifactBase):
    metadata: Optional[TableMetadata] = None
    # Rust orders these by column index; dict insertion order preserves it.
    columns: Dict[str, ColumnMetadata] = field(default_factory=dict)
    stats: Dict[str, CatalogNodeStats] = field(default_factory=dict)
    unique_id: Optional[str] = None


@dataclass
class CatalogArtifact(ArtifactBase):
    metadata: BaseArtifactMetadata
    nodes: Dict[str, CatalogTable] = field(default_factory=dict)
    sources: Dict[str, CatalogTable] = field(default_factory=dict)
    errors: Optional[List[str]] = None
