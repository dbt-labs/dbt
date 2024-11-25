from typing import Any, Dict, Optional

from dbt.adapters.contracts.connection import AdapterRequiredConfig
from dbt.constants import DEFAULT_ENV_PLACEHOLDER
from dbt.context.base import Var, contextmember, contextproperty
from dbt.context.target import TargetContext
from dbt.exceptions import EnvVarMissingError, SecretEnvVarLocationError
from dbt.node_types import NodeType
from dbt.utils import MultiDict
from dbt_common.constants import SECRET_ENV_PREFIX
from dbt_common.context import get_invocation_context


class ConfiguredContext(TargetContext):
    # subclasses are SchemaYamlContext, MacroResolvingContext, ManifestContext
    config: AdapterRequiredConfig

    def __init__(self, config: AdapterRequiredConfig) -> None:
        super().__init__(config.to_target_dict(), config.cli_vars)
        self.config = config

    @contextproperty()
    def project_name(self) -> str:
        return self.config.project_name


class FQNLookup:
    def __init__(self, package_name: str):
        self.package_name = package_name
        self.fqn = [package_name]
        self.resource_type = NodeType.Model


class SchemaYamlVars:
    def __init__(self):
        self.env_vars = {}
        self.vars = {}


class ConfiguredVar(Var):
    def __init__(
        self,
        context: Dict[str, Any],
        config: AdapterRequiredConfig,
        project_name: str,
        schema_yaml_vars: Optional[SchemaYamlVars] = None,
    ):
        super().__init__(context, config.cli_vars)
        self._config = config
        self._project_name = project_name
        self.schema_yaml_vars = schema_yaml_vars

    def __call__(self, var_name, default=Var._VAR_NOTSET):
        my_config = self._config.load_dependencies()[self._project_name]

        var_found = False
        var_value = None

        # cli vars > active project > local project
        if var_name in self._config.cli_vars:
            var_found = True
            var_value = self._config.cli_vars[var_name]

        adapter_type = self._config.credentials.type
        lookup = FQNLookup(self._project_name)
        active_vars = self._config.vars.vars_for(lookup, adapter_type)

        all_vars = MultiDict()
        if self._config.project_name != my_config.project_name:
            all_vars.add(my_config.vars.vars_for(lookup, adapter_type))
        all_vars.add(active_vars)

        if not var_found and var_name in all_vars:
            var_found = True
            var_value = all_vars[var_name]

        if not var_found and default is not Var._VAR_NOTSET:
            var_found = True
            var_value = default

        if not var_found:
            return self.get_missing_var(var_name)
        else:
            if self.schema_yaml_vars:
                self.schema_yaml_vars.vars[var_name] = var_value

            return var_value


class SchemaYamlContext(ConfiguredContext):
    # subclass is DocsRuntimeContext
    def __init__(self, config, project_name: str, schema_yaml_vars: Optional[SchemaYamlVars]):
        super().__init__(config)
        self._project_name = project_name
        self.schema_yaml_vars = schema_yaml_vars

    @contextproperty()
    def var(self) -> ConfiguredVar:
        return ConfiguredVar(self._ctx, self.config, self._project_name, self.schema_yaml_vars)

    @contextmember()
    def env_var(self, var: str, default: Optional[str] = None) -> str:
        return_value = None
        if var.startswith(SECRET_ENV_PREFIX):
            raise SecretEnvVarLocationError(var)
        env = get_invocation_context().env
        if var in env:
            return_value = env[var]
        elif default is not None:
            return_value = default

        if return_value is not None:
            if self.schema_yaml_vars:
                # If the environment variable is set from a default, store a string indicating
                # that so we can skip partial parsing.  Otherwise the file will be scheduled for
                # reparsing. If the default changes, the file will have been updated and therefore
                # will be scheduled for reparsing anyways.
                self.schema_yaml_vars.env_vars[var] = (
                    return_value if var in env else DEFAULT_ENV_PLACEHOLDER
                )

            return return_value
        else:
            raise EnvVarMissingError(var)


class MacroResolvingContext(ConfiguredContext):
    def __init__(self, config):
        super().__init__(config)

    @contextproperty()
    def var(self) -> ConfiguredVar:
        return ConfiguredVar(self._ctx, self.config, self.config.project_name)


def generate_schema_yml_context(
    config: AdapterRequiredConfig,
    project_name: str,
    schema_yaml_vars: Optional[SchemaYamlVars] = None,
) -> Dict[str, Any]:
    ctx = SchemaYamlContext(config, project_name, schema_yaml_vars)
    return ctx.to_dict()


def generate_macro_context(
    config: AdapterRequiredConfig,
) -> Dict[str, Any]:
    ctx = MacroResolvingContext(config)
    return ctx.to_dict()
