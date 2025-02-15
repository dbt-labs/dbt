from dataclasses import dataclass, field
from typing import Dict, List, Literal, Optional, Union

from dbt.artifacts.resources.types import NodeType
from dbt.artifacts.resources.v1.components import CompiledResource, DeferRelation
from dbt.artifacts.resources.v1.config import NodeConfig
from dbt_common.dataclass_schema import ValidationError, dbtClassMixin


@dataclass
class SnapshotMetaColumnNames(dbtClassMixin):
    dbt_valid_to: Optional[str] = None
    dbt_valid_from: Optional[str] = None
    dbt_scd_id: Optional[str] = None
    dbt_updated_at: Optional[str] = None
    dbt_is_deleted: Optional[str] = None


@dataclass
class SnapshotConfig(NodeConfig):
    materialized: str = "snapshot"
    strategy: Optional[str] = None
    unique_key: Union[str, List[str], None] = None
    target_schema: Optional[str] = None
    target_database: Optional[str] = None
    updated_at: Optional[str] = None
    # Not using Optional because of serialization issues with a Union of str and List[str]
    check_cols: Union[str, List[str], None] = None
    snapshot_meta_column_names: SnapshotMetaColumnNames = field(
        default_factory=SnapshotMetaColumnNames
    )
    dbt_valid_to_current: Optional[str] = None

    @property
    def snapshot_table_column_names(self):
        return {
            "dbt_valid_from": self.snapshot_meta_column_names.dbt_valid_from or "dbt_valid_from",
            "dbt_valid_to": self.snapshot_meta_column_names.dbt_valid_to or "dbt_valid_to",
            "dbt_scd_id": self.snapshot_meta_column_names.dbt_scd_id or "dbt_scd_id",
            "dbt_updated_at": self.snapshot_meta_column_names.dbt_updated_at or "dbt_updated_at",
            "dbt_is_deleted": self.snapshot_meta_column_names.dbt_is_deleted or "dbt_is_deleted",
        }

    def final_validate(self):
        if not self.strategy or not self.unique_key:
            raise ValidationError(
                "Snapshots must be configured with a 'strategy' and 'unique_key'."
            )
        if self.strategy == "check":
            if not self.check_cols:
                raise ValidationError(
                    "A snapshot configured with the check strategy must "
                    "specify a check_cols configuration."
                )
            if isinstance(self.check_cols, str) and self.check_cols != "all":
                raise ValidationError(
                    f"Invalid value for 'check_cols': {self.check_cols}. "
                    "Expected 'all' or a list of strings."
                )
            # Validate if there are duplicate column names in check_cols.
            if isinstance(self.check_cols, list):
                if len(self.check_cols) != len(set(self.check_cols)):
                    raise ValidationError(
                        f"Duplicate column names in 'check_cols':" f" {self.check_cols}."
                    )
        elif self.strategy == "timestamp":
            if not self.updated_at:
                raise ValidationError(
                    "A snapshot configured with the timestamp strategy "
                    "must specify an updated_at configuration."
                )
            if self.check_cols:
                raise ValidationError("A 'timestamp' snapshot should not have 'check_cols'")
        # If the strategy is not 'check' or 'timestamp' it's a custom strategy,
        # formerly supported with GenericSnapshotConfig

        if self.materialized and self.materialized != "snapshot":
            raise ValidationError("A snapshot must have a materialized value of 'snapshot'")

    # Called by "calculate_node_config_dict" in ContextConfigGenerator
    def finalize_and_validate(self):
        data = self.to_dict(omit_none=True)
        self.validate(data)
        return self.from_dict(data)


@dataclass
class Snapshot(CompiledResource):
    resource_type: Literal[NodeType.Snapshot]
    config: SnapshotConfig
    defer_relation: Optional[DeferRelation] = None

    def __post_serialize__(self, dct, context: Optional[Dict] = None):
        dct = super().__post_serialize__(dct, context)
        if context and context.get("artifact") and "defer_relation" in dct:
            del dct["defer_relation"]
        return dct
