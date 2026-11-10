"""`sources.json` — mirrors the Rust `FreshnessResultsArtifact`.

`invoke(["source", "freshness"])` returns this on `result`; it is also on disk, at
`FreshnessResultsArtifact.read("target/sources.json")`.
"""

from dataclasses import dataclass, field
from enum import Enum
from typing import Dict, Iterator, List, Optional

from dbt.artifacts.schemas.base import ArtifactBase, BaseArtifactMetadata, TimingInfo


class FreshnessStatus(str, Enum):
    # Capitalised, unlike run_results: the Rust enum has no rename_all.
    PASS = "Pass"
    WARN = "Warn"
    ERROR = "Error"


class FreshnessPeriod(str, Enum):
    SECOND = "second"
    MINUTE = "minute"
    HOUR = "hour"
    DAY = "day"


@dataclass
class FreshnessRules(ArtifactBase):
    count: Optional[int] = None
    period: Optional[FreshnessPeriod] = None


@dataclass
class FreshnessDefinition(ArtifactBase):
    # error_after/warn_after are always present; Rust emits a default rule when unset.
    error_after: Optional[FreshnessRules] = None
    warn_after: Optional[FreshnessRules] = None
    filter: Optional[str] = None
    loaded_at_field: Optional[str] = None
    loaded_at_query: Optional[str] = None


@dataclass
class FreshnessResultsNode(ArtifactBase):
    # Rust also has `node`, but it is skipped on serialize and never reaches the artifact.
    unique_id: str
    max_loaded_at: Optional[str] = None
    snapshotted_at: Optional[str] = None
    max_loaded_at_time_ago_in_s: float = 0.0
    status: Optional[FreshnessStatus] = None
    criteria: Optional[FreshnessDefinition] = None
    adapter_response: Dict[str, str] = field(default_factory=dict)
    timing: List[TimingInfo] = field(default_factory=list)
    thread_id: str = ""
    execution_time: float = 0.0


@dataclass
class FreshnessResultsArtifact(ArtifactBase):
    metadata: BaseArtifactMetadata
    results: List[FreshnessResultsNode] = field(default_factory=list)
    elapsed_time: float = 0.0

    def __len__(self) -> int:
        return len(self.results)

    def __iter__(self) -> Iterator[FreshnessResultsNode]:
        return iter(self.results)

    def __getitem__(self, idx: int) -> FreshnessResultsNode:
        return self.results[idx]
