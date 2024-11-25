import time
from dataclasses import dataclass, field
from typing import Any, Dict, List, Literal, Optional

from dbt.artifacts.resources.base import GraphResource
from dbt.artifacts.resources.types import NodeType
from dbt.artifacts.resources.v1.components import DependsOn, RefArgs
from dbt.artifacts.resources.v1.owner import Owner
from dbt_common.contracts.config.base import BaseConfig
from dbt_common.dataclass_schema import StrEnum


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
class ExposureConfig(BaseConfig):
    enabled: bool = True


@dataclass
class Exposure(GraphResource):
    type: ExposureType
    owner: Owner
    resource_type: Literal[NodeType.Exposure]
    description: str = ""
    label: Optional[str] = None
    maturity: Optional[MaturityType] = None
    meta: Dict[str, Any] = field(default_factory=dict)
    tags: List[str] = field(default_factory=list)
    config: ExposureConfig = field(default_factory=ExposureConfig)
    unrendered_config: Dict[str, Any] = field(default_factory=dict)
    vars: Dict[str, Any] = field(default_factory=dict)
    url: Optional[str] = None
    depends_on: DependsOn = field(default_factory=DependsOn)
    refs: List[RefArgs] = field(default_factory=list)
    sources: List[List[str]] = field(default_factory=list)
    metrics: List[List[str]] = field(default_factory=list)
    created_at: float = field(default_factory=lambda: time.time())
