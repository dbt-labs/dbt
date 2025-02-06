import threading
from queue import PriorityQueue
from typing import Dict, Generator, List, Optional, Set

import networkx as nx  # type: ignore

from dbt.artifacts.schemas.run import RunResult
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.nodes import (
    Exposure,
    GraphMemberNode,
    Metric,
    ModelNode,
    SourceDefinition,
)
from dbt.contracts.state import PreviousState
from dbt.graph.selector_spec import SelectionSpec
from dbt.node_types import NodeType

from .graph import UniqueId


class GraphQueue:
    """A fancy queue that is backed by the dependency graph.
    Note: this will mutate input!

    This queue is thread-safe for `mark_done` calls, though you must ensure
    that separate threads do not call `.empty()` or `__len__()` and `.get()` at
    the same time, as there is an unlocked race!
    """

    def __init__(
        self,
        graph: nx.DiGraph,
        manifest: Manifest,
        selected: Set[UniqueId],
        preserve_edges: bool = True,
    ) -> None:
        # 'create_empty_copy' returns a copy of the graph G with all of the edges removed, and leaves nodes intact.
        self.graph = graph if preserve_edges else nx.classes.function.create_empty_copy(graph)
        self.manifest = manifest
        self._selected = selected
        # store the queue as a priority queue.
        self.inner: PriorityQueue = PriorityQueue()
        # things that have been popped off the queue but not finished
        # and worker thread reservations
        self.in_progress: Set[UniqueId] = set()
        # things that are in the queue
        self.queued: Set[UniqueId] = set()
        # this lock controls most things
        self.lock = threading.Lock()
        # store the 'score' of each node as a number. Lower is higher priority.
        self._scores = self._get_scores(self.graph)
        # populate the initial queue
        self._find_new_additions(list(self.graph.nodes()))
        # awaits after task end
        self.some_task_done = threading.Condition(self.lock)

    def get_selected_nodes(self) -> Set[UniqueId]:
        return self._selected.copy()

    def _include_in_cost(self, node_id: UniqueId) -> bool:
        node = self.manifest.expect(node_id)
        if node.resource_type != NodeType.Model:
            return False
        # must be a Model - tell mypy this won't be a Source or Exposure or Metric
        assert not isinstance(node, (SourceDefinition, Exposure, Metric))
        if node.is_ephemeral:
            return False
        return True

    @staticmethod
    def _grouped_topological_sort(
        graph: nx.DiGraph,
    ) -> Generator[List[str], None, None]:
        """Topological sort of given graph that groups ties.

        Adapted from `nx.topological_sort`, this function returns a topo sort of a graph however
        instead of arbitrarily ordering ties in the sort order, ties are grouped together in
        lists.

        Args:
            graph: The graph to be sorted.

        Returns:
            A generator that yields lists of nodes, one list per graph depth level.
        """
        indegree_map = {v: d for v, d in graph.in_degree() if d > 0}
        zero_indegree = [v for v, d in graph.in_degree() if d == 0]

        while zero_indegree:
            yield zero_indegree
            new_zero_indegree = []
            for v in zero_indegree:
                for _, child in graph.edges(v):
                    indegree_map[child] -= 1
                    if not indegree_map[child]:
                        new_zero_indegree.append(child)
            zero_indegree = new_zero_indegree

    def _get_scores(self, graph: nx.DiGraph) -> Dict[str, int]:
        """Scoring nodes for processing order.

        Scores are calculated by the graph depth level. Lowest score (0) should be processed first.

        Args:
            graph: The graph to be scored.

        Returns:
            A dictionary consisting of `node name`:`score` pairs.
        """
        # split graph by connected subgraphs
        subgraphs = (graph.subgraph(x) for x in nx.connected_components(nx.Graph(graph)))

        # score all nodes in all subgraphs
        scores = {}
        for subgraph in subgraphs:
            grouped_nodes = self._grouped_topological_sort(subgraph)
            for level, group in enumerate(grouped_nodes):
                for node in group:
                    scores[node] = level

        return scores

    def get(self, block: bool = True, timeout: Optional[float] = None) -> GraphMemberNode:
        """Get a node off the inner priority queue. By default, this blocks.

        This takes the lock, but only for part of it.

        :param block: If True, block until the inner queue has data
        :param timeout: If set, block for timeout seconds waiting for data.
        :return: The node as present in the manifest.

        See `queue.PriorityQueue` for more information on `get()` behavior and
        exceptions.
        """
        _, node_id = self.inner.get(block=block, timeout=timeout)
        with self.lock:
            self._mark_in_progress(node_id)
        return self.manifest.expect(node_id)

    def __len__(self) -> int:
        """The length of the queue is the number of tasks left for the queue to
        give out, regardless of where they are. Incomplete tasks are not part
        of the length.

        This takes the lock.
        """
        with self.lock:
            return len(self.graph) - len(self.in_progress)

    def empty(self) -> bool:
        """The graph queue is 'empty' if it all remaining nodes in the graph
        are in progress.

        This takes the lock.
        """
        return len(self) == 0

    def _already_known(self, node: UniqueId) -> bool:
        """Decide if a node is already known (either handed out as a task, or
        in the queue).

        Callers must hold the lock.

        :param str node: The node ID to check
        :returns bool: If the node is in progress/queued.
        """
        return node in self.in_progress or node in self.queued

    def _find_new_additions(self, candidates) -> None:
        """Find any nodes in the graph that need to be added to the internal
        queue and add them.
        """
        for node in candidates:
            if self.graph.in_degree(node) == 0 and not self._already_known(node):
                self.inner.put((self._scores[node], node))
                self.queued.add(node)

    def mark_done(self, node_id: UniqueId) -> None:
        """Given a node's unique ID, mark it as done.

        This method takes the lock.

        :param str node_id: The node ID to mark as complete.
        """
        with self.lock:
            self.in_progress.remove(node_id)
            successors = list(self.graph.successors(node_id))
            self.graph.remove_node(node_id)
            self._find_new_additions(successors)
            self.inner.task_done()
            self.some_task_done.notify_all()

    def _mark_in_progress(self, node_id: UniqueId) -> None:
        """Mark the node as 'in progress'.

        Callers must hold the lock.

        :param str node_id: The node ID to mark as in progress.
        """
        self.queued.remove(node_id)
        self.in_progress.add(node_id)

    def join(self) -> None:
        """Join the queue. Blocks until all tasks are marked as done.

        Make sure not to call this before the queue reports that it is empty.
        """
        self.inner.join()

    def wait_until_something_was_done(self) -> int:
        """Block until a task is done, then return the number of unfinished
        tasks.
        """
        with self.lock:
            self.some_task_done.wait()
            return self.inner.unfinished_tasks


class ExecutionQueue:
    """
    ExecutionQueue manage what nodes to execute in what order, based on the supplied inputs.
    It is responsible for managing the queue of nodes to execute, and for marking nodes as
    done when they have been executed.
    """

    def __init__(
        self,
        manifest: Manifest,
        previous_state: PreviousState,
        resource_types: List[NodeType],
        include_empty_nodes: Optional[bool] = False,
        selection_spec: Optional[SelectionSpec] = None,
        fail_fast: Optional[bool] = False,
    ) -> None:
        """Create a new ExecutionQueue.
        Nodes to execute are selected based on the manifest, previous state, selection spec, inlcude_empty_nodes, and resource_types.
        See Args for more details.
        Example usage:

        pool = ThreadPool(4)
        queue = ExecutionQueue(manifest, previous_state, [NodeType.Model, NodeType.Test])
        def callback(result: RunResult):
            queue.handle_node_result(result)
        def run(node: GraphMemberNode):
            result = node.run()
            return result

        while queue.count() > 0:
            node = queue.get()
            pool.apply_async(run, args=(node), callback=callback)
        results = queue.join()

        Args:
            manifest (Manifest): the manifest of the project
            previous_state (PreviousState): the previous state of the project, used in state selection.
            resource_types (List[NodeType]): the types of resources to include in the selection.
            include_empty_nodes (Optional[bool]): whether to include nodes that do not have values in the selection. Defaults to False.
            selection_spec (Optional[SelectionSpec]): the selection spec to use. Defaults to None
            fail_fast (Optional[bool]): when set to True, the will will stop execution after the first error. Defaults to False.
        """
        pass

    def count(self) -> int:
        """
        Returns:
            int: the number of nodes in the queue (excluding in-progress nodes)
        """
        return 0

    def handle_node_result(self, result: RunResult) -> None:
        """Given a RunResult, mark the node as done and update the queue to make more nodes avaliable.

        Args:
            result (RunResult): _description_
        """
        pass

    def get(self, block: bool = True) -> GraphMemberNode:
        """
        Get the next node to execute.

        Args:
            block (bool, optional): whether to block until a node is available. Defaults to True.
        """
        return ModelNode()  # type: ignore

    def join(self) -> list[RunResult]:
        """Wait for all nodes to finish executing, and return the results of all nodes.

        Returns:
            list[RunResult]: the results of all nodes.
        """
        return []
