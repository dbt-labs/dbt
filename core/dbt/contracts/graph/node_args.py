from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional

from dbt.contracts.graph.unparsed import NodeVersion
from dbt.node_types import AccessType


@dataclass
class ModelNodeArgs:
    name: str
    package_name: str
    identifier: str
    schema: str
    database: Optional[str] = None
    relation_name: Optional[str] = None
    version: Optional[NodeVersion] = None
    latest_version: Optional[NodeVersion] = None
    deprecation_date: Optional[datetime] = None
    access: Optional[str] = AccessType.Protected.value
    generated_at: datetime = field(default_factory=datetime.utcnow)
