import enum
from collections import defaultdict
from dataclasses import dataclass, field
from itertools import chain, islice
from mashumaro.mixins.msgpack import DataClassMessagePackMixin
from multiprocessing.synchronize import Lock
from typing import (
    DefaultDict,
    Dict,
    List,
    Optional,
    Union,
    Mapping,
    MutableMapping,
    Any,
    Set,
    Tuple,
    TypeVar,
    Callable,
    Generic,
    AbstractSet,
    ClassVar,
    Iterable,
)
from typing_extensions import Protocol
from uuid import UUID

from dbt.contracts.graph.nodes import (
    BaseNode,
    Documentation,
    Exposure,
    GenericTestNode,
    GraphMemberNode,
    Group,
    Macro,
    ManifestNode,
    Metric,
    ModelNode,
    DeferRelation,
    ResultNode,
    SavedQuery,
    SemanticModel,
    SourceDefinition,
    UnpatchedSourceDefinition,
)
from dbt.contracts.graph.unparsed import SourcePatch, NodeVersion, UnparsedVersion
from dbt.contracts.graph.manifest_upgrade import upgrade_manifest_json
from dbt.contracts.files import SourceFile, SchemaSourceFile, FileHash, AnySourceFile
from dbt.contracts.util import (
    BaseArtifactMetadata,
    SourceKey,
    ArtifactMixin,
    schema_version,
    get_artifact_schema_version,
)
from dbt.common.dataclass_schema import dbtClassMixin

from dbt.exceptions import (
    CompilationError,
    DuplicateResourceNameError,
    AmbiguousResourceNameRefError,
)
from dbt.adapters.exceptions import DuplicateMacroInPackageError, DuplicateMaterializationNameError
from dbt.common.helper_types import PathSet
from dbt.common.events.functions import fire_event
from dbt.common.events.types import MergedFromState, UnpinnedRefNewVersionAvailable
from dbt.common.events.contextvars import get_node_info
from dbt.node_types import NodeType, AccessType
from dbt.flags import get_flags
from dbt.mp_context import get_mp_context
from dbt import tracking
import dbt.common.utils


NodeEdgeMap = Dict[str, List[str]]
PackageName = str
DocName = str
RefName = str
UniqueID = str


def find_unique_id_for_package(storage, key, package: Optional[PackageName]):
    if key not in storage:
        return None

    pkg_dct: Mapping[PackageName, UniqueID] = storage[key]

    if package is None:
        if not pkg_dct:
            return None
        else:
            return next(iter(pkg_dct.values()))
    elif package in pkg_dct:
        return pkg_dct[package]
    else:
        return None


class DocLookup(dbtClassMixin):
    def __init__(self, manifest: "Manifest") -> None:
        self.storage: Dict[str, Dict[PackageName, UniqueID]] = {}
        self.populate(manifest)

    def get_unique_id(self, key, package: Optional[PackageName]):
        return find_unique_id_for_package(self.storage, key, package)

    def find(self, key, package: Optional[PackageName], manifest: "Manifest"):
        unique_id = self.get_unique_id(key, package)
        if unique_id is not None:
            return self.perform_lookup(unique_id, manifest)
        return None

    def add_doc(self, doc: Documentation):
        if doc.name not in self.storage:
            self.storage[doc.name] = {}
        self.storage[doc.name][doc.package_name] = doc.unique_id

    def populate(self, manifest):
        for doc in manifest.docs.values():
            self.add_doc(doc)

    def perform_lookup(self, unique_id: UniqueID, manifest) -> Documentation:
        if unique_id not in manifest.docs:
            raise dbt.exceptions.DbtInternalError(
                f"Doc {unique_id} found in cache but not found in manifest"
            )
        return manifest.docs[unique_id]


class SourceLookup(dbtClassMixin):
    def __init__(self, manifest: "Manifest") -> None:
        self.storage: Dict[str, Dict[PackageName, UniqueID]] = {}
        self.populate(manifest)

    def get_unique_id(self, search_name, package: Optional[PackageName]):
        return find_unique_id_for_package(self.storage, search_name, package)

    def find(self, search_name, package: Optional[PackageName], manifest: "Manifest"):
        unique_id = self.get_unique_id(search_name, package)
        if unique_id is not None:
            return self.perform_lookup(unique_id, manifest)
        return None

    def add_source(self, source: SourceDefinition):
        if source.search_name not in self.storage:
            self.storage[source.search_name] = {}

        self.storage[source.search_name][source.package_name] = source.unique_id

    def populate(self, manifest):
        for source in manifest.sources.values():
            if hasattr(source, "source_name"):
                self.add_source(source)

    def perform_lookup(self, unique_id: UniqueID, manifest: "Manifest") -> SourceDefinition:
        if unique_id not in manifest.sources:
            raise dbt.exceptions.DbtInternalError(
                f"Source {unique_id} found in cache but not found in manifest"
            )
        return manifest.sources[unique_id]


class RefableLookup(dbtClassMixin):
    # model, seed, snapshot
    _lookup_types: ClassVar[set] = set(NodeType.refable())
    _versioned_types: ClassVar[set] = set(NodeType.versioned())

    def __init__(self, manifest: "Manifest") -> None:
        self.storage: Dict[str, Dict[PackageName, UniqueID]] = {}
        self.populate(manifest)

    def get_unique_id(
        self,
        key: str,
        package: Optional[PackageName],
        version: Optional[NodeVersion],
        node: Optional[GraphMemberNode] = None,
    ):
        if version:
            key = f"{key}.v{version}"

        unique_ids = self._find_unique_ids_for_package(key, package)
        if len(unique_ids) > 1:
            raise AmbiguousResourceNameRefError(key, unique_ids, node)
        else:
            return unique_ids[0] if unique_ids else None

    def find(
        self,
        key: str,
        package: Optional[PackageName],
        version: Optional[NodeVersion],
        manifest: "Manifest",
        source_node: Optional[GraphMemberNode] = None,
    ):
        unique_id = self.get_unique_id(key, package, version, source_node)
        if unique_id is not None:
            node = self.perform_lookup(unique_id, manifest)
            # If this is an unpinned ref (no 'version' arg was passed),
            # AND this is a versioned node,
            # AND this ref is being resolved at runtime -- get_node_info != {}
            # Only ModelNodes can be versioned.
            if (
                isinstance(node, ModelNode)
                and version is None
                and node.is_versioned
                and get_node_info()
            ):
                # Check to see if newer versions are available, and log an "FYI" if so
                max_version: UnparsedVersion = max(
                    [
                        UnparsedVersion(v.version)
                        for v in manifest.nodes.values()
                        if isinstance(v, ModelNode)
                        and v.name == node.name
                        and v.version is not None
                    ]
                )
                assert node.latest_version is not None  # for mypy, whenever i may find it
                if max_version > UnparsedVersion(node.latest_version):
                    fire_event(
                        UnpinnedRefNewVersionAvailable(
                            node_info=get_node_info(),
                            ref_node_name=node.name,
                            ref_node_package=node.package_name,
                            ref_node_version=str(node.version),
                            ref_max_version=str(max_version.v),
                        )
                    )

            return node
        return None

    def add_node(self, node: ManifestNode):
        if node.resource_type in self._lookup_types:
            if node.name not in self.storage:
                self.storage[node.name] = {}

            if node.is_versioned:
                if node.search_name not in self.storage:
                    self.storage[node.search_name] = {}
                self.storage[node.search_name][node.package_name] = node.unique_id
                if node.is_latest_version:  # type: ignore
                    self.storage[node.name][node.package_name] = node.unique_id
            else:
                self.storage[node.name][node.package_name] = node.unique_id

    def populate(self, manifest):
        for node in manifest.nodes.values():
            self.add_node(node)

    def perform_lookup(self, unique_id: UniqueID, manifest) -> ManifestNode:
        if unique_id in manifest.nodes:
            node = manifest.nodes[unique_id]
        else:
            raise dbt.exceptions.DbtInternalError(
                f"Node {unique_id} found in cache but not found in manifest"
            )
        return node

    def _find_unique_ids_for_package(self, key, package: Optional[PackageName]) -> List[str]:
        if key not in self.storage:
            return []

        pkg_dct: Mapping[PackageName, UniqueID] = self.storage[key]

        if package is None:
            if not pkg_dct:
                return []
            else:
                return list(pkg_dct.values())
        elif package in pkg_dct:
            return [pkg_dct[package]]
        else:
            return []


class MetricLookup(dbtClassMixin):
    def __init__(self, manifest: "Manifest") -> None:
        self.storage: Dict[str, Dict[PackageName, UniqueID]] = {}
        self.populate(manifest)

    def get_unique_id(self, search_name, package: Optional[PackageName]):
        return find_unique_id_for_package(self.storage, search_name, package)

    def find(self, search_name, package: Optional[PackageName], manifest: "Manifest"):
        unique_id = self.get_unique_id(search_name, package)
        if unique_id is not None:
            return self.perform_lookup(unique_id, manifest)
        return None

    def add_metric(self, metric: Metric):
        if metric.search_name not in self.storage:
            self.storage[metric.search_name] = {}

        self.storage[metric.search_name][metric.package_name] = metric.unique_id

    def populate(self, manifest):
        for metric in manifest.metrics.values():
            if hasattr(metric, "name"):
                self.add_metric(metric)

    def perform_lookup(self, unique_id: UniqueID, manifest: "Manifest") -> Metric:
        if unique_id not in manifest.metrics:
            raise dbt.exceptions.DbtInternalError(
                f"Metric {unique_id} found in cache but not found in manifest"
            )
        return manifest.metrics[unique_id]


class SavedQueryLookup(dbtClassMixin):
    """Lookup utility for finding SavedQuery nodes"""

    def __init__(self, manifest: "Manifest") -> None:
        self.storage: Dict[str, Dict[PackageName, UniqueID]] = {}
        self.populate(manifest)

    def get_unique_id(self, search_name, package: Optional[PackageName]):
        return find_unique_id_for_package(self.storage, search_name, package)

    def find(self, search_name, package: Optional[PackageName], manifest: "Manifest"):
        unique_id = self.get_unique_id(search_name, package)
        if unique_id is not None:
            return self.perform_lookup(unique_id, manifest)
        return None

    def add_saved_query(self, saved_query: SavedQuery):
        if saved_query.search_name not in self.storage:
            self.storage[saved_query.search_name] = {}

        self.storage[saved_query.search_name][saved_query.package_name] = saved_query.unique_id

    def populate(self, manifest):
        for saved_query in manifest.saved_queries.values():
            if hasattr(saved_query, "name"):
                self.add_saved_query(saved_query)

    def perform_lookup(self, unique_id: UniqueID, manifest: "Manifest") -> SavedQuery:
        if unique_id not in manifest.saved_queries:
            raise dbt.exceptions.DbtInternalError(
                f"SavedQUery {unique_id} found in cache but not found in manifest"
            )
        return manifest.saved_queries[unique_id]


class SemanticModelByMeasureLookup(dbtClassMixin):
    """Lookup utility for finding SemanticModel by measure

    This is possible because measure names are supposed to be unique across
    the semantic models in a manifest.
    """

    def __init__(self, manifest: "Manifest") -> None:
        self.storage: DefaultDict[str, Dict[PackageName, UniqueID]] = defaultdict(dict)
        self.populate(manifest)

    def get_unique_id(self, search_name: str, package: Optional[PackageName]):
        return find_unique_id_for_package(self.storage, search_name, package)

    def find(
        self, search_name: str, package: Optional[PackageName], manifest: "Manifest"
    ) -> Optional[SemanticModel]:
        """Tries to find a SemanticModel based on a measure name"""
        unique_id = self.get_unique_id(search_name, package)
        if unique_id is not None:
            return self.perform_lookup(unique_id, manifest)
        return None

    def add(self, semantic_model: SemanticModel):
        """Sets all measures for a SemanticModel as paths to the SemanticModel's `unique_id`"""
        for measure in semantic_model.measures:
            self.storage[measure.name][semantic_model.package_name] = semantic_model.unique_id

    def populate(self, manifest: "Manifest"):
        """Populate storage with all the measure + package paths to the Manifest's SemanticModels"""
        for semantic_model in manifest.semantic_models.values():
            self.add(semantic_model=semantic_model)
        for disabled in manifest.disabled.values():
            for node in disabled:
                if isinstance(node, SemanticModel):
                    self.add(semantic_model=node)

    def perform_lookup(self, unique_id: UniqueID, manifest: "Manifest") -> SemanticModel:
        """Tries to get a SemanticModel from the Manifest"""
        enabled_semantic_model: Optional[SemanticModel] = manifest.semantic_models.get(unique_id)
        disabled_semantic_model: Optional[List] = manifest.disabled.get(unique_id)

        if isinstance(enabled_semantic_model, SemanticModel):
            return enabled_semantic_model
        elif disabled_semantic_model is not None and isinstance(
            disabled_semantic_model[0], SemanticModel
        ):
            return disabled_semantic_model[0]
        else:
            raise dbt.exceptions.DbtInternalError(
                f"Semantic model `{unique_id}` found in cache but not found in manifest"
            )


# This handles both models/seeds/snapshots and sources/metrics/exposures/semantic_models
class DisabledLookup(dbtClassMixin):
    def __init__(self, manifest: "Manifest") -> None:
        self.storage: Dict[str, Dict[PackageName, List[Any]]] = {}
        self.populate(manifest)

    def populate(self, manifest):
        for node in list(chain.from_iterable(manifest.disabled.values())):
            self.add_node(node)

    def add_node(self, node):
        if node.search_name not in self.storage:
            self.storage[node.search_name] = {}
        if node.package_name not in self.storage[node.search_name]:
            self.storage[node.search_name][node.package_name] = []
        self.storage[node.search_name][node.package_name].append(node)

    # This should return a list of disabled nodes. It's different from
    # the other Lookup functions in that it returns full nodes, not just unique_ids
    def find(
        self, search_name, package: Optional[PackageName], version: Optional[NodeVersion] = None
    ):
        if version:
            search_name = f"{search_name}.v{version}"

        if search_name not in self.storage:
            return None

        pkg_dct: Mapping[PackageName, List[Any]] = self.storage[search_name]

        if package is None:
            if not pkg_dct:
                return None
            else:
                return next(iter(pkg_dct.values()))
        elif package in pkg_dct:
            return pkg_dct[package]
        else:
            return None


class AnalysisLookup(RefableLookup):
    _lookup_types: ClassVar[set] = set([NodeType.Analysis])
    _versioned_types: ClassVar[set] = set()


def _packages_to_search(
    current_project: str,
    node_package: str,
    target_package: Optional[str] = None,
) -> List[Optional[str]]:
    if target_package is not None:
        return [target_package]
    elif current_project == node_package:
        return [current_project, None]
    else:
        return [current_project, node_package, None]


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

    def __post_init__(self):
        if tracking.active_user is None:
            return

        if self.user_id is None:
            self.user_id = tracking.active_user.id

        if self.send_anonymous_usage_stats is None:
            self.send_anonymous_usage_stats = get_flags().SEND_ANONYMOUS_USAGE_STATS

    @classmethod
    def default(cls):
        return cls(
            dbt_schema_version=str(WritableManifest.dbt_schema_version),
        )


def _sort_values(dct):
    """Given a dictionary, sort each value. This makes output deterministic,
    which helps for tests.
    """
    return {k: sorted(v) for k, v in dct.items()}


def build_node_edges(nodes: List[ManifestNode]):
    """Build the forward and backward edges on the given list of ManifestNodes
    and return them as two separate dictionaries, each mapping unique IDs to
    lists of edges.
    """
    backward_edges: Dict[str, List[str]] = {}
    # pre-populate the forward edge dict for simplicity
    forward_edges: Dict[str, List[str]] = {n.unique_id: [] for n in nodes}
    for node in nodes:
        backward_edges[node.unique_id] = node.depends_on_nodes[:]
        for unique_id in backward_edges[node.unique_id]:
            if unique_id in forward_edges.keys():
                forward_edges[unique_id].append(node.unique_id)
    return _sort_values(forward_edges), _sort_values(backward_edges)


# Build a map of children of macros and generic tests
def build_macro_edges(nodes: List[Any]):
    forward_edges: Dict[str, List[str]] = {
        n.unique_id: [] for n in nodes if n.unique_id.startswith("macro") or n.depends_on_macros
    }
    for node in nodes:
        for unique_id in node.depends_on_macros:
            if unique_id in forward_edges.keys():
                forward_edges[unique_id].append(node.unique_id)
    return _sort_values(forward_edges)


def _deepcopy(value):
    return value.from_dict(value.to_dict(omit_none=True))


class Locality(enum.IntEnum):
    Core = 1
    Imported = 2
    Root = 3


@dataclass
class MacroCandidate:
    locality: Locality
    macro: Macro

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, MacroCandidate):
            return NotImplemented
        return self.locality == other.locality

    def __lt__(self, other: object) -> bool:
        if not isinstance(other, MacroCandidate):
            return NotImplemented
        if self.locality < other.locality:
            return True
        if self.locality > other.locality:
            return False
        return False


@dataclass
class MaterializationCandidate(MacroCandidate):
    # specificity describes where in the inheritance chain this materialization candidate is
    # a specificity of 0 means a materialization defined by the current adapter
    # the highest the specificity describes a default materialization. the value itself depends on
    # how many adapters there are in the inheritance chain
    specificity: int

    @classmethod
    def from_macro(cls, candidate: MacroCandidate, specificity: int) -> "MaterializationCandidate":
        return cls(
            locality=candidate.locality,
            macro=candidate.macro,
            specificity=specificity,
        )

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, MaterializationCandidate):
            return NotImplemented
        equal = self.specificity == other.specificity and self.locality == other.locality
        if equal:
            raise DuplicateMaterializationNameError(self.macro, other)

        return equal

    def __lt__(self, other: object) -> bool:
        if not isinstance(other, MaterializationCandidate):
            return NotImplemented
        if self.specificity > other.specificity:
            return True
        if self.specificity < other.specificity:
            return False
        if self.locality < other.locality:
            return True
        if self.locality > other.locality:
            return False
        return False


M = TypeVar("M", bound=MacroCandidate)


class CandidateList(List[M]):
    def last(self) -> Optional[Macro]:
        if not self:
            return None
        self.sort()
        return self[-1].macro


def _get_locality(macro: Macro, root_project_name: str, internal_packages: Set[str]) -> Locality:
    if macro.package_name == root_project_name:
        return Locality.Root
    elif macro.package_name in internal_packages:
        return Locality.Core
    else:
        return Locality.Imported


class Searchable(Protocol):
    resource_type: NodeType
    package_name: str

    @property
    def search_name(self) -> str:
        raise NotImplementedError("search_name not implemented")


D = TypeVar("D")


@dataclass
class Disabled(Generic[D]):
    target: D


MaybeMetricNode = Optional[Union[Metric, Disabled[Metric]]]


MaybeSavedQueryNode = Optional[Union[SavedQuery, Disabled[SavedQuery]]]


MaybeDocumentation = Optional[Documentation]


MaybeParsedSource = Optional[
    Union[
        SourceDefinition,
        Disabled[SourceDefinition],
    ]
]


MaybeNonSource = Optional[Union[ManifestNode, Disabled[ManifestNode]]]


T = TypeVar("T", bound=GraphMemberNode)


# This contains macro methods that are in both the Manifest
# and the MacroManifest
class MacroMethods:
    # Just to make mypy happy. There must be a better way.
    def __init__(self):
        self.macros = []
        self.metadata = {}

    def find_macro_by_name(
        self, name: str, root_project_name: str, package: Optional[str]
    ) -> Optional[Macro]:
        """Find a macro in the graph by its name and package name, or None for
        any package. The root project name is used to determine priority:
         - locally defined macros come first
         - then imported macros
         - then macros defined in the root project
        """
        filter: Optional[Callable[[MacroCandidate], bool]] = None
        if package is not None:

            def filter(candidate: MacroCandidate) -> bool:
                return package == candidate.macro.package_name

        candidates: CandidateList = self._find_macros_by_name(
            name=name,
            root_project_name=root_project_name,
            filter=filter,
        )

        return candidates.last()

    def find_generate_macro_by_name(
        self, component: str, root_project_name: str, imported_package: Optional[str] = None
    ) -> Optional[Macro]:
        """
        The default `generate_X_name` macros are similar to regular ones, but only
        includes imported packages when searching for a package.
        - if package is not provided:
            - if there is a `generate_{component}_name` macro in the root
              project, return it
            - return the `generate_{component}_name` macro from the 'dbt'
              internal project
        - if package is provided
            - return the `generate_{component}_name` macro from the imported
              package, if one exists
        """

        def filter(candidate: MacroCandidate) -> bool:
            if imported_package:
                return (
                    candidate.locality == Locality.Imported
                    and imported_package == candidate.macro.package_name
                )
            else:
                return candidate.locality != Locality.Imported

        candidates: CandidateList = self._find_macros_by_name(
            name=f"generate_{component}_name",
            root_project_name=root_project_name,
            filter=filter,
        )

        return candidates.last()

    def _find_macros_by_name(
        self,
        name: str,
        root_project_name: str,
        filter: Optional[Callable[[MacroCandidate], bool]] = None,
    ) -> CandidateList:
        """Find macros by their name."""
        # avoid an import cycle
        from dbt.clients.adapter import get_adapter_package_names

        candidates: CandidateList = CandidateList()
        packages = set(get_adapter_package_names(self.metadata.adapter_type))
        for unique_id, macro in self.macros.items():
            if macro.name != name:
                continue
            candidate = MacroCandidate(
                locality=_get_locality(macro, root_project_name, packages),
                macro=macro,
            )
            if filter is None or filter(candidate):
                candidates.append(candidate)

        return candidates


@dataclass
class ParsingInfo:
    static_analysis_parsed_path_count: int = 0
    static_analysis_path_count: int = 0


@dataclass
class ManifestStateCheck(dbtClassMixin):
    vars_hash: FileHash = field(default_factory=FileHash.empty)
    project_env_vars_hash: FileHash = field(default_factory=FileHash.empty)
    profile_env_vars_hash: FileHash = field(default_factory=FileHash.empty)
    profile_hash: FileHash = field(default_factory=FileHash.empty)
    project_hashes: MutableMapping[str, FileHash] = field(default_factory=dict)


@dataclass
class Manifest(MacroMethods, DataClassMessagePackMixin, dbtClassMixin):
    """The manifest for the full graph, after parsing and during compilation."""

    # These attributes are both positional and by keyword. If an attribute
    # is added it must all be added in the __reduce_ex__ method in the
    # args tuple in the right position.
    nodes: MutableMapping[str, ManifestNode] = field(default_factory=dict)
    sources: MutableMapping[str, SourceDefinition] = field(default_factory=dict)
    macros: MutableMapping[str, Macro] = field(default_factory=dict)
    docs: MutableMapping[str, Documentation] = field(default_factory=dict)
    exposures: MutableMapping[str, Exposure] = field(default_factory=dict)
    metrics: MutableMapping[str, Metric] = field(default_factory=dict)
    groups: MutableMapping[str, Group] = field(default_factory=dict)
    selectors: MutableMapping[str, Any] = field(default_factory=dict)
    files: MutableMapping[str, AnySourceFile] = field(default_factory=dict)
    metadata: ManifestMetadata = field(default_factory=ManifestMetadata)
    flat_graph: Dict[str, Any] = field(default_factory=dict)
    state_check: ManifestStateCheck = field(default_factory=ManifestStateCheck)
    source_patches: MutableMapping[SourceKey, SourcePatch] = field(default_factory=dict)
    disabled: MutableMapping[str, List[GraphMemberNode]] = field(default_factory=dict)
    env_vars: MutableMapping[str, str] = field(default_factory=dict)
    semantic_models: MutableMapping[str, SemanticModel] = field(default_factory=dict)
    saved_queries: MutableMapping[str, SavedQuery] = field(default_factory=dict)

    _doc_lookup: Optional[DocLookup] = field(
        default=None, metadata={"serialize": lambda x: None, "deserialize": lambda x: None}
    )
    _source_lookup: Optional[SourceLookup] = field(
        default=None, metadata={"serialize": lambda x: None, "deserialize": lambda x: None}
    )
    _ref_lookup: Optional[RefableLookup] = field(
        default=None, metadata={"serialize": lambda x: None, "deserialize": lambda x: None}
    )
    _metric_lookup: Optional[MetricLookup] = field(
        default=None, metadata={"serialize": lambda x: None, "deserialize": lambda x: None}
    )
    _saved_query_lookup: Optional[SavedQueryLookup] = field(
        default=None, metadata={"serialize": lambda x: None, "deserialize": lambda x: None}
    )
    _semantic_model_by_measure_lookup: Optional[SemanticModelByMeasureLookup] = field(
        default=None, metadata={"serialize": lambda x: None, "deserialize": lambda x: None}
    )
    _disabled_lookup: Optional[DisabledLookup] = field(
        default=None, metadata={"serialize": lambda x: None, "deserialize": lambda x: None}
    )
    _analysis_lookup: Optional[AnalysisLookup] = field(
        default=None, metadata={"serialize": lambda x: None, "deserialize": lambda x: None}
    )
    _parsing_info: ParsingInfo = field(
        default_factory=ParsingInfo,
        metadata={"serialize": lambda x: None, "deserialize": lambda x: None},
    )
    _lock: Lock = field(
        default_factory=get_mp_context().Lock,
        metadata={"serialize": lambda x: None, "deserialize": lambda x: None},
    )

    def __pre_serialize__(self):
        # serialization won't work with anything except an empty source_patches because
        # tuple keys are not supported, so ensure it's empty
        self.source_patches = {}
        return self

    @classmethod
    def __post_deserialize__(cls, obj):
        obj._lock = get_mp_context().Lock()
        return obj

    def build_flat_graph(self):
        """This attribute is used in context.common by each node, so we want to
        only build it once and avoid any concurrency issues around it.
        Make sure you don't call this until you're done with building your
        manifest!
        """
        self.flat_graph = {
            "exposures": {k: v.to_dict(omit_none=False) for k, v in self.exposures.items()},
            "groups": {k: v.to_dict(omit_none=False) for k, v in self.groups.items()},
            "metrics": {k: v.to_dict(omit_none=False) for k, v in self.metrics.items()},
            "nodes": {k: v.to_dict(omit_none=False) for k, v in self.nodes.items()},
            "sources": {k: v.to_dict(omit_none=False) for k, v in self.sources.items()},
            "semantic_models": {
                k: v.to_dict(omit_none=False) for k, v in self.semantic_models.items()
            },
            "saved_queries": {
                k: v.to_dict(omit_none=False) for k, v in self.saved_queries.items()
            },
        }

    def build_disabled_by_file_id(self):
        disabled_by_file_id = {}
        for node_list in self.disabled.values():
            for node in node_list:
                disabled_by_file_id[node.file_id] = node
        return disabled_by_file_id

    def _get_parent_adapter_types(self, adapter_type: str) -> List[str]:
        # This is duplicated logic from core/dbt/context/providers.py
        # Ideally this would instead be incorporating actual dispatch logic
        from dbt.clients.adapter import get_adapter_type_names

        # order matters for dispatch:
        #  1. current adapter
        #  2. any parent adapters (dependencies)
        #  3. 'default'
        return get_adapter_type_names(adapter_type) + ["default"]

    def _materialization_candidates_for(
        self,
        project_name: str,
        materialization_name: str,
        adapter_type: str,
        specificity: int,
    ) -> CandidateList:
        full_name = dbt.common.utils.get_materialization_macro_name(
            materialization_name=materialization_name,
            adapter_type=adapter_type,
            with_prefix=False,
        )
        return CandidateList(
            MaterializationCandidate.from_macro(m, specificity)
            for m in self._find_macros_by_name(full_name, project_name)
        )

    def find_materialization_macro_by_name(
        self, project_name: str, materialization_name: str, adapter_type: str
    ) -> Optional[Macro]:
        candidates: CandidateList = CandidateList(
            chain.from_iterable(
                self._materialization_candidates_for(
                    project_name=project_name,
                    materialization_name=materialization_name,
                    adapter_type=atype,
                    specificity=specificity,  # where in the inheritance chain this candidate is
                )
                for specificity, atype in enumerate(self._get_parent_adapter_types(adapter_type))
            )
        )
        return candidates.last()

    def get_resource_fqns(self) -> Mapping[str, PathSet]:
        resource_fqns: Dict[str, Set[Tuple[str, ...]]] = {}
        all_resources = chain(
            self.exposures.values(),
            self.nodes.values(),
            self.sources.values(),
            self.metrics.values(),
            self.semantic_models.values(),
            self.saved_queries.values(),
        )
        for resource in all_resources:
            resource_type_plural = resource.resource_type.pluralize()
            if resource_type_plural not in resource_fqns:
                resource_fqns[resource_type_plural] = set()
            resource_fqns[resource_type_plural].add(tuple(resource.fqn))
        return resource_fqns

    def get_used_schemas(self, resource_types=None):
        return frozenset(
            {
                (node.database, node.schema)
                for node in chain(self.nodes.values(), self.sources.values())
                if not resource_types or node.resource_type in resource_types
            }
        )

    def get_used_databases(self):
        return frozenset(x.database for x in chain(self.nodes.values(), self.sources.values()))

    def deepcopy(self):
        copy = Manifest(
            nodes={k: _deepcopy(v) for k, v in self.nodes.items()},
            sources={k: _deepcopy(v) for k, v in self.sources.items()},
            macros={k: _deepcopy(v) for k, v in self.macros.items()},
            docs={k: _deepcopy(v) for k, v in self.docs.items()},
            exposures={k: _deepcopy(v) for k, v in self.exposures.items()},
            metrics={k: _deepcopy(v) for k, v in self.metrics.items()},
            groups={k: _deepcopy(v) for k, v in self.groups.items()},
            selectors={k: _deepcopy(v) for k, v in self.selectors.items()},
            metadata=self.metadata,
            disabled={k: _deepcopy(v) for k, v in self.disabled.items()},
            files={k: _deepcopy(v) for k, v in self.files.items()},
            state_check=_deepcopy(self.state_check),
            semantic_models={k: _deepcopy(v) for k, v in self.semantic_models.items()},
            saved_queries={k: _deepcopy(v) for k, v in self.saved_queries.items()},
        )
        copy.build_flat_graph()
        return copy

    def build_parent_and_child_maps(self):
        edge_members = list(
            chain(
                self.nodes.values(),
                self.sources.values(),
                self.exposures.values(),
                self.metrics.values(),
                self.semantic_models.values(),
                self.saved_queries.values(),
            )
        )
        forward_edges, backward_edges = build_node_edges(edge_members)
        self.child_map = forward_edges
        self.parent_map = backward_edges

    def build_macro_child_map(self):
        edge_members = list(
            chain(
                self.nodes.values(),
                self.macros.values(),
            )
        )
        forward_edges = build_macro_edges(edge_members)
        return forward_edges

    def build_group_map(self):
        groupable_nodes = list(
            chain(
                self.nodes.values(),
                self.saved_queries.values(),
                self.semantic_models.values(),
                self.metrics.values(),
            )
        )
        group_map = {group.name: [] for group in self.groups.values()}
        for node in groupable_nodes:
            if node.group is not None:
                # group updates are not included with state:modified and
                # by ignoring the groups that aren't in the group map we
                # can avoid hitting errors for groups that are not getting
                # updated.  This is a hack but any groups that are not
                # valid will be caught in
                # parser.manifest.ManifestLoader.check_valid_group_config_node
                if node.group in group_map:
                    group_map[node.group].append(node.unique_id)
        self.group_map = group_map

    def writable_manifest(self) -> "WritableManifest":
        self.build_parent_and_child_maps()
        self.build_group_map()
        return WritableManifest(
            nodes=self.nodes,
            sources=self.sources,
            macros=self.macros,
            docs=self.docs,
            exposures=self.exposures,
            metrics=self.metrics,
            groups=self.groups,
            selectors=self.selectors,
            metadata=self.metadata,
            disabled=self.disabled,
            child_map=self.child_map,
            parent_map=self.parent_map,
            group_map=self.group_map,
            semantic_models=self.semantic_models,
            saved_queries=self.saved_queries,
        )

    def write(self, path):
        self.writable_manifest().write(path)

    # Called in dbt.compilation.Linker.write_graph and
    # dbt.graph.queue.get and ._include_in_cost
    def expect(self, unique_id: str) -> GraphMemberNode:
        if unique_id in self.nodes:
            return self.nodes[unique_id]
        elif unique_id in self.sources:
            return self.sources[unique_id]
        elif unique_id in self.exposures:
            return self.exposures[unique_id]
        elif unique_id in self.metrics:
            return self.metrics[unique_id]
        elif unique_id in self.semantic_models:
            return self.semantic_models[unique_id]
        elif unique_id in self.saved_queries:
            return self.saved_queries[unique_id]
        else:
            # something terrible has happened
            raise dbt.exceptions.DbtInternalError(
                "Expected node {} not found in manifest".format(unique_id)
            )

    @property
    def doc_lookup(self) -> DocLookup:
        if self._doc_lookup is None:
            self._doc_lookup = DocLookup(self)
        return self._doc_lookup

    def rebuild_doc_lookup(self):
        self._doc_lookup = DocLookup(self)

    @property
    def source_lookup(self) -> SourceLookup:
        if self._source_lookup is None:
            self._source_lookup = SourceLookup(self)
        return self._source_lookup

    def rebuild_source_lookup(self):
        self._source_lookup = SourceLookup(self)

    @property
    def ref_lookup(self) -> RefableLookup:
        if self._ref_lookup is None:
            self._ref_lookup = RefableLookup(self)
        return self._ref_lookup

    @property
    def metric_lookup(self) -> MetricLookup:
        if self._metric_lookup is None:
            self._metric_lookup = MetricLookup(self)
        return self._metric_lookup

    @property
    def saved_query_lookup(self) -> SavedQueryLookup:
        """Retuns a SavedQueryLookup, instantiating it first if necessary."""
        if self._saved_query_lookup is None:
            self._saved_query_lookup = SavedQueryLookup(self)
        return self._saved_query_lookup

    @property
    def semantic_model_by_measure_lookup(self) -> SemanticModelByMeasureLookup:
        """Gets (and creates if necessary) the lookup utility for getting SemanticModels by measures"""
        if self._semantic_model_by_measure_lookup is None:
            self._semantic_model_by_measure_lookup = SemanticModelByMeasureLookup(self)
        return self._semantic_model_by_measure_lookup

    def rebuild_ref_lookup(self):
        self._ref_lookup = RefableLookup(self)

    @property
    def disabled_lookup(self) -> DisabledLookup:
        if self._disabled_lookup is None:
            self._disabled_lookup = DisabledLookup(self)
        return self._disabled_lookup

    def rebuild_disabled_lookup(self):
        self._disabled_lookup = DisabledLookup(self)

    @property
    def analysis_lookup(self) -> AnalysisLookup:
        if self._analysis_lookup is None:
            self._analysis_lookup = AnalysisLookup(self)
        return self._analysis_lookup

    @property
    def external_node_unique_ids(self):
        return [node.unique_id for node in self.nodes.values() if node.is_external_node]

    def resolve_refs(
        self,
        source_node: ModelNode,
        current_project: str,  # TODO: ModelNode is overly restrictive typing
    ) -> List[MaybeNonSource]:
        resolved_refs: List[MaybeNonSource] = []
        for ref in source_node.refs:
            resolved = self.resolve_ref(
                source_node,
                ref.name,
                ref.package,
                ref.version,
                current_project,
                source_node.package_name,
            )
            resolved_refs.append(resolved)

        return resolved_refs

    # Called by dbt.parser.manifest._process_refs & ManifestLoader.check_for_model_deprecations
    def resolve_ref(
        self,
        source_node: GraphMemberNode,
        target_model_name: str,
        target_model_package: Optional[str],
        target_model_version: Optional[NodeVersion],
        current_project: str,
        node_package: str,
    ) -> MaybeNonSource:

        node: Optional[ManifestNode] = None
        disabled: Optional[List[ManifestNode]] = None

        candidates = _packages_to_search(current_project, node_package, target_model_package)
        for pkg in candidates:
            node = self.ref_lookup.find(
                target_model_name, pkg, target_model_version, self, source_node
            )

            if node is not None and hasattr(node, "config") and node.config.enabled:
                return node

            # it's possible that the node is disabled
            if disabled is None:
                disabled = self.disabled_lookup.find(target_model_name, pkg, target_model_version)

        if disabled:
            return Disabled(disabled[0])
        return None

    # Called by dbt.parser.manifest._resolve_sources_for_exposure
    # and dbt.parser.manifest._process_source_for_node
    def resolve_source(
        self,
        target_source_name: str,
        target_table_name: str,
        current_project: str,
        node_package: str,
    ) -> MaybeParsedSource:
        search_name = f"{target_source_name}.{target_table_name}"
        candidates = _packages_to_search(current_project, node_package)

        source: Optional[SourceDefinition] = None
        disabled: Optional[List[SourceDefinition]] = None

        for pkg in candidates:
            source = self.source_lookup.find(search_name, pkg, self)
            if source is not None and source.config.enabled:
                return source

            if disabled is None:
                disabled = self.disabled_lookup.find(
                    f"{target_source_name}.{target_table_name}", pkg
                )

        if disabled:
            return Disabled(disabled[0])
        return None

    def resolve_metric(
        self,
        target_metric_name: str,
        target_metric_package: Optional[str],
        current_project: str,
        node_package: str,
    ) -> MaybeMetricNode:

        metric: Optional[Metric] = None
        disabled: Optional[List[Metric]] = None

        candidates = _packages_to_search(current_project, node_package, target_metric_package)
        for pkg in candidates:
            metric = self.metric_lookup.find(target_metric_name, pkg, self)

            if metric is not None and metric.config.enabled:
                return metric

            # it's possible that the node is disabled
            if disabled is None:
                disabled = self.disabled_lookup.find(f"{target_metric_name}", pkg)
        if disabled:
            return Disabled(disabled[0])
        return None

    def resolve_saved_query(
        self,
        target_saved_query_name: str,
        target_saved_query_package: Optional[str],
        current_project: str,
        node_package: str,
    ) -> MaybeSavedQueryNode:
        """Tries to find the SavedQuery by name within the available project and packages.

        Will return the first enabled SavedQuery matching the name found while iterating over
        the scoped packages. If no enabled SavedQuery node match is found, returns the last
        disabled SavedQuery node. Otherwise it returns None.
        """
        disabled: Optional[List[SavedQuery]] = None
        candidates = _packages_to_search(current_project, node_package, target_saved_query_package)
        for pkg in candidates:
            saved_query = self.saved_query_lookup.find(target_saved_query_name, pkg, self)

            if saved_query is not None and saved_query.config.enabled:
                return saved_query

            # it's possible that the node is disabled
            if disabled is None:
                disabled = self.disabled_lookup.find(f"{target_saved_query_name}", pkg)
        if disabled:
            return Disabled(disabled[0])

        return None

    def resolve_semantic_model_for_measure(
        self,
        target_measure_name: str,
        current_project: str,
        node_package: str,
        target_package: Optional[str] = None,
    ) -> Optional[SemanticModel]:
        """Tries to find the SemanticModel that a measure belongs to"""
        candidates = _packages_to_search(current_project, node_package, target_package)

        for pkg in candidates:
            semantic_model = self.semantic_model_by_measure_lookup.find(
                target_measure_name, pkg, self
            )
            # need to return it even if it's disabled so know it's not fully missing
            if semantic_model is not None:
                return semantic_model

        return None

    # Called by DocsRuntimeContext.doc
    def resolve_doc(
        self,
        name: str,
        package: Optional[str],
        current_project: str,
        node_package: str,
    ) -> Optional[Documentation]:
        """Resolve the given documentation. This follows the same algorithm as
        resolve_ref except the is_enabled checks are unnecessary as docs are
        always enabled.
        """
        candidates = _packages_to_search(current_project, node_package, package)

        for pkg in candidates:
            result = self.doc_lookup.find(name, pkg, self)
            if result is not None:
                return result
        return None

    def is_invalid_private_ref(
        self, node: GraphMemberNode, target_model: MaybeNonSource, dependencies: Optional[Mapping]
    ) -> bool:
        dependencies = dependencies or {}
        if not isinstance(target_model, ModelNode):
            return False

        is_private_ref = (
            target_model.access == AccessType.Private
            # don't raise this reference error for ad hoc 'preview' queries
            and node.resource_type != NodeType.SqlOperation
            and node.resource_type != NodeType.RPCCall  # TODO: rm
        )
        target_dependency = dependencies.get(target_model.package_name)
        restrict_package_access = target_dependency.restrict_access if target_dependency else False

        # TODO: SemanticModel and SourceDefinition do not have group, and so should not be able to make _any_ private ref.
        return is_private_ref and (
            not hasattr(node, "group")
            or not node.group
            or node.group != target_model.group
            or restrict_package_access
        )

    def is_invalid_protected_ref(
        self, node: GraphMemberNode, target_model: MaybeNonSource, dependencies: Optional[Mapping]
    ) -> bool:
        dependencies = dependencies or {}
        if not isinstance(target_model, ModelNode):
            return False

        is_protected_ref = (
            target_model.access == AccessType.Protected
            # don't raise this reference error for ad hoc 'preview' queries
            and node.resource_type != NodeType.SqlOperation
            and node.resource_type != NodeType.RPCCall  # TODO: rm
        )
        target_dependency = dependencies.get(target_model.package_name)
        restrict_package_access = target_dependency.restrict_access if target_dependency else False

        return is_protected_ref and (
            node.package_name != target_model.package_name and restrict_package_access
        )

    # Called by RunTask.defer_to_manifest
    def merge_from_artifact(
        self,
        adapter,
        other: "WritableManifest",
        selected: AbstractSet[UniqueID],
        favor_state: bool = False,
    ) -> None:
        """Given the selected unique IDs and a writable manifest, update this
        manifest by replacing any unselected nodes with their counterpart.

        Only non-ephemeral refable nodes are examined.
        """
        refables = set(NodeType.refable())
        merged = set()
        for unique_id, node in other.nodes.items():
            current = self.nodes.get(unique_id)
            if current and (
                node.resource_type in refables
                and not node.is_ephemeral
                and unique_id not in selected
                and (
                    not adapter.get_relation(current.database, current.schema, current.identifier)
                    or favor_state
                )
            ):
                merged.add(unique_id)
                self.nodes[unique_id] = node.replace(deferred=True)

        # Rebuild the flat_graph, which powers the 'graph' context variable,
        # now that we've deferred some nodes
        self.build_flat_graph()

        # log up to 5 items
        sample = list(islice(merged, 5))
        fire_event(MergedFromState(num_merged=len(merged), sample=sample))

    # Called by CloneTask.defer_to_manifest
    def add_from_artifact(
        self,
        other: "WritableManifest",
    ) -> None:
        """Update this manifest by *adding* information about each node's location
        in the other manifest.

        Only non-ephemeral refable nodes are examined.
        """
        refables = set(NodeType.refable())
        for unique_id, node in other.nodes.items():
            current = self.nodes.get(unique_id)
            if current and (node.resource_type in refables and not node.is_ephemeral):
                defer_relation = DeferRelation(
                    node.database, node.schema, node.alias, node.relation_name
                )
                self.nodes[unique_id] = current.replace(defer_relation=defer_relation)

    # Methods that were formerly in ParseResult

    def add_macro(self, source_file: SourceFile, macro: Macro):
        if macro.unique_id in self.macros:
            # detect that the macro exists and emit an error
            raise DuplicateMacroInPackageError(macro=macro, macro_mapping=self.macros)

        self.macros[macro.unique_id] = macro
        source_file.macros.append(macro.unique_id)

    def has_file(self, source_file: SourceFile) -> bool:
        key = source_file.file_id
        if key is None:
            return False
        if key not in self.files:
            return False
        my_checksum = self.files[key].checksum
        return my_checksum == source_file.checksum

    def add_source(self, source_file: SchemaSourceFile, source: UnpatchedSourceDefinition):
        # sources can't be overwritten!
        _check_duplicates(source, self.sources)
        self.sources[source.unique_id] = source  # type: ignore
        source_file.sources.append(source.unique_id)

    def add_node_nofile(self, node: ManifestNode):
        # nodes can't be overwritten!
        _check_duplicates(node, self.nodes)
        self.nodes[node.unique_id] = node

    def add_node(self, source_file: AnySourceFile, node: ManifestNode, test_from=None):
        self.add_node_nofile(node)
        if isinstance(source_file, SchemaSourceFile):
            if isinstance(node, GenericTestNode):
                assert test_from
                source_file.add_test(node.unique_id, test_from)
            if isinstance(node, Metric):
                source_file.metrics.append(node.unique_id)
            if isinstance(node, Exposure):
                source_file.exposures.append(node.unique_id)
            if isinstance(node, Group):
                source_file.groups.append(node.unique_id)
        else:
            source_file.nodes.append(node.unique_id)

    def add_exposure(self, source_file: SchemaSourceFile, exposure: Exposure):
        _check_duplicates(exposure, self.exposures)
        self.exposures[exposure.unique_id] = exposure
        source_file.exposures.append(exposure.unique_id)

    def add_metric(self, source_file: SchemaSourceFile, metric: Metric, generated: bool = False):
        _check_duplicates(metric, self.metrics)
        self.metrics[metric.unique_id] = metric
        if not generated:
            source_file.metrics.append(metric.unique_id)
        else:
            source_file.generated_metrics.append(metric.unique_id)

    def add_group(self, source_file: SchemaSourceFile, group: Group):
        _check_duplicates(group, self.groups)
        self.groups[group.unique_id] = group
        source_file.groups.append(group.unique_id)

    def add_disabled_nofile(self, node: GraphMemberNode):
        # There can be multiple disabled nodes for the same unique_id
        if node.unique_id in self.disabled:
            self.disabled[node.unique_id].append(node)
        else:
            self.disabled[node.unique_id] = [node]

    def add_disabled(self, source_file: AnySourceFile, node: ResultNode, test_from=None):
        self.add_disabled_nofile(node)
        if isinstance(source_file, SchemaSourceFile):
            if isinstance(node, GenericTestNode):
                assert test_from
                source_file.add_test(node.unique_id, test_from)
            if isinstance(node, Metric):
                source_file.metrics.append(node.unique_id)
            if isinstance(node, SavedQuery):
                source_file.saved_queries.append(node.unique_id)
            if isinstance(node, SemanticModel):
                source_file.semantic_models.append(node.unique_id)
            if isinstance(node, Exposure):
                source_file.exposures.append(node.unique_id)
        else:
            source_file.nodes.append(node.unique_id)

    def add_doc(self, source_file: SourceFile, doc: Documentation):
        _check_duplicates(doc, self.docs)
        self.docs[doc.unique_id] = doc
        source_file.docs.append(doc.unique_id)

    def add_semantic_model(self, source_file: SchemaSourceFile, semantic_model: SemanticModel):
        _check_duplicates(semantic_model, self.semantic_models)
        self.semantic_models[semantic_model.unique_id] = semantic_model
        source_file.semantic_models.append(semantic_model.unique_id)

    def add_saved_query(self, source_file: SchemaSourceFile, saved_query: SavedQuery) -> None:
        _check_duplicates(saved_query, self.saved_queries)
        self.saved_queries[saved_query.unique_id] = saved_query
        source_file.saved_queries.append(saved_query.unique_id)

    # end of methods formerly in ParseResult

    # Provide support for copy.deepcopy() - we just need to avoid the lock!
    # pickle and deepcopy use this. It returns a callable object used to
    # create the initial version of the object and a tuple of arguments
    # for the object, i.e. the Manifest.
    # The order of the arguments must match the order of the attributes
    # in the Manifest class declaration, because they are used as
    # positional arguments to construct a Manifest.
    def __reduce_ex__(self, protocol):
        args = (
            self.nodes,
            self.sources,
            self.macros,
            self.docs,
            self.exposures,
            self.metrics,
            self.groups,
            self.selectors,
            self.files,
            self.metadata,
            self.flat_graph,
            self.state_check,
            self.source_patches,
            self.disabled,
            self.env_vars,
            self.semantic_models,
            self.saved_queries,
            self._doc_lookup,
            self._source_lookup,
            self._ref_lookup,
            self._metric_lookup,
            self._semantic_model_by_measure_lookup,
            self._disabled_lookup,
            self._analysis_lookup,
        )
        return self.__class__, args


class MacroManifest(MacroMethods):
    def __init__(self, macros) -> None:
        self.macros = macros
        self.metadata = ManifestMetadata()
        # This is returned by the 'graph' context property
        # in the ProviderContext class.
        self.flat_graph: Dict[str, Any] = {}


AnyManifest = Union[Manifest, MacroManifest]


@dataclass
@schema_version("manifest", 12)
class WritableManifest(ArtifactMixin):
    nodes: Mapping[UniqueID, ManifestNode] = field(
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
    disabled: Optional[Mapping[UniqueID, List[GraphMemberNode]]] = field(
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
        if manifest_schema_version <= 10:
            data = upgrade_manifest_json(data, manifest_schema_version)
        return cls.from_dict(data)

    def __post_serialize__(self, dct):
        for unique_id, node in dct["nodes"].items():
            if "config_call_dict" in node:
                del node["config_call_dict"]
            if "defer_relation" in node:
                del node["defer_relation"]
        return dct


def _check_duplicates(value: BaseNode, src: Mapping[str, BaseNode]):
    if value.unique_id in src:
        raise DuplicateResourceNameError(value, src[value.unique_id])


K_T = TypeVar("K_T")
V_T = TypeVar("V_T")


def _expect_value(key: K_T, src: Mapping[K_T, V_T], old_file: SourceFile, name: str) -> V_T:
    if key not in src:
        raise CompilationError(
            'Expected to find "{}" in cached "result.{}" based '
            "on cached file information: {}!".format(key, name, old_file)
        )
    return src[key]
