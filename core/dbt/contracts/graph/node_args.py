from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional, List

from dbt.artifacts.resources import NodeVersion
from dbt.node_types import NodeType, AccessType


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
    generated_at: datetime = field(default_factory=datetime.now)
    depends_on_nodes: List[str] = field(default_factory=list)
    enabled: bool = True

    @property
    def unique_id(self) -> str:
        unique_id = f"{NodeType.Model}.{self.package_name}.{self.name}"
        if self.version:
            unique_id = f"{unique_id}.v{self.version}"

        return unique_id

    @property
    def fqn(self) -> List[str]:
        fqn = [self.package_name, self.name]
        # Test for None explicitly because version can be 0
        if self.version is not None:
            fqn.append(f"v{self.version}")

        return fqn
