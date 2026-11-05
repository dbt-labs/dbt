from abc import abstractmethod
from copy import deepcopy
from dataclasses import dataclass
from typing import Any, Dict, Generic, Iterator, List, Optional, TypeVar

from mashumaro.exceptions import InvalidFieldValue

from dbt.adapters.factory import get_config_class_by_name
from dbt.config import IsFQNResource, Project, RuntimeConfig
from dbt.contracts.graph.model_config import get_config_for
from dbt.exceptions import ParsingError, SchemaConfigError
from dbt.flags import get_flags
from dbt.node_types import NodeType
from dbt.utils import fqn_search
from dbt_common.contracts.config.base import BaseConfig, merge_config_dicts
from dbt_common.dataclass_schema import ValidationError
from dbt_common.exceptions import DbtInternalError


@dataclass
class ModelParts(IsFQNResource):
    fqn: List[str]
    resource_type: NodeType
    package_name: str


T = TypeVar("T")  # any old type
C = TypeVar("C", bound=BaseConfig)


class ConfigSource:
    def __init__(self, project):
        self.project = project

    def get_config_dict(self, resource_type: NodeType): ...


class UnrenderedConfig(ConfigSource):
    def __init__(self, project: Project):
        self.project = project

    def get_config_dict(self, resource_type: NodeType) -> Dict[str, Any]:
        unrendered = self.project.unrendered.project_dict
        if resource_type == NodeType.Seed:
            model_configs = unrendered.get("seeds")
        elif resource_type == NodeType.Snapshot:
            model_configs = unrendered.get("snapshots")
        elif resource_type == NodeType.Source:
            model_configs = unrendered.get("sources")
        elif resource_type == NodeType.Test:
            model_configs = unrendered.get("data_tests")
        elif resource_type == NodeType.Metric:
            model_configs = unrendered.get("metrics")
        elif resource_type == NodeType.SemanticModel:
            model_configs = unrendered.get("semantic_models")
        elif resource_type == NodeType.SavedQuery:
            model_configs = unrendered.get("saved_queries")
        elif resource_type == NodeType.Exposure:
            model_configs = unrendered.get("exposures")
        elif resource_type == NodeType.Unit:
            model_configs = unrendered.get("unit_tests")
        elif resource_type == NodeType.Analysis:
            # Always use analyses: — reading models: wrongly disables package analyses (#15415).
            model_configs = unrendered.get("analyses")
        else:
            model_configs = unrendered.get("models")
        if model_configs is None:
            return {}
        else:
            return model_configs


class RenderedConfig(ConfigSource):
    def __init__(self, project: Project):
        self.project = project

    def get_config_dict(self, resource_type: NodeType) -> Dict[str, Any]:
        if resource_type == NodeType.Seed:
            model_configs = self.project.seeds
        elif resource_type == NodeType.Snapshot:
            model_configs = self.project.snapshots
        elif resource_type == NodeType.Source:
            model_configs = self.project.sources
        elif resource_type == NodeType.Test:
            model_configs = self.project.data_tests
        elif resource_type == NodeType.Metric:
            model_configs = self.project.metrics
        elif resource_type == NodeType.SemanticModel:
            model_configs = self.project.semantic_models
        elif resource_type == NodeType.SavedQuery:
            model_configs = self.project.saved_queries
        elif resource_type == NodeType.Exposure:
            model_configs = self.project.exposures
        elif resource_type == NodeType.Unit:
            model_configs = self.project.unit_tests
        elif resource_type == NodeType.Analysis:
            # Same as UnrenderedConfig: analyses read from analyses:, not models:.
            model_configs = self.project.analyses
        elif resource_type == NodeType.Function:
            model_configs = self.project.functions
        else:
            model_configs = self.project.models
        return model_configs


class BaseContextConfigGenerator(Generic[T]):
    def __init__(self, active_project: RuntimeConfig):
        self._active_project = active_project

    def get_config_source(self, project: Project) -> ConfigSource:
        return RenderedConfig(project)

    def get_node_project(self, project_name: str):
        if project_name == self._active_project.project_name:
            return self._active_project
        dependencies = self._active_project.load_dependencies()
        if project_name not in dependencies:
            raise DbtInternalError(
                f"Project name {project_name} not found in dependencies "
                f"(found {list(dependencies)})"
            )
        return dependencies[project_name]

    def _project_configs(
        self, project: Project, fqn: List[str], resource_type: NodeType
    ) -> Iterator[Dict[str, Any]]:
        src = self.get_config_source(project)
        model_configs = src.get_config_dict(resource_type)
        for level_config in fqn_search(model_configs, fqn):
            result = {}
            for key, value in level_config.items():
                if key.startswith("+"):
                    result[key[1:].strip()] = deepcopy(value)
                elif not isinstance(value, dict):
                    result[key] = deepcopy(value)

            yield result

    def _active_project_configs(
        self, fqn: List[str], resource_type: NodeType
    ) -> Iterator[Dict[str, Any]]:
        return self._project_configs(self._active_project, fqn, resource_type)

    @abstractmethod
    def _update_from_config(
        self, result: T, partial: Dict[str, Any], validate: bool = False
    ) -> T: ...

    @abstractmethod
    def initial_result(self, resource_type: NodeType, base: bool) -> T: ...

    def calculate_node_config(
        self,
        # this is the config from the sql file
        config_call_dict: Dict[str, Any],
        fqn: List[str],
        resource_type: NodeType,
        project_name: str,
        base: bool,
        # this is the config from the schema file
        patch_config_dict: Optional[Dict[str, Any]] = None,
    ) -> BaseConfig:
        own_config = self.get_node_project(project_name)

        result = self.initial_result(resource_type=resource_type, base=base)

        # builds the config from what was specified in the runtime_config, which generally
        # comes from the project's dbt_project.yml file.
        project_configs = self._project_configs(own_config, fqn, resource_type)
        for fqn_config in project_configs:
            result = self._update_from_config(result, fqn_config)

        # When schema files patch config, it has lower precedence than
        # config in the models (config_call_dict), so we add the patch_config_dict
        # before the config_call_dict
        if patch_config_dict:
            result = self._update_from_config(result, patch_config_dict)

        # config_calls are created in the 'experimental' model parser and
        # the ParseConfigObject (via add_config_call)
        result = self._update_from_config(result, config_call_dict)

        if own_config.project_name != self._active_project.project_name:
            for fqn_config in self._active_project_configs(fqn, resource_type):
                result = self._update_from_config(result, fqn_config)

        # this is mostly impactful in the snapshot config case
        # TODO CT-211
        return result  # type: ignore[return-value]

    @abstractmethod
    def calculate_node_config_dict(
        self,
        config_call_dict: Dict[str, Any],
        fqn: List[str],
        resource_type: NodeType,
        project_name: str,
        base: bool,
        patch_config_dict: Optional[Dict[str, Any]] = None,
    ) -> Dict[str, Any]: ...


class ContextConfigGenerator(BaseContextConfigGenerator[C]):
    def __init__(self, active_project: RuntimeConfig):
        self._active_project = active_project

    def get_config_source(self, project: Project) -> ConfigSource:
        return RenderedConfig(project)

    def initial_result(self, resource_type: NodeType, base: bool) -> C:
        # defaults, own_config, config calls, active_config (if != own_config)
        config_cls = get_config_for(resource_type, base=base)
        # Calculate the defaults. We don't want to validate the defaults,
        # because it might be invalid in the case of required config members
        # (such as on snapshots!)
        result = config_cls.from_dict({})
        return result

    def _update_from_config(self, result: C, partial: Dict[str, Any], validate: bool = False) -> C:
        translated = self._active_project.credentials.translate_aliases(partial)
        translated = self.translate_hook_names(translated)

        adapter_type = self._active_project.credentials.type
        adapter_config_cls = get_config_class_by_name(adapter_type)

        try:
            updated = result.update_from(translated, adapter_config_cls, validate=validate)
        except InvalidFieldValue as e:
            raise ParsingError(str(e))

        return updated

    def translate_hook_names(self, project_dict):
        # This is a kind of kludge because the fix for #6411 specifically allowed misspelling
        # the hook field names in dbt_project.yml, which only ever worked because we didn't
        # run validate on the dbt_project configs.
        if "pre_hook" in project_dict:
            project_dict["pre-hook"] = project_dict.pop("pre_hook")
        if "post_hook" in project_dict:
            project_dict["post-hook"] = project_dict.pop("post_hook")
        return project_dict

    def calculate_node_config_dict(
        self,
        config_call_dict: Dict[str, Any],
        fqn: List[str],
        resource_type: NodeType,
        project_name: str,
        base: bool,
        patch_config_dict: Optional[dict] = None,
    ) -> Dict[str, Any]:
        config = self.calculate_node_config(
            config_call_dict=config_call_dict,
            fqn=fqn,
            resource_type=resource_type,
            project_name=project_name,
            base=base,
            patch_config_dict=patch_config_dict,
        )
        try:
            finalized = config.finalize_and_validate()
            return finalized.to_dict(omit_none=True)
        except ValidationError as exc:
            # we got a ValidationError - probably bad types in config()
            raise SchemaConfigError(exc, node=config) from exc


class UnrenderedConfigGenerator(BaseContextConfigGenerator[Dict[str, Any]]):
    def get_config_source(self, project: Project) -> ConfigSource:
        return UnrenderedConfig(project)

    def calculate_node_config_dict(
        self,
        config_call_dict: Dict[str, Any],
        fqn: List[str],
        resource_type: NodeType,
        project_name: str,
        base: bool,
        patch_config_dict: Optional[dict] = None,
    ) -> Dict[str, Any]:
        # TODO CT-211
        return self.calculate_node_config(
            config_call_dict=config_call_dict,
            fqn=fqn,
            resource_type=resource_type,
            project_name=project_name,
            base=base,
            patch_config_dict=patch_config_dict,
        )  # type: ignore[return-value]

    def initial_result(self, resource_type: NodeType, base: bool) -> Dict[str, Any]:
        return {}

    def _update_from_config(
        self,
        result: Dict[str, Any],
        partial: Dict[str, Any],
        validate: bool = False,
    ) -> Dict[str, Any]:
        translated = self._active_project.credentials.translate_aliases(partial)
        result.update(translated)
        return result


class ContextConfig:
    def __init__(
        self,
        active_project: RuntimeConfig,
        fqn: List[str],
        resource_type: NodeType,
        project_name: str,
    ) -> None:
        self._config_call_dict: Dict[str, Any] = {}
        self._unrendered_config_call_dict: Dict[str, Any] = {}
        self._active_project = active_project
        self._fqn = fqn
        self._resource_type = resource_type
        self._project_name = project_name

    def add_config_call(self, opts: Dict[str, Any]) -> None:
        dct = self._config_call_dict
        merge_config_dicts(dct, opts)

    def add_unrendered_config_call(self, opts: Dict[str, Any]) -> None:
        # Cannot perform complex merge behaviours on unrendered configs as they may not be appropriate types.
        self._unrendered_config_call_dict.update(opts)

    def build_config_dict(
        self,
        base: bool = False,
        *,
        rendered: bool = True,
        patch_config_dict: Optional[dict] = None,
    ) -> Dict[str, Any]:
        if rendered:
            # TODO CT-211
            src = ContextConfigGenerator(self._active_project)  # type: ignore[var-annotated]
            config_call_dict = self._config_call_dict
        else:
            # TODO CT-211
            src = UnrenderedConfigGenerator(self._active_project)  # type: ignore[assignment]

            # preserve legacy behaviour - using unreliable (potentially rendered) _config_call_dict
            if get_flags().state_modified_compare_more_unrendered_values is False:
                config_call_dict = self._config_call_dict
            else:
                # Prefer _config_call_dict if it is available and _unrendered_config_call_dict is not,
                # as _unrendered_config_call_dict is unreliable for non-sql nodes (e.g. no jinja config block rendered for python models, etc)
                if self._config_call_dict and not self._unrendered_config_call_dict:
                    config_call_dict = self._config_call_dict
                else:
                    config_call_dict = self._unrendered_config_call_dict

        return src.calculate_node_config_dict(
            config_call_dict=config_call_dict,
            fqn=self._fqn,
            resource_type=self._resource_type,
            project_name=self._project_name,
            base=base,
            patch_config_dict=patch_config_dict,
        )
