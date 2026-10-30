from typing import List

from dbt.cli import params
from dbt.deprecations import warn
from dbt_common.constants import ENGINE_ENV_PREFIX
from dbt_common.context import get_invocation_context

# These are env vars that are not in the params module, but are still allowed to be set.
# New additions to this list should use the new naming scheme, unless they are being added because
# they already existed, but we didn't know about them previously.
# TODO: Should at least some of these become (undocumented) cli param options?
_ADDITIONAL_ENGINE_ENV_VARS: List[str] = [
    "DBT_INVOCATION_ENV",
    "DBT_RECORDED_FILE_PATH",
    "DBT_TEST_STATE_MODIFIED",  # TODO: This is testing related, should we do this differently?
    "DBT_PACKAGE_HUB_URL",
    "DBT_DOWNLOAD_DIR",
    "DBT_PP_FILE_DIFF_TEST",  # TODO: This is testing related, should we do this differently?
    "DBT_PP_TEST",  # TODO: This is testing related, should we do this differently?
    "DBT_ENGINE_STATE_OAUTH_CLIENT_ID",
    "DBT_ENGINE_STATE_AUTH_URL",
    "DBT_ENGINE_STATE_TOKEN_URL",
    "DBT_ENGINE_STATE_API_URL",
]


# Here we are creating a set of all known engine env vars. This is used in this moduleto create an allow list of dbt
# engine env vars. We also use it in the cli flags module to cross propagate engine env vars with their old non-engine prefixed names.
KNOWN_ENGINE_ENV_VARS: set[params.EngineEnvVar] = set(params.KNOWN_ENV_VARS).union(
    {params.EngineEnvVar(envvar=envvar) for envvar in _ADDITIONAL_ENGINE_ENV_VARS}
)
_ALLOWED_ENV_VARS: set[str] = {engine_env_var.name for engine_env_var in KNOWN_ENGINE_ENV_VARS}


def validate_engine_env_vars() -> None:
    """
    Validate that any set environment variables that begin with the engine prefix are allowed.
    """
    env_vars = get_invocation_context()._env
    for env_var in env_vars.keys():
        if env_var.startswith(ENGINE_ENV_PREFIX) and env_var not in _ALLOWED_ENV_VARS:
            warn(
                "environment-variable-namespace-deprecation",
                env_var=env_var,
                reserved_prefix=ENGINE_ENV_PREFIX,
            )
