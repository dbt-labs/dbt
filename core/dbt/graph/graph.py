from functools import partial
from itertools import product
from typing import Collection, Iterable, Iterator, NewType, Optional, Set, Tuple

import networkx as nx  # type: ignore

from dbt_common.exceptions import DbtInternalError

UniqueId = NewType("UniqueId", str)


class Graph:
    """A wrapper around the networkx graph that understands SelectionCriteria
    and how they interact with the graph.
    """

    def __init__(self, graph: nx.DiGraph) -> None:
        self.graph: nx.DiGraph = graph

    def nodes(self) -> Set[UniqueId]:
        return set(self.graph.nodes())

    def edges(self) -> Collection[Tuple[UniqueId, UniqueId]]:
        return self.graph.edges()

    def __iter__(self) -> Iterator[UniqueId]:
        return iter(self.graph.nodes())

    def ancestors(self, node: UniqueId, max_depth: Optional[int] = None) -> Set[UniqueId]:
        """Returns all nodes having a path to `node` in `graph`"""
        if not self.graph.has_node(node):
            raise DbtInternalError(f"Node {node} not found in the graph!")
        filtered_graph = self.exclude_edge_type("parent_test")
        return {
            child
            for _, child in nx.bfs_edges(filtered_graph, node, reverse=True, depth_limit=max_depth)
        }

    def descendants(self, node: UniqueId, max_depth: Optional[int] = None) -> Set[UniqueId]:
        """Returns all nodes reachable from `node` in `graph`"""
        if not self.graph.has_node(node):
            raise DbtInternalError(f"Node {node} not found in the graph!")
        filtered_graph = self.exclude_edge_type("parent_test")
        return {child for _, child in nx.bfs_edges(filtered_graph, node, depth_limit=max_depth)}

    def exclude_edge_type(self, edge_type_to_exclude) -> nx.Graph:
        return nx.subgraph_view(
            self.graph,
            filter_edge=partial(self.filter_edges_by_type, edge_type=edge_type_to_exclude),
        )

    def filter_edges_by_type(self, first_node, second_node, edge_type) -> bool:
        return self.graph.get_edge_data(first_node, second_node).get("edge_type") != edge_type

    def select_childrens_parents(self, selected: Set[UniqueId]) -> Set[UniqueId]:
        ancestors_for = self.select_children(selected) | selected
        return self.select_parents(ancestors_for) | ancestors_for

    def select_children(
        self, selected: Set[UniqueId], max_depth: Optional[int] = None
    ) -> Set[UniqueId]:
        """Returns all nodes which are descendants of the 'selected' set.
        Nodes in the 'selected' set are counted as children only if
        they are descendants of other nodes in the 'selected' set."""
        children: Set[UniqueId] = set()
        i = 0
        while len(selected) > 0 and (max_depth is None or i < max_depth):
            next_layer: Set[UniqueId] = set()
            for node in selected:
                next_layer.update(
                    iter(
                        e[1]
                        for e in self.graph.out_edges(node)
                        if e[1] not in children
                        and self.filter_edges_by_type(e[0], e[1], "parent_test")
                    )
                )
            children.update(next_layer)
            selected = next_layer
            i += 1

        return children

    def select_parents(
        self, selected: Set[UniqueId], max_depth: Optional[int] = None
    ) -> Set[UniqueId]:
        """Returns all nodes which are ancestors of the 'selected' set.
        Nodes in the 'selected' set are counted as parents only if
        they are ancestors of other nodes in the 'selected' set."""
        parents: Set[UniqueId] = set()
        i = 0
        while len(selected) > 0 and (max_depth is None or i < max_depth):
            next_layer: Set[UniqueId] = set()
            for node in selected:
                next_layer.update(
                    iter(
                        e[0]
                        for e in self.graph.in_edges(node)
                        if e[0] not in parents
                        and self.filter_edges_by_type(e[0], e[1], "parent_test")
                    )
                )
            parents.update(next_layer)
            selected = next_layer
            i += 1

        return parents

    def select_successors(self, selected: Set[UniqueId]) -> Set[UniqueId]:
        successors: Set[UniqueId] = set()
        for node in selected:
            successors.update(self.graph.successors(node))
        return successors

    def get_subset_graph(self, selected: Iterable[UniqueId]) -> "Graph":
        """Create and return a new graph that is a shallow copy of the graph,
        but with only the nodes in include_nodes. Transitive edges across
        removed nodes are preserved as explicit new edges.
        """

        new_graph: nx.DiGraph = self.graph.copy()
        include_nodes: Set[UniqueId] = set(selected)

        still_removing: bool = True
        while still_removing:
            nodes_to_remove = list(
                node
                for node in new_graph
                if node not in include_nodes
                and (new_graph.in_degree(node) * new_graph.out_degree(node)) == 0
            )
            if len(nodes_to_remove) == 0:
                still_removing = False
            else:
                new_graph.remove_nodes_from(nodes_to_remove)

        # sort remaining nodes by degree
        remaining_nodes = list(new_graph.nodes())
        remaining_nodes.sort(
            key=lambda node: new_graph.in_degree(node) * new_graph.out_degree(node)
        )

        for node in remaining_nodes:
            if node not in include_nodes:
                source_nodes = [x for x, _ in new_graph.in_edges(node)]
                target_nodes = [x for _, x in new_graph.out_edges(node)]

                new_edges = product(source_nodes, target_nodes)
                non_cyclic_new_edges = [
                    (source, target)
                    for source, target in new_edges
                    if source != target and not new_graph.has_edge(source, target)
                ]  # removes cyclic refs and edges already existing in new graph

                new_graph.add_edges_from(non_cyclic_new_edges)
                new_graph.remove_node(node)

        for node in include_nodes:
            if node not in new_graph:
                raise ValueError(
                    "Couldn't find model '{}' -- does it exist or is it disabled?".format(node)
                )

        return Graph(new_graph)

    def subgraph(self, nodes: Iterable[UniqueId]) -> "Graph":
        # Take the original networkx graph and return a subgraph containing only
        # the selected unique_id nodes.
        return Graph(self.graph.subgraph(nodes))

    def get_dependent_nodes(self, node: UniqueId) -> Collection[UniqueId]:
        return nx.descendants(self.graph, node)
