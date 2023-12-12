from dataclasses import dataclass, Field

from itertools import chain
from typing import Callable, Dict, Any, List, TypeVar

from dbt.common.contracts.config.metadata import Metadata
from dbt.common.exceptions import CompilationError, DbtInternalError
from dbt.common.contracts.config.properties import AdditionalPropertiesAllowed
from dbt.contracts.util import Replaceable

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
        from dbt.clients.adapter import get_config_class_by_name

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


def _listify(value: Any) -> List:
    if isinstance(value, list):
        return value[:]
    else:
        return [value]


# There are two versions of this code. The one here is for config
# objects, the one in _add_config_call in core context_config.py is for
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
