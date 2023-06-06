from dataclasses import field, Field, dataclass
from enum import Enum
from itertools import chain
from typing import Any, List, Optional, Dict, Union, Type, TypeVar, Callable

from dbt.dataclass_schema import (
    dbtClassMixin,
    ValidationError,
    register_pattern,
    StrEnum,
)
from dbt.contracts.graph.unparsed import AdditionalPropertiesAllowed, Docs
from dbt.contracts.graph.utils import validate_color
from dbt.contracts.util import Replaceable, list_str
from dbt.exceptions import DbtInternalError, CompilationError
from dbt import hooks
from dbt.node_types import NodeType


M = TypeVar("M", bound="Metadata")


def _get_meta_value(cls: Type[M], fld: Field, key: str, default: Any) -> M:
    # a metadata field might exist. If it does, it might have a matching key.
    # If it has both, make sure the value is valid and return it. If it
    # doesn't, return the default.
    if fld.metadata:
        value = fld.metadata.get(key, default)
    else:
        value = default

    try:
        return cls(value)
    except ValueError as exc:
        raise DbtInternalError(f"Invalid {cls} value: {value}") from exc


def _set_meta_value(obj: M, key: str, existing: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
    if existing is None:
        result = {}
    else:
        result = existing.copy()
    result.update({key: obj})
    return result


class Metadata(Enum):
    @classmethod
    def from_field(cls: Type[M], fld: Field) -> M:
        default = cls.default_field()
        key = cls.metadata_key()

        return _get_meta_value(cls, fld, key, default)

    def meta(self, existing: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
        key = self.metadata_key()
        return _set_meta_value(self, key, existing)

    @classmethod
    def default_field(cls) -> "Metadata":
        raise NotImplementedError("Not implemented")

    @classmethod
    def metadata_key(cls) -> str:
        raise NotImplementedError("Not implemented")


class MergeBehavior(Metadata):
    Append = 1
    Update = 2
    Clobber = 3
    DictKeyAppend = 4

    @classmethod
    def default_field(cls) -> "MergeBehavior":
        return cls.Clobber

    @classmethod
    def metadata_key(cls) -> str:
        return "merge"


class ShowBehavior(Metadata):
    Show = 1
    Hide = 2

    @classmethod
    def default_field(cls) -> "ShowBehavior":
        return cls.Show

    @classmethod
    def metadata_key(cls) -> str:
        return "show_hide"

    @classmethod
    def should_show(cls, fld: Field) -> bool:
        return cls.from_field(fld) == cls.Show


class CompareBehavior(Metadata):
    Include = 1
    Exclude = 2

    @classmethod
    def default_field(cls) -> "CompareBehavior":
        return cls.Include

    @classmethod
    def metadata_key(cls) -> str:
        return "compare"

    @classmethod
    def should_include(cls, fld: Field) -> bool:
        return cls.from_field(fld) == cls.Include


def metas(*metas: Metadata) -> Dict[str, Any]:
    existing: Dict[str, Any] = {}
    for m in metas:
        existing = m.meta(existing)
    return existing


def _listify(value: Any) -> List:
    if isinstance(value, list):
        return value[:]
    else:
        return [value]


# There are two versions of this code. The one here is for config
# objects, the one in _add_config_call in context_config.py is for
# config_call_dict dictionaries.
def _merge_field_value(
    merge_behavior: MergeBehavior,
    self_value: Any,
    other_value: Any,
):
    if merge_behavior == MergeBehavior.Clobber:
        return other_value
    elif merge_behavior == MergeBehavior.Append:
        return _listify(self_value) + _listify(other_value)
    elif merge_behavior == MergeBehavior.Update:
        if not isinstance(self_value, dict):
            raise DbtInternalError(f"expected dict, got {self_value}")
        if not isinstance(other_value, dict):
            raise DbtInternalError(f"expected dict, got {other_value}")
        value = self_value.copy()
        value.update(other_value)
        return value
    elif merge_behavior == MergeBehavior.DictKeyAppend:
        if not isinstance(self_value, dict):
            raise DbtInternalError(f"expected dict, got {self_value}")
        if not isinstance(other_value, dict):
            raise DbtInternalError(f"expected dict, got {other_value}")
        new_dict = {}
        for key in self_value.keys():
            new_dict[key] = _listify(self_value[key])
        for key in other_value.keys():
            extend = False
            new_key = key
            # This might start with a +, to indicate we should extend the list
            # instead of just clobbering it
            if new_key.startswith("+"):
                new_key = key.lstrip("+")
                extend = True
            if new_key in new_dict and extend:
                # extend the list
                value = other_value[key]
                new_dict[new_key].extend(_listify(value))
            else:
                # clobber the list
                new_dict[new_key] = _listify(other_value[key])
        return new_dict

    else:
        raise DbtInternalError(f"Got an invalid merge_behavior: {merge_behavior}")


def insensitive_patterns(*patterns: str):
    lowercased = []
    for pattern in patterns:
        lowercased.append("".join("[{}{}]".format(s.upper(), s.lower()) for s in pattern))
    return "^({})$".format("|".join(lowercased))


class Severity(str):
    pass


register_pattern(Severity, insensitive_patterns("warn", "error"))


class OnConfigurationChangeOption(StrEnum):
    Apply = "apply"
    Continue = "continue"
    Fail = "fail"

    @classmethod
    def default(cls) -> "OnConfigurationChangeOption":
        return cls.Apply


@dataclass
class ContractConfig(dbtClassMixin, Replaceable):
    enforced: bool = False


@dataclass
class Hook(dbtClassMixin, Replaceable):
    sql: str
    transaction: bool = True
    index: Optional[int] = None


T = TypeVar("T", bound="BaseConfig")


@dataclass
class BaseConfig(AdditionalPropertiesAllowed, Replaceable):

    # enable syntax like: config['key']
    def __getitem__(self, key):
        return self.get(key)

    # like doing 'get' on a dictionary
    def get(self, key, default=None):
        if hasattr(self, key):
            return getattr(self, key)
        elif key in self._extra:
            return self._extra[key]
        else:
            return default

    # enable syntax like: config['key'] = value
    def __setitem__(self, key, value):
        if hasattr(self, key):
            setattr(self, key, value)
        else:
            self._extra[key] = value

    def __delitem__(self, key):
        if hasattr(self, key):
            msg = (
                'Error, tried to delete config key "{}": Cannot delete ' "built-in keys"
            ).format(key)
            raise CompilationError(msg)
        else:
            del self._extra[key]

    def _content_iterator(self, include_condition: Callable[[Field], bool]):
        seen = set()
        for fld, _ in self._get_fields():
            seen.add(fld.name)
            if include_condition(fld):
                yield fld.name

        for key in self._extra:
            if key not in seen:
                seen.add(key)
                yield key

    def __iter__(self):
        yield from self._content_iterator(include_condition=lambda f: True)

    def __len__(self):
        return len(self._get_fields()) + len(self._extra)

    @staticmethod
    def compare_key(
        unrendered: Dict[str, Any],
        other: Dict[str, Any],
        key: str,
    ) -> bool:
        if key not in unrendered and key not in other:
            return True
        elif key not in unrendered and key in other:
            return False
        elif key in unrendered and key not in other:
            return False
        else:
            return unrendered[key] == other[key]

    @classmethod
    def same_contents(cls, unrendered: Dict[str, Any], other: Dict[str, Any]) -> bool:
        """This is like __eq__, except it ignores some fields."""
        seen = set()
        for fld, target_name in cls._get_fields():
            key = target_name
            seen.add(key)
            if CompareBehavior.should_include(fld):
                if not cls.compare_key(unrendered, other, key):
                    return False

        for key in chain(unrendered, other):
            if key not in seen:
                seen.add(key)
                if not cls.compare_key(unrendered, other, key):
                    return False
        return True

    # This is used in 'add_config_call' to create the combined config_call_dict.
    # 'meta' moved here from node
    mergebehavior = {
        "append": ["pre-hook", "pre_hook", "post-hook", "post_hook", "tags"],
        "update": [
            "quoting",
            "column_types",
            "meta",
            "docs",
            "contract",
        ],
        "dict_key_append": ["grants"],
    }

    @classmethod
    def _merge_dicts(cls, src: Dict[str, Any], data: Dict[str, Any]) -> Dict[str, Any]:
        """Find all the items in data that match a target_field on this class,
        and merge them with the data found in `src` for target_field, using the
        field's specified merge behavior. Matching items will be removed from
        `data` (but _not_ `src`!).

        Returns a dict with the merge results.

        That means this method mutates its input! Any remaining values in data
        were not merged.
        """
        result = {}

        for fld, target_field in cls._get_fields():
            if target_field not in data:
                continue

            data_attr = data.pop(target_field)
            if target_field not in src:
                result[target_field] = data_attr
                continue

            merge_behavior = MergeBehavior.from_field(fld)
            self_attr = src[target_field]

            result[target_field] = _merge_field_value(
                merge_behavior=merge_behavior,
                self_value=self_attr,
                other_value=data_attr,
            )
        return result

    def update_from(self: T, data: Dict[str, Any], adapter_type: str, validate: bool = True) -> T:
        """Given a dict of keys, update the current config from them, validate
        it, and return a new config with the updated values
        """
        # sadly, this is a circular import
        from dbt.adapters.factory import get_config_class_by_name

        dct = self.to_dict(omit_none=False)

        adapter_config_cls = get_config_class_by_name(adapter_type)

        self_merged = self._merge_dicts(dct, data)
        dct.update(self_merged)

        adapter_merged = adapter_config_cls._merge_dicts(dct, data)
        dct.update(adapter_merged)

        # any remaining fields must be "clobber"
        dct.update(data)

        # any validation failures must have come from the update
        if validate:
            self.validate(dct)
        return self.from_dict(dct)

    def finalize_and_validate(self: T) -> T:
        dct = self.to_dict(omit_none=False)
        self.validate(dct)
        return self.from_dict(dct)

    def replace(self, **kwargs):
        dct = self.to_dict(omit_none=True)

        mapping = self.field_mapping()
        for key, value in kwargs.items():
            new_key = mapping.get(key, key)
            dct[new_key] = value
        return self.from_dict(dct)


@dataclass
class MetricConfig(BaseConfig):
    enabled: bool = True
    group: Optional[str] = None


@dataclass
class ExposureConfig(BaseConfig):
    enabled: bool = True


@dataclass
class SourceConfig(BaseConfig):
    enabled: bool = True


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
    persist_docs: Dict[str, Any] = field(default_factory=dict)
    post_hook: List[Hook] = field(
        default_factory=list,
        metadata=MergeBehavior.Append.meta(),
    )
    pre_hook: List[Hook] = field(
        default_factory=list,
        metadata=MergeBehavior.Append.meta(),
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
            and self.on_schema_change != "append_new_columns"
        ):
            raise ValidationError(
                f"Invalid value for on_schema_change: {self.on_schema_change}. Models "
                "materialized as incremental with contracts enabled must set "
                "on_schema_change to 'append_new_columns'"
            )

    @classmethod
    def __pre_deserialize__(cls, data):
        data = super().__pre_deserialize__(data)
        field_map = {"post-hook": "post_hook", "pre-hook": "pre_hook"}
        # create a new dict because otherwise it gets overwritten in
        # tests
        new_dict = {}
        for key in data:
            new_dict[key] = data[key]
        data = new_dict
        for key in hooks.ModelHookType:
            if key in data:
                data[key] = [hooks.get_hook_dict(h) for h in data[key]]
        for field_name in field_map:
            if field_name in data:
                new_name = field_map[field_name]
                data[new_name] = data.pop(field_name)
        return data

    def __post_serialize__(self, dct):
        dct = super().__post_serialize__(dct)
        field_map = {"post_hook": "post-hook", "pre_hook": "pre-hook"}
        for field_name in field_map:
            if field_name in dct:
                dct[field_map[field_name]] = dct.pop(field_name)
        return dct

    # this is still used by jsonschema validation
    @classmethod
    def field_mapping(cls):
        return {"post_hook": "post-hook", "pre_hook": "pre-hook"}


@dataclass
class SeedConfig(NodeConfig):
    materialized: str = "seed"
    quote_columns: Optional[bool] = None

    @classmethod
    def validate(cls, data):
        super().validate(data)
        if data.get("materialized") and data.get("materialized") != "seed":
            raise ValidationError("A seed must have a materialized value of 'seed'")


@dataclass
class TestConfig(NodeAndTestConfig):
    # this is repeated because of a different default
    schema: Optional[str] = field(
        default="dbt_test__audit",
        metadata=CompareBehavior.Exclude.meta(),
    )
    materialized: str = "test"
    severity: Severity = Severity("ERROR")
    store_failures: Optional[bool] = None
    where: Optional[str] = None
    limit: Optional[int] = None
    fail_calc: str = "count(*)"
    warn_if: str = "!= 0"
    error_if: str = "!= 0"

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
        super().validate(data)
        if data.get("materialized") and data.get("materialized") != "test":
            raise ValidationError("A test must have a materialized value of 'test'")


@dataclass
class EmptySnapshotConfig(NodeConfig):
    materialized: str = "snapshot"
    unique_key: Optional[str] = None  # override NodeConfig unique_key definition


@dataclass
class SnapshotConfig(EmptySnapshotConfig):
    strategy: Optional[str] = None
    unique_key: Optional[str] = None
    target_schema: Optional[str] = None
    target_database: Optional[str] = None
    updated_at: Optional[str] = None
    # Not using Optional because of serialization issues with a Union of str and List[str]
    check_cols: Union[str, List[str], None] = None

    @classmethod
    def validate(cls, data):
        super().validate(data)
        if not data.get("strategy") or not data.get("unique_key") or not data.get("target_schema"):
            raise ValidationError(
                "Snapshots must be configured with a 'strategy', 'unique_key', "
                "and 'target_schema'."
            )
        if data.get("strategy") == "check":
            if not data.get("check_cols"):
                raise ValidationError(
                    "A snapshot configured with the check strategy must "
                    "specify a check_cols configuration."
                )
            if isinstance(data["check_cols"], str) and data["check_cols"] != "all":
                raise ValidationError(
                    f"Invalid value for 'check_cols': {data['check_cols']}. "
                    "Expected 'all' or a list of strings."
                )
        elif data.get("strategy") == "timestamp":
            if not data.get("updated_at"):
                raise ValidationError(
                    "A snapshot configured with the timestamp strategy "
                    "must specify an updated_at configuration."
                )
            if data.get("check_cols"):
                raise ValidationError("A 'timestamp' snapshot should not have 'check_cols'")
        # If the strategy is not 'check' or 'timestamp' it's a custom strategy,
        # formerly supported with GenericSnapshotConfig

        if data.get("materialized") and data.get("materialized") != "snapshot":
            raise ValidationError("A snapshot must have a materialized value of 'snapshot'")

    def finalize_and_validate(self):
        data = self.to_dict(omit_none=True)
        self.validate(data)
        return self.from_dict(data)


RESOURCE_TYPES: Dict[NodeType, Type[BaseConfig]] = {
    NodeType.Metric: MetricConfig,
    NodeType.Exposure: ExposureConfig,
    NodeType.Source: SourceConfig,
    NodeType.Seed: SeedConfig,
    NodeType.Test: TestConfig,
    NodeType.Model: NodeConfig,
    NodeType.Snapshot: SnapshotConfig,
}


# base resource types are like resource types, except nothing has mandatory
# configs.
BASE_RESOURCE_TYPES: Dict[NodeType, Type[BaseConfig]] = RESOURCE_TYPES.copy()
BASE_RESOURCE_TYPES.update({NodeType.Snapshot: EmptySnapshotConfig})


def get_config_for(resource_type: NodeType, base=False) -> Type[BaseConfig]:
    if base:
        lookup = BASE_RESOURCE_TYPES
    else:
        lookup = RESOURCE_TYPES
    return lookup.get(resource_type, NodeConfig)
