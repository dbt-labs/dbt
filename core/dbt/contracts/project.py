from dataclasses import dataclass, field
from typing import Any, ClassVar, Dict, List, Optional, Union

from mashumaro.jsonschema.annotations import Pattern
from mashumaro.types import SerializableType
from typing_extensions import Annotated

from dbt.adapters.contracts.connection import QueryComment
from dbt.contracts.util import Identifier, list_str
from dbt_common.contracts.util import Mergeable
from dbt_common.dataclass_schema import (
    ExtensibleDbtClassMixin,
    ValidationError,
    dbtClassMixin,
    dbtMashConfig,
)
from dbt_common.helper_types import NoValue

DEFAULT_SEND_ANONYMOUS_USAGE_STATS = True


class SemverString(str, SerializableType):
    def _serialize(self) -> str:
        return self

    @classmethod
    def _deserialize(cls, value: str) -> "SemverString":
        return SemverString(value)


# This supports full semver, but also allows for 2 group version numbers, (allows '1.0').
sem_ver_pattern = r"^(0|[1-9]\d*)\.(0|[1-9]\d*)(\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?)?$"


@dataclass
class Quoting(dbtClassMixin, Mergeable):
    schema: Optional[bool] = None
    database: Optional[bool] = None
    project: Optional[bool] = None
    identifier: Optional[bool] = None


@dataclass
class Package(dbtClassMixin):
    pass


@dataclass
class LocalPackage(Package):
    local: str
    unrendered: Dict[str, Any] = field(default_factory=dict)


# `float` also allows `int`, according to PEP484 (and jsonschema!)
RawVersion = Union[str, float]


@dataclass
class TarballPackage(Package):
    tarball: str
    name: str
    unrendered: Dict[str, Any] = field(default_factory=dict)


@dataclass
class GitPackage(Package):
    git: str
    revision: Optional[RawVersion] = None
    warn_unpinned: Optional[bool] = field(default=None, metadata={"alias": "warn-unpinned"})
    subdirectory: Optional[str] = None
    unrendered: Dict[str, Any] = field(default_factory=dict)

    def get_revisions(self) -> List[str]:
        if self.revision is None:
            return []
        else:
            return [str(self.revision)]


@dataclass
class PrivatePackage(Package):
    private: str
    provider: Optional[str] = None
    revision: Optional[RawVersion] = None
    warn_unpinned: Optional[bool] = field(default=None, metadata={"alias": "warn-unpinned"})
    subdirectory: Optional[str] = None
    unrendered: Dict[str, Any] = field(default_factory=dict)


@dataclass
class RegistryPackage(Package):
    package: str
    version: Union[RawVersion, List[RawVersion]]
    install_prerelease: Optional[bool] = False
    unrendered: Dict[str, Any] = field(default_factory=dict)

    def get_versions(self) -> List[str]:
        if isinstance(self.version, list):
            return [str(v) for v in self.version]
        else:
            return [str(self.version)]


PackageSpec = Union[LocalPackage, TarballPackage, GitPackage, RegistryPackage, PrivatePackage]


@dataclass
class PackageConfig(dbtClassMixin):
    packages: List[PackageSpec]

    @classmethod
    def validate(cls, data):
        for package in data.get("packages", data):
            # This can happen when the target is a variable that is not filled and results in hangs
            if isinstance(package, dict):
                if package.get("package") == "":
                    raise ValidationError(
                        "A hub package is missing the value. It is a required property."
                    )
                if package.get("local") == "":
                    raise ValidationError(
                        "A local package is missing the value. It is a required property."
                    )
                if package.get("git") == "":
                    raise ValidationError(
                        "A git package is missing the value. It is a required property."
                    )
            if isinstance(package, dict) and package.get("package"):
                if not package.get("version"):
                    raise ValidationError(
                        f"{package['package']} is missing the version. When installing from the Hub "
                        "package index, version is a required property"
                    )
                if "/" not in package["package"]:
                    raise ValidationError(
                        f"{package['package']} was not found in the package index. Packages on the index "
                        "require a namespace, e.g dbt-labs/dbt_utils"
                    )
        super().validate(data)


@dataclass
class ProjectPackageMetadata:
    name: str
    packages: List[PackageSpec]

    @classmethod
    def from_project(cls, project):
        return cls(name=project.project_name, packages=project.packages.packages)


@dataclass
class Downloads(ExtensibleDbtClassMixin):
    tarball: str


@dataclass
class RegistryPackageMetadata(
    ExtensibleDbtClassMixin,
    ProjectPackageMetadata,
):
    downloads: Downloads


# A list of all the reserved words that packages may not have as names.
BANNED_PROJECT_NAMES = {
    "_sql_results",
    "adapter",
    "api",
    "column",
    "config",
    "context",
    "database",
    "env",
    "env_var",
    "exceptions",
    "execute",
    "flags",
    "fromjson",
    "fromyaml",
    "graph",
    "invocation_id",
    "load_agate_table",
    "load_result",
    "log",
    "model",
    "modules",
    "post_hooks",
    "pre_hooks",
    "ref",
    "render",
    "return",
    "run_started_at",
    "schema",
    "source",
    "sql",
    "sql_now",
    "store_result",
    "store_raw_result",
    "target",
    "this",
    "tojson",
    "toyaml",
    "try_or_compiler_error",
    "var",
    "write",
}


@dataclass
class Project(dbtClassMixin):
    _hyphenated: ClassVar[bool] = True
    # Annotated is used by mashumaro for jsonschema generation
    name: Annotated[Identifier, Pattern(r"^[^\d\W]\w*$")]
    config_version: Optional[int] = 2
    # Annotated is used by mashumaro for jsonschema generation
    version: Optional[Union[Annotated[SemverString, Pattern(sem_ver_pattern)], float]] = None
    project_root: Optional[str] = None
    source_paths: Optional[List[str]] = None
    model_paths: Optional[List[str]] = None
    macro_paths: Optional[List[str]] = None
    data_paths: Optional[List[str]] = None  # deprecated
    seed_paths: Optional[List[str]] = None
    test_paths: Optional[List[str]] = None
    analysis_paths: Optional[List[str]] = None
    docs_paths: Optional[List[str]] = None
    asset_paths: Optional[List[str]] = None
    target_path: Optional[str] = None
    snapshot_paths: Optional[List[str]] = None
    clean_targets: Optional[List[str]] = None
    profile: Optional[str] = None
    log_path: Optional[str] = None
    packages_install_path: Optional[str] = None
    quoting: Optional[Quoting] = None
    on_run_start: Optional[List[str]] = field(default_factory=list_str)
    on_run_end: Optional[List[str]] = field(default_factory=list_str)
    require_dbt_version: Optional[Union[List[str], str]] = None
    dispatch: List[Dict[str, Any]] = field(default_factory=list)
    models: Dict[str, Any] = field(default_factory=dict)
    seeds: Dict[str, Any] = field(default_factory=dict)
    snapshots: Dict[str, Any] = field(default_factory=dict)
    analyses: Dict[str, Any] = field(default_factory=dict)
    sources: Dict[str, Any] = field(default_factory=dict)
    tests: Dict[str, Any] = field(default_factory=dict)  # deprecated
    data_tests: Dict[str, Any] = field(default_factory=dict)
    unit_tests: Dict[str, Any] = field(default_factory=dict)
    metrics: Dict[str, Any] = field(default_factory=dict)
    semantic_models: Dict[str, Any] = field(default_factory=dict)
    saved_queries: Dict[str, Any] = field(default_factory=dict)
    exposures: Dict[str, Any] = field(default_factory=dict)
    vars: Optional[Dict[str, Any]] = field(
        default=None,
        metadata=dict(
            description="map project names to their vars override dicts",
        ),
    )
    packages: List[PackageSpec] = field(default_factory=list)
    query_comment: Optional[Union[QueryComment, NoValue, str]] = field(default_factory=NoValue)
    restrict_access: bool = False
    dbt_cloud: Optional[Dict[str, Any]] = None
    flags: Dict[str, Any] = field(default_factory=dict)

    class Config(dbtMashConfig):
        # These tell mashumaro to use aliases for jsonschema and for "from_dict"
        aliases = {
            "config_version": "config-version",
            "project_root": "project-root",
            "source_paths": "source-paths",
            "model_paths": "model-paths",
            "macro_paths": "macro-paths",
            "data_paths": "data-paths",
            "seed_paths": "seed-paths",
            "test_paths": "test-paths",
            "analysis_paths": "analysis-paths",
            "docs_paths": "docs-paths",
            "asset_paths": "asset-paths",
            "target_path": "target-path",
            "snapshot_paths": "snapshot-paths",
            "clean_targets": "clean-targets",
            "log_path": "log-path",
            "packages_install_path": "packages-install-path",
            "on_run_start": "on-run-start",
            "on_run_end": "on-run-end",
            "require_dbt_version": "require-dbt-version",
            "query_comment": "query-comment",
            "restrict_access": "restrict-access",
            "semantic_models": "semantic-models",
            "saved_queries": "saved-queries",
            "dbt_cloud": "dbt-cloud",
        }

    @classmethod
    def validate(cls, data):
        super().validate(data)
        if data["name"] in BANNED_PROJECT_NAMES:
            raise ValidationError(f"Invalid project name: {data['name']} is a reserved word")
        # validate dispatch config
        if "dispatch" in data and data["dispatch"]:
            entries = data["dispatch"]
            for entry in entries:
                if (
                    "macro_namespace" not in entry
                    or "search_order" not in entry
                    or not isinstance(entry["search_order"], list)
                ):
                    raise ValidationError(f"Invalid project dispatch config: {entry}")
        if "dbt_cloud" in data and not isinstance(data["dbt_cloud"], dict):
            raise ValidationError(
                f"Invalid dbt_cloud config. Expected a 'dict' but got '{type(data['dbt_cloud'])}'"
            )
        if data.get("tests", None) and data.get("data_tests", None):
            raise ValidationError(
                "Invalid project config: cannot have both 'tests' and 'data_tests' defined"
            )


@dataclass
class ProjectFlags(ExtensibleDbtClassMixin):
    cache_selected_only: Optional[bool] = None
    debug: Optional[bool] = None
    fail_fast: Optional[bool] = None
    indirect_selection: Optional[str] = None
    log_format: Optional[str] = None
    log_format_file: Optional[str] = None
    log_level: Optional[str] = None
    log_level_file: Optional[str] = None
    partial_parse: Optional[bool] = None
    populate_cache: Optional[bool] = None
    printer_width: Optional[int] = None
    send_anonymous_usage_stats: bool = DEFAULT_SEND_ANONYMOUS_USAGE_STATS
    static_parser: Optional[bool] = None
    use_colors: Optional[bool] = None
    use_colors_file: Optional[bool] = None
    use_experimental_parser: Optional[bool] = None
    version_check: Optional[bool] = None
    warn_error: Optional[bool] = None
    warn_error_options: Optional[Dict[str, Union[str, List[str]]]] = None
    write_json: Optional[bool] = None

    # legacy behaviors - https://github.com/dbt-labs/dbt-core/blob/main/docs/guides/behavior-change-flags.md
    require_batched_execution_for_custom_microbatch_strategy: bool = False
    require_explicit_package_overrides_for_builtin_materializations: bool = True
    require_resource_names_without_spaces: bool = False
    source_freshness_run_project_hooks: bool = False
    skip_nodes_if_on_run_start_fails: bool = False
    state_modified_compare_more_unrendered_values: bool = False
    state_modified_compare_vars: bool = False
    require_yaml_configuration_for_mf_time_spines: bool = False
    require_nested_cumulative_type_params: bool = False

    @property
    def project_only_flags(self) -> Dict[str, Any]:
        return {
            "require_batched_execution_for_custom_microbatch_strategy": self.require_batched_execution_for_custom_microbatch_strategy,
            "require_explicit_package_overrides_for_builtin_materializations": self.require_explicit_package_overrides_for_builtin_materializations,
            "require_resource_names_without_spaces": self.require_resource_names_without_spaces,
            "source_freshness_run_project_hooks": self.source_freshness_run_project_hooks,
            "skip_nodes_if_on_run_start_fails": self.skip_nodes_if_on_run_start_fails,
            "state_modified_compare_more_unrendered_values": self.state_modified_compare_more_unrendered_values,
            "state_modified_compare_vars": self.state_modified_compare_vars,
            "require_yaml_configuration_for_mf_time_spines": self.require_yaml_configuration_for_mf_time_spines,
            "require_nested_cumulative_type_params": self.require_nested_cumulative_type_params,
        }


@dataclass
class ProfileConfig(dbtClassMixin):
    profile_name: str
    target_name: str
    threads: int
    # TODO: make this a dynamic union of some kind?
    credentials: Optional[Dict[str, Any]]


@dataclass
class ConfiguredQuoting(Quoting):
    identifier: bool = True
    schema: bool = True
    database: Optional[bool] = None
    project: Optional[bool] = None


@dataclass
class Configuration(Project, ProfileConfig):
    cli_vars: Dict[str, Any] = field(
        default_factory=dict,
        metadata={"preserve_underscore": True},
    )
    quoting: Optional[ConfiguredQuoting] = None


@dataclass
class ProjectList(dbtClassMixin):
    projects: Dict[str, Project]
