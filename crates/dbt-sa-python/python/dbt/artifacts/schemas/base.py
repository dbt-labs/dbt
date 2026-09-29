"""Shared base for the artifact schemas.

These mirror dbt's **Rust** structs, which are the contract — not dbt-core 1.x's
Python schemas. Where they disagree, Rust wins.

Timestamps stay `str`: Rust emits ISO-8601, and this avoids relying on msgpack
datetime behaviour.
"""

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, Optional, TypeVar, Union

from mashumaro.mixins.msgpack import DataClassMessagePackMixin

T = TypeVar("T", bound="ArtifactBase")


@dataclass
class ArtifactBase(DataClassMessagePackMixin):
    """Provides to_dict/from_dict (via DataClassDictMixin) and to/from_msgpack."""

    @classmethod
    def read(cls: type[T], path: Union[str, Path]) -> T:
        """Load from the JSON on disk; equal to what `from_msgpack` gives for the same run."""
        with open(path, encoding="utf-8") as fh:
            return cls.from_dict(json.load(fh))


@dataclass
class BaseArtifactMetadata(ArtifactBase):
    # Optionals need defaults: Rust's skip_serializing_none omits unset keys.
    dbt_schema_version: str
    dbt_version: str = ""
    generated_at: Optional[str] = None
    invocation_id: Optional[str] = None
    invocation_started_at: Optional[str] = None
    env: Dict[str, str] = field(default_factory=dict)


@dataclass
class TimingInfo(ArtifactBase):
    name: str
    started_at: Optional[str] = None
    completed_at: Optional[str] = None
