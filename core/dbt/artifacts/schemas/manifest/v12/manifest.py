from dataclasses import dataclass, field
from datetime import datetime
from typing import Any, Dict, Iterable, List, Mapping, Optional, Tuple, Union
from uuid import UUID

from dbt import tracking
from dbt.artifacts.resources import (
    Analysis,
    Documentation,
    Exposure,
    Function,
    GenericTest,
    Group,
    HookNode,
    Macro,
    Metric,
    Model,
    SavedQuery,
    Seed,
    SemanticModel,
    SingularTest,
    Snapshot,
    SourceDefinition,
    SqlOperation,
    UnitTestDefinition,
)
from dbt.artifacts.resources.v1.components import Quoting
from dbt.artifacts.schemas.base import (
    ArtifactMixin,
    BaseArtifactMetadata,
    get_artifact_dbt_version,
    get_artifact_schema_version,
    schema_version,
)
from dbt.artifacts.schemas.upgrades import (
    drop_unknown_resource_types,
    upgrade_manifest_json,
    upgrade_manifest_json_dbt_version,
)
from dbt.version import __version__
from dbt_common.exceptions import DbtInternalError

NodeEdgeMap = Dict[str, List[str]]
UniqueID = str
ManifestResource = Union[
    Seed, Analysis, SingularTest, HookNode, Model, SqlOperation, GenericTest, Snapshot, Function
]
DisabledManifestResource = Union[
    ManifestResource,
    SourceDefinition,
    Exposure,
    Metric,
    SavedQuery,
    SemanticModel,
    UnitTestDefinition,
]


@dataclass
class ManifestMetadata(BaseArtifactMetadata):
    """Metadata for the manifest."""

    dbt_schema_version: str = field(
        default_factory=lambda: str(WritableManifest.dbt_schema_version)
    )
    project_name: Optional[str] = field(
        default=None,
        metadata={
            "description": "Name of the root project",
        },
    )
    project_id: Optional[str] = field(
        default=None,
        metadata={
            "description": "A unique identifier for the project, hashed from the project name",
        },
    )
    user_id: Optional[UUID] = field(
        default=None,
        metadata={
            "description": "A unique identifier for the user",
        },
    )
    send_anonymous_usage_stats: Optional[bool] = field(
        default=None,
        metadata=dict(
            description=("Whether dbt is configured to send anonymous usage statistics")
        ),
    )
    adapter_type: Optional[str] = field(
        default=None,
        metadata=dict(description="The type name of the adapter"),
    )
    quoting: Optional[Quoting] = field(
        default_factory=Quoting,
        metadata=dict(description="The quoting configuration for the project"),
    )
    run_started_at: Optional[datetime] = field(
        default=tracking.active_user.run_started_at if tracking.active_user is not None else None,
        metadata=dict(description="The timestamp when the run started"),
    )

    @classmethod
    def default(cls):
        return cls(
            dbt_schema_version=str(WritableManifest.dbt_schema_version),
        )


@dataclass
@schema_version("manifest", 12)
class WritableManifest(ArtifactMixin):
    nodes: Mapping[UniqueID, ManifestResource] = field(
        metadata=dict(description=("The nodes defined in the dbt project and its dependencies"))
    )
    sources: Mapping[UniqueID, SourceDefinition] = field(
        metadata=dict(description=("The sources defined in the dbt project and its dependencies"))
    )
    macros: Mapping[UniqueID, Macro] = field(
        metadata=dict(description=("The macros defined in the dbt project and its dependencies"))
    )
    docs: Mapping[UniqueID, Documentation] = field(
        metadata=dict(description=("The docs defined in the dbt project and its dependencies"))
    )
    exposures: Mapping[UniqueID, Exposure] = field(
        metadata=dict(
            description=("The exposures defined in the dbt project and its dependencies")
        )
    )
    metrics: Mapping[UniqueID, Metric] = field(
        metadata=dict(description=("The metrics defined in the dbt project and its dependencies"))
    )
    groups: Mapping[UniqueID, Group] = field(
        metadata=dict(description=("The groups defined in the dbt project"))
    )
    selectors: Mapping[UniqueID, Any] = field(
        metadata=dict(description=("The selectors defined in selectors.yml"))
    )
    disabled: Optional[Mapping[UniqueID, List[DisabledManifestResource]]] = field(
        metadata=dict(description="A mapping of the disabled nodes in the target")
    )
    parent_map: Optional[NodeEdgeMap] = field(
        metadata=dict(
            description="A mapping from child nodes to their dependencies",
        )
    )
    child_map: Optional[NodeEdgeMap] = field(
        metadata=dict(
            description="A mapping from parent nodes to their dependents",
        )
    )
    group_map: Optional[NodeEdgeMap] = field(
        metadata=dict(
            description="A mapping from group names to their nodes",
        )
    )
    saved_queries: Mapping[UniqueID, SavedQuery] = field(
        metadata=dict(description=("The saved queries defined in the dbt project"))
    )
    semantic_models: Mapping[UniqueID, SemanticModel] = field(
        metadata=dict(description=("The semantic models defined in the dbt project"))
    )
    metadata: ManifestMetadata = field(
        metadata=dict(
            description="Metadata about the manifest",
        )
    )
    unit_tests: Mapping[UniqueID, UnitTestDefinition] = field(
        metadata=dict(
            description="The unit tests defined in the project",
        )
    )
    functions: Mapping[UniqueID, Function] = field(
        default_factory=dict,
        metadata=dict(description=("The functions defined in the dbt project")),
    )

    @classmethod
    def compatible_previous_versions(cls) -> Iterable[Tuple[str, int]]:
        return [
            ("manifest", 4),
            ("manifest", 5),
            ("manifest", 6),
            ("manifest", 7),
            ("manifest", 8),
            ("manifest", 9),
            ("manifest", 10),
            ("manifest", 11),
        ]

    @classmethod
    def upgrade_schema_version(cls, data):
        """This overrides the "upgrade_schema_version" call in VersionedSchema (via
        ArtifactMixin) to modify the dictionary passed in from earlier versions of the manifest."""
        manifest_schema_version = get_artifact_schema_version(data)
        data = drop_unknown_resource_types(data)
        if manifest_schema_version < cls.dbt_schema_version.version:
            data = upgrade_manifest_json(data, manifest_schema_version)

        manifest_dbt_version = get_artifact_dbt_version(data)
        if manifest_dbt_version and manifest_dbt_version != __version__:
            data = upgrade_manifest_json_dbt_version(data)
        return cls.from_dict(data)

    @classmethod
    def validate(cls, _):
        # When dbt try to load an artifact with additional optional fields
        # that are not present in the schema, from_dict will work fine.
        # As long as validate is not called, the schema will not be enforced.
        # This is intentional, as it allows for safer schema upgrades.
        raise DbtInternalError(
            "The WritableManifest should never be validated directly to allow for schema upgrades."
        )
