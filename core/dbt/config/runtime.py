import itertools
import os
from copy import deepcopy
from dataclasses import dataclass
from pathlib import Path
from typing import (
    Any,
    Dict,
    Iterable,
    Iterator,
    Mapping,
    MutableSet,
    Optional,
    Tuple,
    Type,
)

from dbt.clients.adapter import get_include_paths, get_relation_class_by_name
from dbt.adapters.contracts.connection import AdapterRequiredConfig, Credentials, HasCredentials
from dbt.adapters.contracts.relation import ComponentName
from dbt.flags import get_flags
from dbt.config.project import load_raw_project
from dbt.contracts.graph.manifest import ManifestMetadata
from dbt.contracts.project import Configuration, UserConfig
from dbt.common.dataclass_schema import ValidationError
from dbt.common.events.functions import warn_or_error
from dbt.common.events.types import UnusedResourceConfigPath
from dbt.exceptions import (
    ConfigContractBrokenError,
    DbtProjectError,
    NonUniquePackageNameError,
    DbtRuntimeError,
    UninstalledPackagesFoundError,
)
from dbt.common.helper_types import DictDefaultEmptyStr, FQNPath, PathSet
from .profile import Profile
from .project import Project
from .renderer import DbtProjectYamlRenderer, ProfileRenderer


# Called by RuntimeConfig.collect_parts class method
def load_project(
    project_root: str,
    version_check: bool,
    profile: HasCredentials,
    cli_vars: Optional[Dict[str, Any]] = None,
) -> Project:
    # get the project with all of the provided information
    project_renderer = DbtProjectYamlRenderer(profile, cli_vars)
    project = Project.from_project_root(
        project_root, project_renderer, verify_version=version_check
    )

    # Save env_vars encountered in rendering for partial parsing
    project.project_env_vars = project_renderer.ctx_obj.env_vars
    return project


def load_profile(
    project_root: str,
    cli_vars: Dict[str, Any],
    profile_name_override: Optional[str] = None,
    target_override: Optional[str] = None,
    threads_override: Optional[int] = None,
) -> Profile:
    raw_project = load_raw_project(project_root)
    raw_profile_name = raw_project.get("profile")
    profile_renderer = ProfileRenderer(cli_vars)
    profile_name = profile_renderer.render_value(raw_profile_name)
    profile = Profile.render(
        profile_renderer, profile_name, profile_name_override, target_override, threads_override
    )
    # Save env_vars encountered in rendering for partial parsing
    profile.profile_env_vars = profile_renderer.ctx_obj.env_vars
    return profile


def _project_quoting_dict(proj: Project, profile: Profile) -> Dict[ComponentName, bool]:
    src: Dict[str, Any] = profile.credentials.translate_aliases(proj.quoting)
    result: Dict[ComponentName, bool] = {}
    for key in ComponentName:
        if key in src:
            value = src[key]
            if isinstance(value, bool):
                result[key] = value
    return result


@dataclass
class RuntimeConfig(Project, Profile, AdapterRequiredConfig):
    args: Any
    profile_name: str
    cli_vars: Dict[str, Any]
    dependencies: Optional[Mapping[str, "RuntimeConfig"]] = None

    def __post_init__(self):
        self.validate()

    @classmethod
    def get_profile(
        cls,
        project_root: str,
        cli_vars: Dict[str, Any],
        args: Any,
    ) -> Profile:
        return load_profile(
            project_root,
            cli_vars,
            args.profile,
            args.target,
            args.threads,
        )

    # Called by 'new_project' and 'from_args'
    @classmethod
    def from_parts(
        cls,
        project: Project,
        profile: Profile,
        args: Any,
        dependencies: Optional[Mapping[str, "RuntimeConfig"]] = None,
    ) -> "RuntimeConfig":
        """Instantiate a RuntimeConfig from its components.

        :param profile: A parsed dbt Profile.
        :param project: A parsed dbt Project.
        :param args: The parsed command-line arguments.
        :returns RuntimeConfig: The new configuration.
        """
        quoting: Dict[str, Any] = (
            get_relation_class_by_name(profile.credentials.type)
            .get_default_quote_policy()
            .replace_dict(_project_quoting_dict(project, profile))
        ).to_dict(omit_none=True)

        cli_vars: Dict[str, Any] = getattr(args, "vars", {})
        log_cache_events: bool = getattr(args, "log_cache_events", profile.log_cache_events)

        return cls(
            project_name=project.project_name,
            version=project.version,
            project_root=project.project_root,
            model_paths=project.model_paths,
            macro_paths=project.macro_paths,
            seed_paths=project.seed_paths,
            test_paths=project.test_paths,
            analysis_paths=project.analysis_paths,
            docs_paths=project.docs_paths,
            asset_paths=project.asset_paths,
            target_path=project.target_path,
            snapshot_paths=project.snapshot_paths,
            clean_targets=project.clean_targets,
            log_path=project.log_path,
            packages_install_path=project.packages_install_path,
            packages_specified_path=project.packages_specified_path,
            quoting=quoting,
            models=project.models,
            on_run_start=project.on_run_start,
            on_run_end=project.on_run_end,
            dispatch=project.dispatch,
            seeds=project.seeds,
            snapshots=project.snapshots,
            dbt_version=project.dbt_version,
            packages=project.packages,
            manifest_selectors=project.manifest_selectors,
            selectors=project.selectors,
            query_comment=project.query_comment,
            sources=project.sources,
            tests=project.tests,
            metrics=project.metrics,
            semantic_models=project.semantic_models,
            saved_queries=project.saved_queries,
            exposures=project.exposures,
            vars=project.vars,
            config_version=project.config_version,
            unrendered=project.unrendered,
            project_env_vars=project.project_env_vars,
            restrict_access=project.restrict_access,
            profile_env_vars=profile.profile_env_vars,
            profile_name=profile.profile_name,
            target_name=profile.target_name,
            user_config=profile.user_config,
            threads=profile.threads,
            credentials=profile.credentials,
            args=args,
            cli_vars=cli_vars,
            log_cache_events=log_cache_events,
            dependencies=dependencies,
            dbt_cloud=project.dbt_cloud,
        )

    # Called by 'load_projects' in this class
    def new_project(self, project_root: str) -> "RuntimeConfig":
        """Given a new project root, read in its project dictionary, supply the
        existing project's profile info, and create a new project file.

        :param project_root: A filepath to a dbt project.
        :raises DbtProfileError: If the profile is invalid.
        :raises DbtProjectError: If project is missing or invalid.
        :returns: The new configuration.
        """
        # copy profile
        profile = Profile(**self.to_profile_info())
        profile.validate()

        # load the new project and its packages. Don't pass cli variables.
        renderer = DbtProjectYamlRenderer(profile)
        project = Project.from_project_root(
            project_root,
            renderer,
            verify_version=bool(getattr(self.args, "VERSION_CHECK", True)),
        )

        runtime_config = self.from_parts(
            project=project,
            profile=profile,
            args=deepcopy(self.args),
        )
        # force our quoting back onto the new project.
        runtime_config.quoting = deepcopy(self.quoting)
        return runtime_config

    def serialize(self) -> Dict[str, Any]:
        """Serialize the full configuration to a single dictionary. For any
        instance that has passed validate() (which happens in __init__), it
        matches the Configuration contract.

        Note that args are not serialized.

        :returns dict: The serialized configuration.
        """
        result = self.to_project_config(with_packages=True)
        result.update(self.to_profile_info(serialize_credentials=True))
        result["cli_vars"] = deepcopy(self.cli_vars)
        return result

    def validate(self):
        """Validate the configuration against its contract.

        :raises DbtProjectError: If the configuration fails validation.
        """
        try:
            Configuration.validate(self.serialize())
        except ValidationError as e:
            raise ConfigContractBrokenError(e) from e

    # Called by RuntimeConfig.from_args
    @classmethod
    def collect_parts(cls: Type["RuntimeConfig"], args: Any) -> Tuple[Project, Profile]:
        # profile_name from the project
        project_root = args.project_dir if args.project_dir else os.getcwd()
        cli_vars: Dict[str, Any] = getattr(args, "vars", {})
        profile = cls.get_profile(
            project_root,
            cli_vars,
            args,
        )
        flags = get_flags()
        project = load_project(project_root, bool(flags.VERSION_CHECK), profile, cli_vars)
        return project, profile

    # Called in task/base.py, in BaseTask.from_args
    @classmethod
    def from_args(cls, args: Any) -> "RuntimeConfig":
        """Given arguments, read in dbt_project.yml from the current directory,
        read in packages.yml if it exists, and use them to find the profile to
        load.

        :param args: The arguments as parsed from the cli.
        :raises DbtProjectError: If the project is invalid or missing.
        :raises DbtProfileError: If the profile is invalid or missing.
        :raises DbtValidationError: If the cli variables are invalid.
        """
        project, profile = cls.collect_parts(args)

        return cls.from_parts(
            project=project,
            profile=profile,
            args=args,
        )

    def get_metadata(self) -> ManifestMetadata:
        return ManifestMetadata(
            project_name=self.project_name,
            project_id=self.hashed_name(),
            adapter_type=self.credentials.type,
        )

    def _get_v2_config_paths(
        self,
        config,
        path: FQNPath,
        paths: MutableSet[FQNPath],
    ) -> PathSet:
        for key, value in config.items():
            if isinstance(value, dict) and not key.startswith("+"):
                self._get_config_paths(value, path + (key,), paths)
            else:
                paths.add(path)
        return frozenset(paths)

    def _get_config_paths(
        self,
        config: Dict[str, Any],
        path: FQNPath = (),
        paths: Optional[MutableSet[FQNPath]] = None,
    ) -> PathSet:
        if paths is None:
            paths = set()

        for key, value in config.items():
            if isinstance(value, dict) and not key.startswith("+"):
                self._get_v2_config_paths(value, path + (key,), paths)
            else:
                paths.add(path)
        return frozenset(paths)

    def get_resource_config_paths(self) -> Dict[str, PathSet]:
        """Return a dictionary with resource type keys whose values are
        lists of lists of strings, where each inner list of strings represents
        a configured path in the resource.
        """
        return {
            "models": self._get_config_paths(self.models),
            "seeds": self._get_config_paths(self.seeds),
            "snapshots": self._get_config_paths(self.snapshots),
            "sources": self._get_config_paths(self.sources),
            "tests": self._get_config_paths(self.tests),
            "metrics": self._get_config_paths(self.metrics),
            "semantic_models": self._get_config_paths(self.semantic_models),
            "saved_queries": self._get_config_paths(self.saved_queries),
            "exposures": self._get_config_paths(self.exposures),
        }

    def warn_for_unused_resource_config_paths(
        self,
        resource_fqns: Mapping[str, PathSet],
        disabled: PathSet,
    ) -> None:
        """Return a list of lists of strings, where each inner list of strings
        represents a type + FQN path of a resource configuration that is not
        used.
        """
        disabled_fqns = frozenset(tuple(fqn) for fqn in disabled)
        resource_config_paths = self.get_resource_config_paths()
        unused_resource_config_paths = []
        for resource_type, config_paths in resource_config_paths.items():
            used_fqns = resource_fqns.get(resource_type, frozenset())
            fqns = used_fqns | disabled_fqns

            for config_path in config_paths:
                if not _is_config_used(config_path, fqns):
                    resource_path = ".".join(i for i in ((resource_type,) + config_path))
                    unused_resource_config_paths.append(resource_path)

        if len(unused_resource_config_paths) == 0:
            return

        warn_or_error(UnusedResourceConfigPath(unused_config_paths=unused_resource_config_paths))

    def load_dependencies(self, base_only=False) -> Mapping[str, "RuntimeConfig"]:
        if self.dependencies is None:
            all_projects = {self.project_name: self}
            internal_packages = get_include_paths(self.credentials.type)
            if base_only:
                # Test setup -- we want to load macros without dependencies
                project_paths = itertools.chain(internal_packages)
            else:
                # raise exception if fewer installed packages than in packages.yml
                count_packages_specified = len(self.packages.packages)  # type: ignore
                count_packages_installed = len(tuple(self._get_project_directories()))
                if count_packages_specified > count_packages_installed:
                    raise UninstalledPackagesFoundError(
                        count_packages_specified,
                        count_packages_installed,
                        self.packages_specified_path,
                        self.packages_install_path,
                    )
                project_paths = itertools.chain(internal_packages, self._get_project_directories())
            for project_name, project in self.load_projects(project_paths):
                if project_name in all_projects:
                    raise NonUniquePackageNameError(project_name)
                all_projects[project_name] = project
            self.dependencies = all_projects
        return self.dependencies

    def clear_dependencies(self):
        self.dependencies = None

    # Called by 'load_dependencies' in this class
    def load_projects(self, paths: Iterable[Path]) -> Iterator[Tuple[str, "RuntimeConfig"]]:
        for path in paths:
            try:
                project = self.new_project(str(path))
            except DbtProjectError as e:
                raise DbtProjectError(
                    f"Failed to read package: {e}",
                    result_type="invalid_project",
                    path=path,
                ) from e
            else:
                yield project.project_name, project

    def _get_project_directories(self) -> Iterator[Path]:
        root = Path(self.project_root) / self.packages_install_path

        if root.exists():
            for path in root.iterdir():
                if path.is_dir() and not path.name.startswith("__"):
                    yield path


class UnsetCredentials(Credentials):
    def __init__(self) -> None:
        super().__init__("", "")

    @property
    def type(self):
        return None

    @property
    def unique_field(self):
        return None

    def connection_info(self, *args, **kwargs):
        return {}

    def _connection_keys(self):
        return ()


# This is used by commands which do not require
# a profile, i.e. dbt deps and clean
class UnsetProfile(Profile):
    def __init__(self):
        self.credentials = UnsetCredentials()
        self.user_config = UserConfig()  # This will be read in _get_rendered_profile
        self.profile_name = ""
        self.target_name = ""
        self.threads = -1

    def to_target_dict(self):
        return DictDefaultEmptyStr({})

    def __getattribute__(self, name):
        if name in {"profile_name", "target_name", "threads"}:
            raise DbtRuntimeError(f'Error: disallowed attribute "{name}" - no profile!')

        return Profile.__getattribute__(self, name)


UNUSED_RESOURCE_CONFIGURATION_PATH_MESSAGE = """\
Configuration paths exist in your dbt_project.yml file which do not \
apply to any resources.
There are {} unused configuration paths:
{}
"""


def _is_config_used(path, fqns):
    if fqns:
        for fqn in fqns:
            if len(path) <= len(fqn) and fqn[: len(path)] == path:
                return True
    return False
