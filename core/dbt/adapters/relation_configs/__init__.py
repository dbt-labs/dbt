from dbt.adapters.relation_configs._factory import RelationConfigFactory
from dbt.adapters.relation_configs._materialized_view import (
    MaterializedViewRelationConfig,
)
from dbt.adapters.relation_configs.config_base import (
    RelationConfigBase,
    RelationResults,
)
from dbt.adapters.relation_configs.config_change import (
    RelationConfigChangeAction,
    RelationConfigChange,
)
from dbt.adapters.relation_configs.config_validation import (
    RelationConfigValidationMixin,
    RelationConfigValidationRule,
)
