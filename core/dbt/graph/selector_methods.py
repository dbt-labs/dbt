import abc
from fnmatch import fnmatch
from itertools import chain
from pathlib import Path
from typing import (
    Any,
    Callable,
    Dict,
    Iterator,
    List,
    Optional,
    Set,
    Tuple,
    Type,
    Union,
)

from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.nodes import (
    Exposure,
    GenericTestNode,
    ManifestNode,
    Metric,
    ModelNode,
    ResultNode,
    SavedQuery,
    SemanticModel,
    SingularTestNode,
    SourceDefinition,
    UnitTestDefinition,
)
from dbt.contracts.graph.unparsed import UnparsedVersion
from dbt.contracts.state import PreviousState
from dbt.node_types import NodeType
from dbt_common.dataclass_schema import StrEnum
from dbt_common.events.contextvars import get_project_root
from dbt_common.exceptions import DbtInternalError, DbtRuntimeError

from .graph import UniqueId

SELECTOR_GLOB = "*"
SELECTOR_DELIMITER = ":"


class MethodName(StrEnum):
    FQN = "fqn"
    Tag = "tag"
    Group = "group"
    Access = "access"
    Source = "source"
    Path = "path"
    File = "file"
    Package = "package"
    Config = "config"
    TestName = "test_name"
    TestType = "test_type"
    ResourceType = "resource_type"
    State = "state"
    Exposure = "exposure"
    Metric = "metric"
    Result = "result"
    SourceStatus = "source_status"
    Version = "version"
    SemanticModel = "semantic_model"
    SavedQuery = "saved_query"
    UnitTest = "unit_test"


def is_selected_node(fqn: List[str], node_selector: str, is_versioned: bool) -> bool:
    # If qualified_name exactly matches model name (fqn's leaf), return True
    if is_versioned:
        flat_node_selector = node_selector.split(".")
        if fqn[-2] == node_selector:
            return True
        # If this is a versioned model, then the last two segments should be allowed to exactly match on either the '.' or '_' delimiter
        elif "_".join(fqn[-2:]) == "_".join(flat_node_selector[-2:]):
            return True
    else:
        if fqn[-1] == node_selector:
            return True
    # Flatten node parts. Dots in model names act as namespace separators
    flat_fqn = [item for segment in fqn for item in segment.split(".")]
    # Selector components cannot be more than fqn's
    if len(flat_fqn) < len(node_selector.split(".")):
        return False

    slurp_from_ix: Optional[int] = None
    for i, selector_part in enumerate(node_selector.split(".")):
        if any(wildcard in selector_part for wildcard in ("*", "?", "[", "]")):
            slurp_from_ix = i
            break
        elif flat_fqn[i] == selector_part:
            continue
        else:
            return False

    if slurp_from_ix is not None:
        # If we have a wildcard, we need to make sure that the selector matches the
        # rest of the fqn, this is 100% backwards compatible with the old behavior of
        # encountering a wildcard but more expressive in naturally allowing you to
        # match the rest of the fqn with more advanced patterns
        return fnmatch(
            ".".join(flat_fqn[slurp_from_ix:]),
            ".".join(node_selector.split(".")[slurp_from_ix:]),
        )

    # if we get all the way down here, then the node is a match
    return True


SelectorTarget = Union[
    SourceDefinition, ManifestNode, Exposure, Metric, SemanticModel, UnitTestDefinition, SavedQuery
]


class SelectorMethod(metaclass=abc.ABCMeta):
    def __init__(
        self, manifest: Manifest, previous_state: Optional[PreviousState], arguments: List[str]
    ) -> None:
        self.manifest: Manifest = manifest
        self.previous_state = previous_state
        self.arguments: List[str] = arguments

    def parsed_nodes(
        self, included_nodes: Set[UniqueId]
    ) -> Iterator[Tuple[UniqueId, ManifestNode]]:

        for key, node in self.manifest.nodes.items():
            unique_id = UniqueId(key)
            if unique_id not in included_nodes:
                continue
            yield unique_id, node

    def source_nodes(
        self, included_nodes: Set[UniqueId]
    ) -> Iterator[Tuple[UniqueId, SourceDefinition]]:

        for key, source in self.manifest.sources.items():
            unique_id = UniqueId(key)
            if unique_id not in included_nodes:
                continue
            yield unique_id, source

    def exposure_nodes(self, included_nodes: Set[UniqueId]) -> Iterator[Tuple[UniqueId, Exposure]]:

        for key, exposure in self.manifest.exposures.items():
            unique_id = UniqueId(key)
            if unique_id not in included_nodes:
                continue
            yield unique_id, exposure

    def metric_nodes(self, included_nodes: Set[UniqueId]) -> Iterator[Tuple[UniqueId, Metric]]:

        for key, metric in self.manifest.metrics.items():
            unique_id = UniqueId(key)
            if unique_id not in included_nodes:
                continue
            yield unique_id, metric

    def unit_tests(
        self, included_nodes: Set[UniqueId]
    ) -> Iterator[Tuple[UniqueId, UnitTestDefinition]]:
        for unique_id, unit_test in self.manifest.unit_tests.items():
            unique_id = UniqueId(unique_id)
            if unique_id not in included_nodes:
                continue
            yield unique_id, unit_test

    def parsed_and_unit_nodes(self, included_nodes: Set[UniqueId]):
        yield from chain(
            self.parsed_nodes(included_nodes),
            self.unit_tests(included_nodes),
        )

    def semantic_model_nodes(
        self, included_nodes: Set[UniqueId]
    ) -> Iterator[Tuple[UniqueId, SemanticModel]]:

        for key, semantic_model in self.manifest.semantic_models.items():
            unique_id = UniqueId(key)
            if unique_id not in included_nodes:
                continue
            yield unique_id, semantic_model

    def saved_query_nodes(
        self, included_nodes: Set[UniqueId]
    ) -> Iterator[Tuple[UniqueId, SavedQuery]]:

        for key, saved_query in self.manifest.saved_queries.items():
            unique_id = UniqueId(key)
            if unique_id not in included_nodes:
                continue
            yield unique_id, saved_query

    def all_nodes(
        self, included_nodes: Set[UniqueId]
    ) -> Iterator[Tuple[UniqueId, SelectorTarget]]:
        yield from chain(
            self.parsed_nodes(included_nodes),
            self.source_nodes(included_nodes),
            self.exposure_nodes(included_nodes),
            self.metric_nodes(included_nodes),
            self.unit_tests(included_nodes),
            self.semantic_model_nodes(included_nodes),
            self.saved_query_nodes(included_nodes),
        )

    def configurable_nodes(
        self, included_nodes: Set[UniqueId]
    ) -> Iterator[Tuple[UniqueId, ResultNode]]:
        yield from chain(self.parsed_nodes(included_nodes), self.source_nodes(included_nodes))

    def non_source_nodes(
        self,
        included_nodes: Set[UniqueId],
    ) -> Iterator[Tuple[UniqueId, Union[Exposure, ManifestNode, Metric]]]:
        yield from chain(
            self.parsed_nodes(included_nodes),
            self.exposure_nodes(included_nodes),
            self.metric_nodes(included_nodes),
            self.unit_tests(included_nodes),
            self.semantic_model_nodes(included_nodes),
            self.saved_query_nodes(included_nodes),
        )

    def groupable_nodes(
        self,
        included_nodes: Set[UniqueId],
    ) -> Iterator[Tuple[UniqueId, Union[ManifestNode, Metric]]]:
        yield from chain(
            self.parsed_nodes(included_nodes),
            self.metric_nodes(included_nodes),
        )

    @abc.abstractmethod
    def search(
        self,
        included_nodes: Set[UniqueId],
        selector: str,
    ) -> Iterator[UniqueId]:
        raise NotImplementedError("subclasses should implement this")


class QualifiedNameSelectorMethod(SelectorMethod):
    def node_is_match(self, qualified_name: str, fqn: List[str], is_versioned: bool) -> bool:
        """Determine if a qualified name matches an fqn for all package
        names in the graph.

        :param str qualified_name: The qualified name to match the nodes with
        :param List[str] fqn: The node's fully qualified name in the graph.
        """
        unscoped_fqn = fqn[1:]

        if is_selected_node(fqn, qualified_name, is_versioned):
            return True
        # Match nodes across different packages
        elif is_selected_node(unscoped_fqn, qualified_name, is_versioned):
            return True

        return False

    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        """Yield all nodes in the graph that match the selector.

        :param str selector: The selector or node name
        """
        non_source_nodes = list(self.non_source_nodes(included_nodes))
        for unique_id, node in non_source_nodes:
            if self.node_is_match(selector, node.fqn, node.is_versioned):
                yield unique_id


class TagSelectorMethod(SelectorMethod):
    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        """yields nodes from included that have the specified tag"""
        for unique_id, node in self.all_nodes(included_nodes):
            if hasattr(node, "tags") and any(fnmatch(tag, selector) for tag in node.tags):
                yield unique_id


class GroupSelectorMethod(SelectorMethod):
    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        """yields nodes from included in the specified group"""
        for unique_id, node in self.groupable_nodes(included_nodes):
            node_group = node.config.get("group")
            if node_group and fnmatch(node_group, selector):
                yield unique_id


class AccessSelectorMethod(SelectorMethod):
    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        """yields model nodes matching the specified access level"""
        for unique_id, node in self.parsed_nodes(included_nodes):
            if not isinstance(node, ModelNode):
                continue
            if selector == node.access:
                yield unique_id


class SourceSelectorMethod(SelectorMethod):
    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        """yields nodes from included are the specified source."""
        parts = selector.split(".")
        target_package = SELECTOR_GLOB
        if len(parts) == 1:
            target_source, target_table = parts[0], SELECTOR_GLOB
        elif len(parts) == 2:
            target_source, target_table = parts
        elif len(parts) == 3:
            target_package, target_source, target_table = parts
        else:  # len(parts) > 3 or len(parts) == 0
            msg = (
                'Invalid source selector value "{}". Sources must be of the '
                "form `${{source_name}}`, "
                "`${{source_name}}.${{target_name}}`, or "
                "`${{package_name}}.${{source_name}}.${{target_name}}"
            ).format(selector)
            raise DbtRuntimeError(msg)

        for unique_id, node in self.source_nodes(included_nodes):
            if not fnmatch(node.package_name, target_package):
                continue
            if not fnmatch(node.source_name, target_source):
                continue
            if not fnmatch(node.name, target_table):
                continue
            yield unique_id


class ExposureSelectorMethod(SelectorMethod):
    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        parts = selector.split(".")
        target_package = SELECTOR_GLOB
        if len(parts) == 1:
            target_name = parts[0]
        elif len(parts) == 2:
            target_package, target_name = parts
        else:
            msg = (
                'Invalid exposure selector value "{}". Exposures must be of '
                "the form ${{exposure_name}} or "
                "${{exposure_package.exposure_name}}"
            ).format(selector)
            raise DbtRuntimeError(msg)

        for unique_id, node in self.exposure_nodes(included_nodes):
            if not fnmatch(node.package_name, target_package):
                continue
            if not fnmatch(node.name, target_name):
                continue

            yield unique_id


class MetricSelectorMethod(SelectorMethod):
    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        parts = selector.split(".")
        target_package = SELECTOR_GLOB
        if len(parts) == 1:
            target_name = parts[0]
        elif len(parts) == 2:
            target_package, target_name = parts
        else:
            msg = (
                'Invalid metric selector value "{}". Metrics must be of '
                "the form ${{metric_name}} or "
                "${{metric_package.metric_name}}"
            ).format(selector)
            raise DbtRuntimeError(msg)

        for unique_id, node in self.metric_nodes(included_nodes):
            if not fnmatch(node.package_name, target_package):
                continue
            if not fnmatch(node.name, target_name):
                continue

            yield unique_id


class SemanticModelSelectorMethod(SelectorMethod):
    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        parts = selector.split(".")
        target_package = SELECTOR_GLOB
        if len(parts) == 1:
            target_name = parts[0]
        elif len(parts) == 2:
            target_package, target_name = parts
        else:
            msg = (
                'Invalid semantic model selector value "{}". Semantic models must be of '
                "the form ${{semantic_model_name}} or "
                "${{semantic_model_package.semantic_model_name}}"
            ).format(selector)
            raise DbtRuntimeError(msg)

        for unique_id, node in self.semantic_model_nodes(included_nodes):
            if not fnmatch(node.package_name, target_package):
                continue
            if not fnmatch(node.name, target_name):
                continue

            yield unique_id


class SavedQuerySelectorMethod(SelectorMethod):
    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        parts = selector.split(".")
        target_package = SELECTOR_GLOB
        if len(parts) == 1:
            target_name = parts[0]
        elif len(parts) == 2:
            target_package, target_name = parts
        else:
            msg = (
                'Invalid saved query selector value "{}". Saved queries must be of '
                "the form ${{saved_query_name}} or "
                "${{saved_query_package.saved_query_name}}"
            ).format(selector)
            raise DbtRuntimeError(msg)

        for unique_id, node in self.saved_query_nodes(included_nodes):
            if not fnmatch(node.package_name, target_package):
                continue
            if not fnmatch(node.name, target_name):
                continue

            yield unique_id


class UnitTestSelectorMethod(SelectorMethod):
    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        parts = selector.split(".")
        target_package = SELECTOR_GLOB
        if len(parts) == 1:
            target_name = parts[0]
        elif len(parts) == 2:
            target_package, target_name = parts
        else:
            msg = (
                'Invalid unit test selector value "{}". Saved queries must be of '
                "the form ${{unit_test_name}} or "
                "${{unit_test_package_name.unit_test_name}}"
            ).format(selector)
            raise DbtRuntimeError(msg)

        for unique_id, node in self.unit_tests(included_nodes):
            if not fnmatch(node.package_name, target_package):
                continue
            if not fnmatch(node.name, target_name):
                continue

            yield unique_id


class PathSelectorMethod(SelectorMethod):
    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        """Yields nodes from included that match the given path."""
        # get project root from contextvar
        project_root = get_project_root()
        if project_root:
            root = Path(project_root)
        else:
            root = Path.cwd()
        paths = set(p.relative_to(root) for p in root.glob(selector))
        for unique_id, node in self.all_nodes(included_nodes):
            ofp = Path(node.original_file_path)
            if ofp in paths:
                yield unique_id
            if hasattr(node, "patch_path") and node.patch_path:  # type: ignore
                pfp = node.patch_path.split("://")[1]  # type: ignore
                ymlfp = Path(pfp)
                if ymlfp in paths:
                    yield unique_id
            if any(parent in paths for parent in ofp.parents):
                yield unique_id


class FileSelectorMethod(SelectorMethod):
    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        """Yields nodes from included that match the given file name."""
        for unique_id, node in self.all_nodes(included_nodes):
            if fnmatch(Path(node.original_file_path).name, selector):
                yield unique_id
            elif fnmatch(Path(node.original_file_path).stem, selector):
                yield unique_id


class PackageSelectorMethod(SelectorMethod):
    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        """Yields nodes from included that have the specified package"""
        # `this` is an alias for the current dbt project name
        if selector == "this" and self.manifest.metadata.project_name is not None:
            selector = self.manifest.metadata.project_name

        for unique_id, node in self.all_nodes(included_nodes):
            if fnmatch(node.package_name, selector):
                yield unique_id


def _getattr_descend(obj: Any, attrs: List[str]) -> Any:
    value = obj
    for attr in attrs:
        try:
            value = getattr(value, attr)
        except AttributeError:
            # if it implements getitem (dict, list, ...), use that. On failure,
            # raise an attribute error instead of the KeyError, TypeError, etc.
            # that arbitrary getitem calls might raise
            try:
                value = value[attr]
            except Exception as exc:
                raise AttributeError(f"'{type(value)}' object has no attribute '{attr}'") from exc
    return value


class CaseInsensitive(str):
    def __eq__(self, other):
        if isinstance(other, str):
            return self.upper() == other.upper()
        else:
            return self.upper() == other


class ConfigSelectorMethod(SelectorMethod):
    def search(
        self,
        included_nodes: Set[UniqueId],
        selector: Any,
    ) -> Iterator[UniqueId]:
        parts = self.arguments
        # special case: if the user wanted to compare test severity,
        # make the comparison case-insensitive
        if parts == ["severity"]:
            selector = CaseInsensitive(selector)

        # search sources is kind of useless now source configs only have
        # 'enabled', which you can't really filter on anyway, but maybe we'll
        # add more someday, so search them anyway.
        for unique_id, node in self.configurable_nodes(included_nodes):
            try:
                value = _getattr_descend(node.config, parts)
            except AttributeError:
                continue
            else:
                if isinstance(value, list):
                    if (
                        (selector in value)
                        or (CaseInsensitive(selector) == "true" and True in value)
                        or (CaseInsensitive(selector) == "false" and False in value)
                    ):
                        yield unique_id
                else:
                    if (
                        (selector == value)
                        or (CaseInsensitive(selector) == "true" and value is True)
                        or (CaseInsensitive(selector) == "false")
                        and value is False
                    ):
                        yield unique_id


class ResourceTypeSelectorMethod(SelectorMethod):
    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        try:
            resource_type = NodeType(selector)
        except ValueError as exc:
            raise DbtRuntimeError(f'Invalid resource_type selector "{selector}"') from exc
        for unique_id, node in self.all_nodes(included_nodes):
            if node.resource_type == resource_type:
                yield unique_id


class TestNameSelectorMethod(SelectorMethod):
    __test__ = False

    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        for unique_id, node in self.parsed_and_unit_nodes(included_nodes):
            if node.resource_type == NodeType.Test and hasattr(node, "test_metadata"):
                if fnmatch(node.test_metadata.name, selector):  # type: ignore[union-attr]
                    yield unique_id
            elif node.resource_type == NodeType.Unit:
                if fnmatch(node.name, selector):
                    yield unique_id


class TestTypeSelectorMethod(SelectorMethod):
    __test__ = False

    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        search_types: List[Any]
        # continue supporting 'schema' + 'data' for backwards compatibility
        if selector in ("generic", "schema"):
            search_types = [GenericTestNode]
        elif selector in ("data"):
            search_types = [GenericTestNode, SingularTestNode]
        elif selector in ("singular"):
            search_types = [SingularTestNode]
        elif selector in ("unit"):
            search_types = [UnitTestDefinition]
        else:
            raise DbtRuntimeError(
                f'Invalid test type selector {selector}: expected "generic", "singular", "unit", or "data"'
            )

        for unique_id, node in self.parsed_and_unit_nodes(included_nodes):
            if isinstance(node, tuple(search_types)):
                yield unique_id


class StateSelectorMethod(SelectorMethod):
    def __init__(self, *args, **kwargs) -> None:
        super().__init__(*args, **kwargs)
        self.modified_macros: Optional[List[str]] = None

    def _macros_modified(self) -> List[str]:
        # we checked in the caller!
        if self.previous_state is None or self.previous_state.manifest is None:
            raise DbtInternalError("No comparison manifest in _macros_modified")
        old_macros = self.previous_state.manifest.macros
        new_macros = self.manifest.macros

        modified = []
        for uid, macro in new_macros.items():
            if uid in old_macros:
                old_macro = old_macros[uid]
                if macro.macro_sql != old_macro.macro_sql:
                    modified.append(uid)
            else:
                modified.append(uid)

        for uid, _ in old_macros.items():
            if uid not in new_macros:
                modified.append(uid)

        return modified

    def recursively_check_macros_modified(self, node, visited_macros):
        if not hasattr(node, "depends_on"):
            return False

        for macro_uid in node.depends_on.macros:
            if macro_uid in visited_macros:
                continue
            visited_macros.append(macro_uid)

            if macro_uid in self.modified_macros:
                return True

            # this macro hasn't been modified, but depends on other
            # macros which each need to be tested for modification
            macro_node = self.manifest.macros[macro_uid]
            if len(macro_node.depends_on.macros) > 0:
                upstream_macros_changed = self.recursively_check_macros_modified(
                    macro_node, visited_macros
                )
                if upstream_macros_changed:
                    return True
                continue

            # this macro hasn't been modified, but we haven't checked
            # the other macros the node depends on, so keep looking
            if len(node.depends_on.macros) > len(visited_macros):
                continue

        return False

    def check_macros_modified(self, node):
        # check if there are any changes in macros the first time
        if self.modified_macros is None:
            self.modified_macros = self._macros_modified()
        # no macros have been modified, skip looping entirely
        if not self.modified_macros:
            return False
        # recursively loop through upstream macros to see if any is modified
        else:
            visited_macros = []
            return self.recursively_check_macros_modified(node, visited_macros)

    # TODO check modifed_content and check_modified macro seems a bit redundent
    def check_modified_content(
        self, old: Optional[SelectorTarget], new: SelectorTarget, adapter_type: str
    ) -> bool:
        if isinstance(
            new,
            (SourceDefinition, Exposure, Metric, SemanticModel, UnitTestDefinition, SavedQuery),
        ):
            # these all overwrite `same_contents`
            different_contents = not new.same_contents(old)  # type: ignore
        else:
            different_contents = not new.same_contents(old, adapter_type)  # type: ignore

        upstream_macro_change = self.check_macros_modified(new)
        return different_contents or upstream_macro_change

    def check_unmodified_content(
        self, old: Optional[SelectorTarget], new: SelectorTarget, adapter_type: str
    ) -> bool:
        return not self.check_modified_content(old, new, adapter_type)

    def check_modified_macros(self, old, new: SelectorTarget) -> bool:
        return self.check_macros_modified(new)

    @staticmethod
    def check_modified_factory(
        compare_method: str,
    ) -> Callable[[Optional[SelectorTarget], SelectorTarget], bool]:
        # get a function that compares two selector target based on compare method provided
        def check_modified_things(old: Optional[SelectorTarget], new: SelectorTarget) -> bool:
            if hasattr(new, compare_method):
                # when old body does not exist or old and new are not the same
                return not old or not getattr(new, compare_method)(old)  # type: ignore
            else:
                return False

        return check_modified_things

    @staticmethod
    def check_modified_contract(
        compare_method: str,
        adapter_type: Optional[str],
    ) -> Callable[[Optional[SelectorTarget], SelectorTarget], bool]:
        # get a function that compares two selector target based on compare method provided
        def check_modified_contract(old: Optional[SelectorTarget], new: SelectorTarget) -> bool:
            if new is None and hasattr(old, compare_method + "_removed"):
                return getattr(old, compare_method + "_removed")()
            elif hasattr(new, compare_method):
                # when old body does not exist or old and new are not the same
                return not old or not getattr(new, compare_method)(old, adapter_type)  # type: ignore
            else:
                return False

        return check_modified_contract

    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        if self.previous_state is None or self.previous_state.manifest is None:
            raise DbtRuntimeError("Got a state selector method, but no comparison manifest")

        adapter_type = self.manifest.metadata.adapter_type

        state_checks = {
            # it's new if there is no old version
            "new": lambda old, new: old is None,
            "old": lambda old, new: old is not None,
            # use methods defined above to compare properties of old + new
            "modified": self.check_modified_content,
            "unmodified": self.check_unmodified_content,
            "modified.body": self.check_modified_factory("same_body"),
            "modified.configs": self.check_modified_factory("same_config"),
            "modified.persisted_descriptions": self.check_modified_factory(
                "same_persisted_description"
            ),
            "modified.relation": self.check_modified_factory("same_database_representation"),
            "modified.macros": self.check_modified_macros,
            "modified.contract": self.check_modified_contract("same_contract", adapter_type),
            "modified.vars": self.check_modified_factory("same_vars"),
        }
        if selector in state_checks:
            checker = state_checks[selector]
        else:
            raise DbtRuntimeError(
                f'Got an invalid selector "{selector}", expected one of ' f'"{list(state_checks)}"'
            )

        manifest: Manifest = self.previous_state.manifest

        for unique_id, node in self.all_nodes(included_nodes):
            previous_node: Optional[SelectorTarget] = None

            if unique_id in manifest.nodes:
                previous_node = manifest.nodes[unique_id]
            elif unique_id in manifest.sources:
                previous_node = SourceDefinition.from_resource(manifest.sources[unique_id])
            elif unique_id in manifest.exposures:
                previous_node = Exposure.from_resource(manifest.exposures[unique_id])
            elif unique_id in manifest.metrics:
                previous_node = Metric.from_resource(manifest.metrics[unique_id])
            elif unique_id in manifest.semantic_models:
                previous_node = SemanticModel.from_resource(manifest.semantic_models[unique_id])
            elif unique_id in manifest.unit_tests:
                previous_node = UnitTestDefinition.from_resource(manifest.unit_tests[unique_id])
            elif unique_id in manifest.saved_queries:
                previous_node = SavedQuery.from_resource(manifest.saved_queries[unique_id])

            keyword_args = {}
            if checker.__name__ in [
                "same_contract",
                "check_modified_content",
                "check_unmodified_content",
            ]:
                keyword_args["adapter_type"] = adapter_type  # type: ignore

            if checker(previous_node, node, **keyword_args):  # type: ignore
                yield unique_id

        # checkers that can handle removed nodes
        if checker.__name__ in ["check_modified_contract"]:
            # ignore included_nodes, since those cannot contain removed nodes
            for previous_unique_id, previous_node in manifest.nodes.items():
                # detect removed (deleted, renamed, or disabled) nodes
                removed_node = None
                if previous_unique_id in self.manifest.disabled.keys():
                    removed_node = self.manifest.disabled[previous_unique_id][0]
                elif previous_unique_id not in self.manifest.nodes.keys():
                    removed_node = previous_node

                if removed_node:
                    # do not yield -- removed nodes should never be selected for downstream execution
                    # as they are not part of the current project's manifest.nodes
                    checker(removed_node, None, **keyword_args)  # type: ignore


class ResultSelectorMethod(SelectorMethod):
    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        if self.previous_state is None or self.previous_state.results is None:
            raise DbtInternalError("No comparison run_results")
        matches = set(
            result.unique_id for result in self.previous_state.results if result.status == selector
        )
        for unique_id, node in self.all_nodes(included_nodes):
            if unique_id in matches:
                yield unique_id


class SourceStatusSelectorMethod(SelectorMethod):
    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:

        if self.previous_state is None or self.previous_state.sources is None:
            raise DbtInternalError(
                "No previous state comparison freshness results in sources.json"
            )
        elif self.previous_state.sources_current is None:
            raise DbtInternalError("No current state comparison freshness results in sources.json")

        current_state_sources = {
            result.unique_id: getattr(result, "max_loaded_at", 0)
            for result in self.previous_state.sources_current.results
            if hasattr(result, "max_loaded_at")
        }

        current_state_sources_runtime_error = {
            result.unique_id
            for result in self.previous_state.sources_current.results
            if not hasattr(result, "max_loaded_at")
        }

        previous_state_sources = {
            result.unique_id: getattr(result, "max_loaded_at", 0)
            for result in self.previous_state.sources.results
            if hasattr(result, "max_loaded_at")
        }

        previous_state_sources_runtime_error = {
            result.unique_id
            for result in self.previous_state.sources_current.results
            if not hasattr(result, "max_loaded_at")
        }

        matches = set()
        if selector == "fresher":
            for unique_id in current_state_sources:
                if unique_id not in previous_state_sources:
                    matches.add(unique_id)
                elif current_state_sources[unique_id] > previous_state_sources[unique_id]:
                    matches.add(unique_id)

            for unique_id in matches:
                if (
                    unique_id in previous_state_sources_runtime_error
                    or unique_id in current_state_sources_runtime_error
                ):
                    matches.remove(unique_id)

        for unique_id, node in self.all_nodes(included_nodes):
            if unique_id in matches:
                yield unique_id


class VersionSelectorMethod(SelectorMethod):
    def search(self, included_nodes: Set[UniqueId], selector: str) -> Iterator[UniqueId]:
        for unique_id, node in self.parsed_nodes(included_nodes):
            if isinstance(node, ModelNode):
                if selector == "latest":
                    if node.is_latest_version:
                        yield unique_id
                elif selector == "prerelease":
                    if (
                        node.version
                        and node.latest_version
                        and UnparsedVersion(v=node.version)
                        > UnparsedVersion(v=node.latest_version)
                    ):
                        yield unique_id
                elif selector == "old":
                    if (
                        node.version
                        and node.latest_version
                        and UnparsedVersion(v=node.version)
                        < UnparsedVersion(v=node.latest_version)
                    ):
                        yield unique_id
                elif selector == "none":
                    if node.version is None:
                        yield unique_id
                else:
                    raise DbtRuntimeError(
                        f'Invalid version type selector {selector}: expected one of: "latest", "prerelease", "old", or "none"'
                    )


class MethodManager:
    SELECTOR_METHODS: Dict[MethodName, Type[SelectorMethod]] = {
        MethodName.FQN: QualifiedNameSelectorMethod,
        MethodName.Tag: TagSelectorMethod,
        MethodName.Group: GroupSelectorMethod,
        MethodName.Access: AccessSelectorMethod,
        MethodName.Source: SourceSelectorMethod,
        MethodName.Path: PathSelectorMethod,
        MethodName.File: FileSelectorMethod,
        MethodName.Package: PackageSelectorMethod,
        MethodName.Config: ConfigSelectorMethod,
        MethodName.TestName: TestNameSelectorMethod,
        MethodName.TestType: TestTypeSelectorMethod,
        MethodName.ResourceType: ResourceTypeSelectorMethod,
        MethodName.State: StateSelectorMethod,
        MethodName.Exposure: ExposureSelectorMethod,
        MethodName.Metric: MetricSelectorMethod,
        MethodName.Result: ResultSelectorMethod,
        MethodName.SourceStatus: SourceStatusSelectorMethod,
        MethodName.Version: VersionSelectorMethod,
        MethodName.SemanticModel: SemanticModelSelectorMethod,
        MethodName.SavedQuery: SavedQuerySelectorMethod,
        MethodName.UnitTest: UnitTestSelectorMethod,
    }

    def __init__(
        self,
        manifest: Manifest,
        previous_state: Optional[PreviousState],
    ) -> None:
        self.manifest = manifest
        self.previous_state = previous_state

    def get_method(self, method: MethodName, method_arguments: List[str]) -> SelectorMethod:

        if method not in self.SELECTOR_METHODS:
            raise DbtInternalError(
                f'Method name "{method}" is a valid node selection '
                f"method name, but it is not handled"
            )
        cls: Type[SelectorMethod] = self.SELECTOR_METHODS[method]
        return cls(self.manifest, self.previous_state, method_arguments)
