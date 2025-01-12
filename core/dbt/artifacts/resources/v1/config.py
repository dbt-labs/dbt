import re
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Union

from mashumaro.jsonschema.annotations import Pattern
from typing_extensions import Annotated

from dbt import hooks
from dbt.artifacts.resources.base import Docs
from dbt.artifacts.resources.types import ModelHookType
from dbt.artifacts.utils.validation import validate_color
from dbt_common.contracts.config.base import BaseConfig, CompareBehavior, MergeBehavior
from dbt_common.contracts.config.materialization import OnConfigurationChangeOption
from dbt_common.contracts.config.metadata import Metadata, ShowBehavior
from dbt_common.dataclass_schema import ValidationError, dbtClassMixin


def list_str() -> List[str]:
    return []


class Severity(str):
    pass


def metas(*metas: Metadata) -> Dict[str, Any]:
    existing: Dict[str, Any] = {}
    for m in metas:
        existing = m.meta(existing)
    return existing


@dataclass
class ContractConfig(dbtClassMixin):
    enforced: bool = False
    alias_types: bool = True


@dataclass
class Hook(dbtClassMixin):
    sql: str
    transaction: bool = True
    index: Optional[int] = None


@dataclass
class NodeAndTestConfig(BaseConfig):
    enabled: bool = True
    # these fields are included in serialized output, but are not part of
    # config comparison (they are part of database_representation)
    alias: Optional[str] = field(
        default=None,
        metadata=CompareBehavior.Exclude.meta(),
    )
    schema: Optional[str] = field(
        default=None,
        metadata=CompareBehavior.Exclude.meta(),
    )
    database: Optional[str] = field(
        default=None,
        metadata=CompareBehavior.Exclude.meta(),
    )
    tags: Union[List[str], str] = field(
        default_factory=list_str,
        metadata=metas(ShowBehavior.Hide, MergeBehavior.Append, CompareBehavior.Exclude),
    )
    meta: Dict[str, Any] = field(
        default_factory=dict,
        metadata=MergeBehavior.Update.meta(),
    )
    group: Optional[str] = field(
        default=None,
        metadata=CompareBehavior.Exclude.meta(),
    )


@dataclass
class NodeConfig(NodeAndTestConfig):
    # Note: if any new fields are added with MergeBehavior, also update the
    # 'mergebehavior' dictionary
    materialized: str = "view"
    incremental_strategy: Optional[str] = None
    batch_size: Any = None
    lookback: Any = 1
    begin: Any = None
    persist_docs: Dict[str, Any] = field(default_factory=dict)
    post_hook: List[Hook] = field(
        default_factory=list,
        metadata={"merge": MergeBehavior.Append, "alias": "post-hook"},
    )
    pre_hook: List[Hook] = field(
        default_factory=list,
        metadata={"merge": MergeBehavior.Append, "alias": "pre-hook"},
    )
    quoting: Dict[str, Any] = field(
        default_factory=dict,
        metadata=MergeBehavior.Update.meta(),
    )
    # This is actually only used by seeds. Should it be available to others?
    # That would be a breaking change!
    column_types: Dict[str, Any] = field(
        default_factory=dict,
        metadata=MergeBehavior.Update.meta(),
    )
    full_refresh: Optional[bool] = None
    # 'unique_key' doesn't use 'Optional' because typing.get_type_hints was
    # sometimes getting the Union order wrong, causing serialization failures.
    unique_key: Union[str, List[str], None] = None
    on_schema_change: Optional[str] = "ignore"
    on_configuration_change: OnConfigurationChangeOption = field(
        default_factory=OnConfigurationChangeOption.default
    )
    grants: Dict[str, Any] = field(
        default_factory=dict, metadata=MergeBehavior.DictKeyAppend.meta()
    )
    packages: List[str] = field(
        default_factory=list,
        metadata=MergeBehavior.Append.meta(),
    )
    docs: Docs = field(
        default_factory=Docs,
        metadata=MergeBehavior.Update.meta(),
    )
    contract: ContractConfig = field(
        default_factory=ContractConfig,
        metadata=MergeBehavior.Update.meta(),
    )
    event_time: Any = None
    concurrent_batches: Any = None

    def __post_init__(self):
        # we validate that node_color has a suitable value to prevent dbt-docs from crashing
        if self.docs.node_color:
            node_color = self.docs.node_color
            if not validate_color(node_color):
                raise ValidationError(
                    f"Invalid color name for docs.node_color: {node_color}. "
                    "It is neither a valid HTML color name nor a valid HEX code."
                )

        if (
            self.contract.enforced
            and self.materialized == "incremental"
            and self.on_schema_change not in ("append_new_columns", "fail")
        ):
            raise ValidationError(
                f"Invalid value for on_schema_change: {self.on_schema_change}. Models "
                "materialized as incremental with contracts enabled must set "
                "on_schema_change to 'append_new_columns' or 'fail'"
            )

    @classmethod
    def __pre_deserialize__(cls, data):
        data = super().__pre_deserialize__(data)
        for key in ModelHookType:
            if key in data:
                data[key] = [hooks.get_hook_dict(h) for h in data[key]]
        return data


SEVERITY_PATTERN = r"^([Ww][Aa][Rr][Nn]|[Ee][Rr][Rr][Oo][Rr])$"


@dataclass
class TestConfig(NodeAndTestConfig):
    __test__ = False

    # this is repeated because of a different default
    schema: Optional[str] = field(
        default="dbt_test__audit",
        metadata=CompareBehavior.Exclude.meta(),
    )
    materialized: str = "test"
    # Annotated is used by mashumaro for jsonschema generation
    severity: Annotated[Severity, Pattern(SEVERITY_PATTERN)] = Severity("ERROR")
    store_failures: Optional[bool] = None
    store_failures_as: Optional[str] = None
    sql_header: Optional[str] = None
    where: Optional[str] = None
    limit: Optional[int] = None
    fail_calc: str = "count(*)"
    warn_if: str = "!= 0"
    error_if: str = "!= 0"

    def __post_init__(self):
        """
        The presence of a setting for `store_failures_as` overrides any existing setting for `store_failures`,
        regardless of level of granularity. If `store_failures_as` is not set, then `store_failures` takes effect.
        At the time of implementation, `store_failures = True` would always create a table; the user could not
        configure this. Hence, if `store_failures = True` and `store_failures_as` is not specified, then it
        should be set to "table" to mimic the existing functionality.

        A side effect of this overriding functionality is that `store_failures_as="view"` at the project
        level cannot be turned off at the model level without setting both `store_failures_as` and
        `store_failures`. The former would cascade down and override `store_failures=False`. The proposal
        is to include "ephemeral" as a value for `store_failures_as`, which effectively sets
        `store_failures=False`.

        The exception handling for this is tricky. If we raise an exception here, the entire run fails at
        parse time. We would rather well-formed models run successfully, leaving only exceptions to be rerun
        if necessary. Hence, the exception needs to be raised in the test materialization. In order to do so,
        we need to make sure that we go down the `store_failures = True` route with the invalid setting for
        `store_failures_as`. This results in the `.get()` defaulted to `True` below, instead of a normal
        dictionary lookup as is done in the `if` block. Refer to the test materialization for the
        exception that is raise as a result of an invalid value.

        The intention of this block is to behave as if `store_failures_as` is the only setting,
        but still allow for backwards compatibility for `store_failures`.
        See https://github.com/dbt-labs/dbt-core/issues/6914 for more information.
        """

        # if `store_failures_as` is not set, it gets set by `store_failures`
        # the settings below mimic existing behavior prior to `store_failures_as`
        get_store_failures_as_map = {
            True: "table",
            False: "ephemeral",
            None: None,
        }

        # if `store_failures_as` is set, it dictates what `store_failures` gets set to
        # the settings below overrides whatever `store_failures` is set to by the user
        get_store_failures_map = {
            "ephemeral": False,
            "table": True,
            "view": True,
        }

        if self.store_failures_as is None:
            self.store_failures_as = get_store_failures_as_map[self.store_failures]
        else:
            self.store_failures = get_store_failures_map.get(self.store_failures_as, True)

    @classmethod
    def same_contents(cls, unrendered: Dict[str, Any], other: Dict[str, Any]) -> bool:
        """This is like __eq__, except it explicitly checks certain fields."""
        modifiers = [
            "severity",
            "where",
            "limit",
            "fail_calc",
            "warn_if",
            "error_if",
            "store_failures",
            "store_failures_as",
            "sql_header",
        ]

        seen = set()
        for _, target_name in cls._get_fields():
            key = target_name
            seen.add(key)
            if key in modifiers:
                if not cls.compare_key(unrendered, other, key):
                    return False
        return True

    @classmethod
    def validate(cls, data):
        if data.get("severity") and not re.match(SEVERITY_PATTERN, data.get("severity")):
            raise ValidationError(
                f"Severity must be either 'warn' or 'error'. Got '{data.get('severity')}'"
            )

        super().validate(data)

        if data.get("materialized") and data.get("materialized") != "test":
            raise ValidationError("A test must have a materialized value of 'test'")
