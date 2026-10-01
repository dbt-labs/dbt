"""`manifest.json` — mirrors the Rust `DbtManifestV12`.

Only `metadata` is typed. Typing the collections means mirroring the whole
resource tree — ~490 fields across ~43 structs behind `nodes` alone — for little
gain to callers that read `metadata` and index `nodes` by unique_id.
"""

from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional

from dbt.artifacts.schemas.base import ArtifactBase


@dataclass
class ManifestMetadata(ArtifactBase):
    # Flat, not nested: Rust folds BaseMetadata in with #[serde(flatten)].
    dbt_schema_version: str
    dbt_version: str = ""
    generated_at: Optional[str] = None
    invocation_id: Optional[str] = None
    invocation_started_at: Optional[str] = None
    env: Dict[str, str] = field(default_factory=dict)
    project_name: str = ""
    project_id: Optional[str] = None
    user_id: Optional[str] = None
    send_anonymous_usage_stats: Optional[bool] = None
    adapter_type: str = ""
    quoting: Optional[Dict[str, Any]] = None


@dataclass
class Manifest(ArtifactBase):
    metadata: ManifestMetadata
    nodes: Dict[str, Any] = field(default_factory=dict)
    sources: Dict[str, Any] = field(default_factory=dict)
    macros: Dict[str, Any] = field(default_factory=dict)
    unit_tests: Dict[str, Any] = field(default_factory=dict)
    docs: Dict[str, Any] = field(default_factory=dict)
    semantic_models: Dict[str, Any] = field(default_factory=dict)
    saved_queries: Dict[str, Any] = field(default_factory=dict)
    exposures: Dict[str, Any] = field(default_factory=dict)
    metrics: Dict[str, Any] = field(default_factory=dict)
    functions: Dict[str, Any] = field(default_factory=dict)
    child_map: Dict[str, List[str]] = field(default_factory=dict)
    parent_map: Dict[str, List[str]] = field(default_factory=dict)
    group_map: Dict[str, List[str]] = field(default_factory=dict)
    disabled: Dict[str, List[Any]] = field(default_factory=dict)
    selectors: Dict[str, Any] = field(default_factory=dict)
    groups: Dict[str, Any] = field(default_factory=dict)
