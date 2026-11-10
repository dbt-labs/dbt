"""`run_results.json` — mirrors the Rust `RunResultsArtifact`."""

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, Iterator, List, Optional, Tuple

from dbt.artifacts.schemas.base import ArtifactBase, BaseArtifactMetadata, TimingInfo


class StaticAnalysisOffReason(str, Enum):
    # Rust's rename_all = "lowercase" lowercases the whole variant, not snake_case.
    CONFIGURED_OFF = "configuredoff"
    UNABLE_TO_FETCH_SCHEMA = "unabletofetchschema"
    NO_DOWNSTREAM = "nodownstream"
    CUSTOM_MATERIALIZATION = "custommaterialization"


@dataclass
class BatchResults(ArtifactBase):
    # Rust `Vec<(String, String)>`; tuples serialize to msgpack arrays.
    successful: List[Tuple[str, str]] = field(default_factory=list)
    failed: List[Tuple[str, str]] = field(default_factory=list)


@dataclass
class RunResultOutput(ArtifactBase):
    """One row of `results`.

    Rust omits skip_serializing_none here, so every key is present (null when
    empty) — consumers like dbt-artifacts rely on that. `static_analysis_off_reason`
    is the exception and is omitted when unset.
    """

    status: str
    unique_id: str
    thread_id: str = ""
    execution_time: float = 0.0
    timing: List[TimingInfo] = field(default_factory=list)
    adapter_response: Dict[str, Any] = field(default_factory=dict)
    message: Optional[str] = None
    failures: Optional[int] = None
    compiled: Optional[bool] = None
    compiled_code: Optional[str] = None
    relation_name: Optional[str] = None
    batch_results: Optional[BatchResults] = None
    static_analysis_off_reason: Optional[StaticAnalysisOffReason] = None


@dataclass
class RunResultsArtifact(ArtifactBase):
    metadata: BaseArtifactMetadata
    results: List[RunResultOutput] = field(default_factory=list)
    elapsed_time: float = 0.0
    # Open-ended: Rust's `__other__` flattens the extra CLI flags in here.
    args: Dict[str, Any] = field(default_factory=dict)

    # A sequence over its rows, like dbt-core's ExecutionResult, so the documented
    # `for r in res.result:` works.
    def __len__(self) -> int:
        return len(self.results)

    def __iter__(self) -> Iterator[RunResultOutput]:
        return iter(self.results)

    def __getitem__(self, idx: int) -> RunResultOutput:
        return self.results[idx]
