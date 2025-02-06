from dbt_semantic_interfaces.type_enums import TimeGranularity

DEFAULT_ENV_PLACEHOLDER = "DBT_DEFAULT_PLACEHOLDER"

SECRET_PLACEHOLDER = "$$$DBT_SECRET_START$$${}$$$DBT_SECRET_END$$$"

MAXIMUM_SEED_SIZE = 1 * 1024 * 1024
MAXIMUM_SEED_SIZE_NAME = "1MB"

PIN_PACKAGE_URL = (
    "https://docs.getdbt.com/docs/package-management#section-specifying-package-versions"
)

DBT_PROJECT_FILE_NAME = "dbt_project"
PACKAGES_FILE_NAME = "packages"
DEPENDENCIES_FILE_NAME = "dependencies.yml"
PACKAGE_LOCK_FILE_NAME = "package-lock.yml"
MANIFEST_FILE_NAME = "manifest.json"
SEMANTIC_MANIFEST_FILE_NAME = "semantic_manifest.json"
LEGACY_TIME_SPINE_MODEL_NAME = "metricflow_time_spine"
LEGACY_TIME_SPINE_GRANULARITY = TimeGranularity.DAY
MINIMUM_REQUIRED_TIME_SPINE_GRANULARITY = TimeGranularity.DAY
PARTIAL_PARSE_FILE_NAME = "partial_parse.msgpack"
PACKAGE_LOCK_HASH_KEY = "sha1_hash"
