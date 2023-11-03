from dbt.adapters.relation_configs.config_base import RelationConfigBase


class MaterializedViewRelationConfig(RelationConfigBase):
    @property
    def auto_refresh(self) -> bool:
        return False
