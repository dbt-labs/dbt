import os
import re
from abc import ABCMeta, abstractmethod
from dataclasses import dataclass

from typing import (
    Set, Iterator, List, Optional, Dict, Union, Any, Sequence
)
from .selector_methods import MethodName
from dbt.exceptions import RuntimeException


RAW_SELECTOR_PATTERN = re.compile(
    r'\A'
    r'(?P<childs_parents>(\@))?'
    r'(?P<parents>((?P<parents_depth>(\d*))\+))?'
    r'((?P<method>(\w+)):)?(?P<value>(.*?))'
    r'(?P<children>(\+(?P<children_depth>(\d*))))?'
    r'\Z'
)


def _probably_path(value: str):
    """Decide if value is probably a path. Windows has two path separators, so
    we should check both sep ('\\') and altsep ('/') there.
    """
    if os.path.sep in value:
        return True
    elif os.path.altsep is not None and os.path.altsep in value:
        return True
    else:
        return False


def _match_to_int(match: Dict[str, str], key: str) -> Optional[int]:
    raw = match.get(key)
    # turn the empty string into None, too.
    if not raw:
        return None
    try:
        return int(raw)
    except ValueError as exc:
        raise RuntimeException(
            f'Invalid node spec - could not handle parent depth {raw}'
        ) from exc


SelectionSpec = Union[
    'SelectionCriteria',
    'SelectionIntersection',
    'SelectionDifference',
    'SelectionUnion',
]


@dataclass
class SelectionCriteria:
    raw: str
    method: MethodName
    value: str
    select_childrens_parents: bool
    select_parents: bool
    select_parents_max_depth: Optional[int]
    select_children: bool
    select_children_max_depth: Optional[int]

    def __post_init__(self):
        if self.select_children and self.select_childrens_parents:
            raise RuntimeException(
                f'Invalid node spec {self.raw} - "@" prefix and "+" suffix '
                'are incompatible'
            )

    @classmethod
    def default_method(cls, value: str) -> MethodName:
        if _probably_path(value):
            return MethodName.Path
        else:
            return MethodName.FQN

    @classmethod
    def parse_method(cls, raw: str, groupdict: Dict[str, Any]) -> MethodName:
        raw_method = groupdict.get('method')
        if raw_method is None:
            return cls.default_method(groupdict['value'])

        try:
            return MethodName(raw_method)
        except ValueError:
            raise RuntimeException(
                f'unknown selector filter "{raw_method}" in "{raw}"'
            ) from None

    @classmethod
    def from_single_spec(cls, raw: str) -> 'SelectionCriteria':
        result = RAW_SELECTOR_PATTERN.match(raw)
        if result is None:
            # bad spec!
            raise RuntimeException(f'Invalid selector spec "{raw}"')
        result_dict = result.groupdict()

        if 'value' not in result_dict:
            raise RuntimeException(
                f'Invalid node spec "{raw}" - no search value!'
            )

        method = cls.parse_method(raw, result_dict)

        parents_max_depth = _match_to_int(result_dict, 'parents_depth')
        children_max_depth = _match_to_int(result_dict, 'children_depth')

        return cls(
            raw=raw,
            method=method,
            value=result_dict['value'],
            select_childrens_parents=bool(result_dict.get('childs_parents')),
            select_parents=bool(result_dict.get('parents')),
            select_parents_max_depth=parents_max_depth,
            select_children=bool(result_dict.get('children')),
            select_children_max_depth=children_max_depth,
        )


class BaseSelectionGroup(metaclass=ABCMeta):
    def __init__(
        self,
        components: Sequence[SelectionSpec],
        expect_exists: bool = False,
        raw: Any = None,
    ):
        self.components: List[SelectionSpec] = list(components)
        self.expect_exists = expect_exists
        self.raw = raw

    def __iter__(self) -> Iterator[SelectionSpec]:
        for component in self.components:
            yield component

    @abstractmethod
    def combine_selections(self, selections: List[Set[str]]) -> Set[str]:
        raise NotImplementedError(
            '_combine_selections not implemented!'
        )


class SelectionIntersection(BaseSelectionGroup):
    def combine_selections(self, selections: List[Set[str]]) -> Set[str]:
        return set.intersection(*selections)


class SelectionDifference(BaseSelectionGroup):
    def combine_selections(self, selections: List[Set[str]]) -> Set[str]:
        return set.difference(*selections)


class SelectionUnion(BaseSelectionGroup):
    def combine_selections(self, selections: List[Set[str]]) -> Set[str]:
        return set.union(*selections)
