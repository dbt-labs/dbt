import json

import networkx as nx  # type: ignore
import os
import pickle

from collections import defaultdict
from typing import List, Dict, Any, Tuple, Optional

from dbt.common.invocation import get_invocation_id
from dbt.flags import get_flags
from dbt.clients.adapter import get_adapter
from dbt.clients import jinja
from dbt.clients.system import make_directory
from dbt.context.providers import generate_runtime_model_context
from dbt.contracts.graph.manifest import Manifest, UniqueID
from dbt.contracts.graph.nodes import (
    ManifestNode,
    ManifestSQLNode,
    GenericTestNode,
    GraphMemberNode,
    InjectedCTE,
    SeedNode,
)
from dbt.exceptions import (
    GraphDependencyNotFoundError,
    DbtInternalError,
    DbtRuntimeError,
)
from dbt.graph import Graph
from dbt.common.events.functions import fire_event
from dbt.common.events.types import FoundStats, Note, WritingInjectedSQLForNode
from dbt.common.events.contextvars import get_node_info
from dbt.node_types import NodeType, ModelLanguage
from dbt.common.events.format import pluralize
import dbt.tracking
import sqlparse

graph_file_name = "graph.gpickle"


def print_compile_stats(stats):
    names = {
        NodeType.Model: "model",
        NodeType.Test: "test",
        NodeType.Snapshot: "snapshot",
        NodeType.Analysis: "analysis",
        NodeType.Macro: "macro",
        NodeType.Operation: "operation",
        NodeType.Seed: "seed",
        NodeType.Source: "source",
        NodeType.Exposure: "exposure",
        NodeType.SemanticModel: "semantic model",
        NodeType.Metric: "metric",
        NodeType.Group: "group",
    }

    results = {k: 0 for k in names.keys()}
    results.update(stats)

    # create tracking event for resource_counts
    if dbt.tracking.active_user is not None:
        resource_counts = {k.pluralize(): v for k, v in results.items()}
        dbt.tracking.track_resource_counts(resource_counts)

    # do not include resource types that are not actually defined in the project
    stat_line = ", ".join([pluralize(ct, names.get(t)) for t, ct in stats.items() if t in names])

    fire_event(FoundStats(stat_line=stat_line))


def _node_enabled(node: ManifestNode):
    # Disabled models are already excluded from the manifest
    if node.resource_type == NodeType.Test and not node.config.enabled:
        return False
    else:
        return True


def _generate_stats(manifest: Manifest):
    stats: Dict[NodeType, int] = defaultdict(int)
    for node in manifest.nodes.values():
        if _node_enabled(node):
            stats[node.resource_type] += 1

    # Disabled nodes don't appear in the following collections, so we don't check.
    stats[NodeType.Source] += len(manifest.sources)
    stats[NodeType.Exposure] += len(manifest.exposures)
    stats[NodeType.Metric] += len(manifest.metrics)
    stats[NodeType.Macro] += len(manifest.macros)
    stats[NodeType.Group] += len(manifest.groups)
    stats[NodeType.SemanticModel] += len(manifest.semantic_models)

    # TODO: should we be counting dimensions + entities?

    return stats


def _add_prepended_cte(prepended_ctes, new_cte):
    for cte in prepended_ctes:
        if cte.id == new_cte.id and new_cte.sql:
            cte.sql = new_cte.sql
            return
    if new_cte.sql:
        prepended_ctes.append(new_cte)


def _extend_prepended_ctes(prepended_ctes, new_prepended_ctes):
    for new_cte in new_prepended_ctes:
        _add_prepended_cte(prepended_ctes, new_cte)


def _get_tests_for_node(manifest: Manifest, unique_id: UniqueID) -> List[UniqueID]:
    """Get a list of tests that depend on the node with the
    provided unique id"""

    tests = []
    if unique_id in manifest.child_map:
        for child_unique_id in manifest.child_map[unique_id]:
            if child_unique_id.startswith("test."):
                tests.append(child_unique_id)

    return tests


class Linker:
    def __init__(self, data=None) -> None:
        if data is None:
            data = {}
        self.graph = nx.DiGraph(**data)

    def edges(self):
        return self.graph.edges()

    def nodes(self):
        return self.graph.nodes()

    def find_cycles(self):
        try:
            cycle = nx.find_cycle(self.graph)
        except nx.NetworkXNoCycle:
            return None
        else:
            # cycles is a List[Tuple[str, ...]]
            return " --> ".join(c[0] for c in cycle)

    def dependency(self, node1, node2):
        "indicate that node1 depends on node2"
        self.graph.add_node(node1)
        self.graph.add_node(node2)
        self.graph.add_edge(node2, node1)

    def add_node(self, node):
        self.graph.add_node(node)

    def write_graph(self, outfile: str, manifest: Manifest):
        """Write the graph to a gpickle file. Before doing so, serialize and
        include all nodes in their corresponding graph entries.
        """
        out_graph = self.graph.copy()
        for node_id in self.graph:
            data = manifest.expect(node_id).to_dict(omit_none=True)
            out_graph.add_node(node_id, **data)
        with open(outfile, "wb") as outfh:
            pickle.dump(out_graph, outfh, protocol=pickle.HIGHEST_PROTOCOL)

    def link_node(self, node: GraphMemberNode, manifest: Manifest):
        self.add_node(node.unique_id)

        for dependency in node.depends_on_nodes:
            if dependency in manifest.nodes:
                self.dependency(node.unique_id, (manifest.nodes[dependency].unique_id))
            elif dependency in manifest.sources:
                self.dependency(node.unique_id, (manifest.sources[dependency].unique_id))
            elif dependency in manifest.metrics:
                self.dependency(node.unique_id, (manifest.metrics[dependency].unique_id))
            elif dependency in manifest.semantic_models:
                self.dependency(node.unique_id, (manifest.semantic_models[dependency].unique_id))
            else:
                raise GraphDependencyNotFoundError(node, dependency)

    def link_graph(self, manifest: Manifest):
        for source in manifest.sources.values():
            self.add_node(source.unique_id)
        for node in manifest.nodes.values():
            self.link_node(node, manifest)
        for semantic_model in manifest.semantic_models.values():
            self.link_node(semantic_model, manifest)
        for exposure in manifest.exposures.values():
            self.link_node(exposure, manifest)
        for metric in manifest.metrics.values():
            self.link_node(metric, manifest)
        for saved_query in manifest.saved_queries.values():
            self.link_node(saved_query, manifest)

        cycle = self.find_cycles()

        if cycle:
            raise RuntimeError("Found a cycle: {}".format(cycle))

    def add_test_edges(self, manifest: Manifest) -> None:
        """This method adds additional edges to the DAG. For a given non-test
        executable node, add an edge from an upstream test to the given node if
        the set of nodes the test depends on is a subset of the upstream nodes
        for the given node."""

        # Given a graph:
        # model1 --> model2 --> model3
        #   |             |
        #   |            \/
        #  \/          test 2
        # test1
        #
        # Produce the following graph:
        # model1 --> model2 --> model3
        #   |       /\    |      /\ /\
        #   |       |    \/      |  |
        #  \/       |  test2 ----|  |
        # test1 ----|---------------|

        for node_id in self.graph:
            # If node is executable (in manifest.nodes) and does _not_
            # represent a test, continue.
            if (
                node_id in manifest.nodes
                and manifest.nodes[node_id].resource_type != NodeType.Test
            ):
                # Get *everything* upstream of the node
                all_upstream_nodes = nx.traversal.bfs_tree(self.graph, node_id, reverse=True)
                # Get the set of upstream nodes not including the current node.
                upstream_nodes = set([n for n in all_upstream_nodes if n != node_id])

                # Get all tests that depend on any upstream nodes.
                upstream_tests = []
                for upstream_node in upstream_nodes:
                    upstream_tests += _get_tests_for_node(manifest, upstream_node)

                for upstream_test in upstream_tests:
                    # Get the set of all nodes that the test depends on
                    # including the upstream_node itself. This is necessary
                    # because tests can depend on multiple nodes (ex:
                    # relationship tests). Test nodes do not distinguish
                    # between what node the test is "testing" and what
                    # node(s) it depends on.
                    test_depends_on = set(manifest.nodes[upstream_test].depends_on_nodes)

                    # If the set of nodes that an upstream test depends on
                    # is a subset of all upstream nodes of the current node,
                    # add an edge from the upstream test to the current node.
                    if test_depends_on.issubset(upstream_nodes):
                        self.graph.add_edge(upstream_test, node_id, edge_type="parent_test")

    def get_graph(self, manifest: Manifest) -> Graph:
        self.link_graph(manifest)
        return Graph(self.graph)

    def get_graph_summary(self, manifest: Manifest) -> Dict[int, Dict[str, Any]]:
        """Create a smaller summary of the graph, suitable for basic diagnostics
        and performance tuning. The summary includes only the edge structure,
        node types, and node names. Each of the n nodes is assigned an integer
        index 0, 1, 2,..., n-1 for compactness"""
        graph_nodes = dict()
        index_dict = dict()
        for node_index, node_name in enumerate(self.graph):
            index_dict[node_name] = node_index
            data = manifest.expect(node_name).to_dict(omit_none=True)
            graph_nodes[node_index] = {"name": node_name, "type": data["resource_type"]}

        for node_index, node in graph_nodes.items():
            successors = [index_dict[n] for n in self.graph.successors(node["name"])]
            if successors:
                node["succ"] = [index_dict[n] for n in self.graph.successors(node["name"])]

        return graph_nodes


class Compiler:
    def __init__(self, config) -> None:
        self.config = config

    def initialize(self):
        make_directory(self.config.project_target_path)
        make_directory(self.config.packages_install_path)

    # creates a ModelContext which is converted to
    # a dict for jinja rendering of SQL
    def _create_node_context(
        self,
        node: ManifestSQLNode,
        manifest: Manifest,
        extra_context: Dict[str, Any],
    ) -> Dict[str, Any]:

        context = generate_runtime_model_context(node, self.config, manifest)
        context.update(extra_context)

        if isinstance(node, GenericTestNode):
            # for test nodes, add a special keyword args value to the context
            jinja.add_rendered_test_kwargs(context, node)

        return context

    def add_ephemeral_prefix(self, name: str):
        adapter = get_adapter(self.config)
        relation_cls = adapter.Relation
        return relation_cls.add_ephemeral_prefix(name)

    def _recursively_prepend_ctes(
        self,
        model: ManifestSQLNode,
        manifest: Manifest,
        extra_context: Optional[Dict[str, Any]],
    ) -> Tuple[ManifestSQLNode, List[InjectedCTE]]:
        """This method is called by the 'compile_node' method. Starting
        from the node that it is passed in, it will recursively call
        itself using the 'extra_ctes'.  The 'ephemeral' models do
        not produce SQL that is executed directly, instead they
        are rolled up into the models that refer to them by
        inserting CTEs into the SQL.
        """
        if model.compiled_code is None:
            raise DbtRuntimeError("Cannot inject ctes into an uncompiled node", model)

        # tech debt: safe flag/arg access (#6259)
        if not getattr(self.config.args, "inject_ephemeral_ctes", True):
            return (model, [])

        # extra_ctes_injected flag says that we've already recursively injected the ctes
        if model.extra_ctes_injected:
            return (model, model.extra_ctes)

        # Just to make it plain that nothing is actually injected for this case
        if len(model.extra_ctes) == 0:
            # SeedNodes don't have compilation attributes
            if not isinstance(model, SeedNode):
                model.extra_ctes_injected = True
            return (model, [])

        # This stores the ctes which will all be recursively
        # gathered and then "injected" into the model.
        prepended_ctes: List[InjectedCTE] = []

        # extra_ctes are added to the model by
        # RuntimeRefResolver.create_relation, which adds an
        # extra_cte for every model relation which is an
        # ephemeral model. InjectedCTEs have a unique_id and sql.
        # extra_ctes start out with sql set to None, and the sql is set in this loop.
        for cte in model.extra_ctes:
            if cte.id not in manifest.nodes:
                raise DbtInternalError(
                    f"During compilation, found a cte reference that "
                    f"could not be resolved: {cte.id}"
                )
            cte_model = manifest.nodes[cte.id]
            assert not isinstance(cte_model, SeedNode)

            if not cte_model.is_ephemeral_model:
                raise DbtInternalError(f"{cte.id} is not ephemeral")

            # This model has already been compiled and extra_ctes_injected, so it's been
            # through here before. We already checked above for extra_ctes_injected, but
            # checking again because updates maybe have happened in another thread.
            if cte_model.compiled is True and cte_model.extra_ctes_injected is True:
                new_prepended_ctes = cte_model.extra_ctes

            # if the cte_model isn't compiled, i.e. first time here
            else:
                # This is an ephemeral parsed model that we can compile.
                # Render the raw_code and set compiled to True
                cte_model = self._compile_code(cte_model, manifest, extra_context)
                # recursively call this method, sets extra_ctes_injected to True
                cte_model, new_prepended_ctes = self._recursively_prepend_ctes(
                    cte_model, manifest, extra_context
                )
                # Write compiled SQL file
                self._write_node(cte_model)

            _extend_prepended_ctes(prepended_ctes, new_prepended_ctes)

            new_cte_name = self.add_ephemeral_prefix(cte_model.name)
            rendered_sql = cte_model._pre_injected_sql or cte_model.compiled_code
            sql = f" {new_cte_name} as (\n{rendered_sql}\n)"

            _add_prepended_cte(prepended_ctes, InjectedCTE(id=cte.id, sql=sql))

        # Check again before updating for multi-threading
        if not model.extra_ctes_injected:
            injected_sql = inject_ctes_into_sql(
                model.compiled_code,
                prepended_ctes,
            )
            model.extra_ctes_injected = True
            model._pre_injected_sql = model.compiled_code
            model.compiled_code = injected_sql
            model.extra_ctes = prepended_ctes

        # if model.extra_ctes is not set to prepended ctes, something went wrong
        return model, model.extra_ctes

    # Sets compiled_code and compiled flag in the ManifestSQLNode passed in,
    # creates a "context" dictionary for jinja rendering,
    # and then renders the "compiled_code" using the node, the
    # raw_code and the context.
    def _compile_code(
        self,
        node: ManifestSQLNode,
        manifest: Manifest,
        extra_context: Optional[Dict[str, Any]] = None,
    ) -> ManifestSQLNode:
        if extra_context is None:
            extra_context = {}

        if node.language == ModelLanguage.python:
            context = self._create_node_context(node, manifest, extra_context)

            postfix = jinja.get_rendered(
                "{{ py_script_postfix(model) }}",
                context,
                node,
            )
            # we should NOT jinja render the python model's 'raw code'
            node.compiled_code = f"{node.raw_code}\n\n{postfix}"

        else:
            context = self._create_node_context(node, manifest, extra_context)
            node.compiled_code = jinja.get_rendered(
                node.raw_code,
                context,
                node,
            )

        node.compiled = True

        # relation_name is set at parse time, except for tests without store_failures,
        # but cli param can turn on store_failures, so we set here.
        if (
            node.resource_type == NodeType.Test
            and node.relation_name is None
            and node.is_relational
        ):
            adapter = get_adapter(self.config)
            relation_cls = adapter.Relation
            relation_name = str(relation_cls.create_from(self.config, node))
            node.relation_name = relation_name

        return node

    # This method doesn't actually "compile" any of the nodes. That is done by the
    # "compile_node" method. This creates a Linker and builds the networkx graph,
    # writes out the graph.gpickle file, and prints the stats, returning a Graph object.
    def compile(self, manifest: Manifest, write=True, add_test_edges=False) -> Graph:
        self.initialize()
        linker = Linker()
        linker.link_graph(manifest)

        # Create a file containing basic information about graph structure,
        # supporting diagnostics and performance analysis.
        summaries: Dict = dict()
        summaries["_invocation_id"] = get_invocation_id()
        summaries["linked"] = linker.get_graph_summary(manifest)

        if add_test_edges:
            manifest.build_parent_and_child_maps()
            linker.add_test_edges(manifest)

            # Create another diagnostic summary, just as above, but this time
            # including the test edges.
            summaries["with_test_edges"] = linker.get_graph_summary(manifest)

        with open(
            os.path.join(self.config.project_target_path, "graph_summary.json"), "w"
        ) as out_stream:
            try:
                out_stream.write(json.dumps(summaries))
            except Exception as e:  # This is non-essential information, so merely note failures.
                fire_event(
                    Note(
                        msg=f"An error was encountered writing the graph summary information: {e}"
                    )
                )

        stats = _generate_stats(manifest)

        if write:
            self.write_graph_file(linker, manifest)

        # Do not print these for list command
        if self.config.args.which != "list":
            stats = _generate_stats(manifest)
            print_compile_stats(stats)

        return Graph(linker.graph)

    def write_graph_file(self, linker: Linker, manifest: Manifest):
        filename = graph_file_name
        graph_path = os.path.join(self.config.project_target_path, filename)
        flags = get_flags()
        if flags.WRITE_JSON:
            linker.write_graph(graph_path, manifest)

    # writes the "compiled_code" into the target/compiled directory
    def _write_node(self, node: ManifestSQLNode) -> ManifestSQLNode:
        if not node.extra_ctes_injected or node.resource_type in (
            NodeType.Snapshot,
            NodeType.Seed,
        ):
            return node
        fire_event(WritingInjectedSQLForNode(node_info=get_node_info()))

        if node.compiled_code:
            node.compiled_path = node.get_target_write_path(self.config.target_path, "compiled")
            node.write_node(self.config.project_root, node.compiled_path, node.compiled_code)
        return node

    def compile_node(
        self,
        node: ManifestSQLNode,
        manifest: Manifest,
        extra_context: Optional[Dict[str, Any]] = None,
        write: bool = True,
    ) -> ManifestSQLNode:
        """This is the main entry point into this code. It's called by
        CompileRunner.compile, GenericRPCRunner.compile, and
        RunTask.get_hook_sql. It calls '_compile_code' to render
        the node's raw_code into compiled_code, and then calls the
        recursive method to "prepend" the ctes.
        """
        # Make sure Lexer for sqlparse 0.4.4 is initialized
        from sqlparse.lexer import Lexer  # type: ignore

        if hasattr(Lexer, "get_default_instance"):
            Lexer.get_default_instance()

        node = self._compile_code(node, manifest, extra_context)

        node, _ = self._recursively_prepend_ctes(node, manifest, extra_context)
        if write:
            self._write_node(node)
        return node


def inject_ctes_into_sql(sql: str, ctes: List[InjectedCTE]) -> str:
    """
    `ctes` is a list of InjectedCTEs like:

        [
            InjectedCTE(
                id="cte_id_1",
                sql="__dbt__cte__ephemeral as (select * from table)",
            ),
            InjectedCTE(
                id="cte_id_2",
                sql="__dbt__cte__events as (select id, type from events)",
            ),
        ]

    Given `sql` like:

      "with internal_cte as (select * from sessions)
       select * from internal_cte"

    This will spit out:

      "with __dbt__cte__ephemeral as (select * from table),
            __dbt__cte__events as (select id, type from events),
            internal_cte as (select * from sessions)
       select * from internal_cte"

    (Whitespace enhanced for readability.)
    """
    if len(ctes) == 0:
        return sql

    parsed_stmts = sqlparse.parse(sql)
    parsed = parsed_stmts[0]

    with_stmt = None
    for token in parsed.tokens:
        if token.is_keyword and token.normalized == "WITH":
            with_stmt = token
        elif token.is_keyword and token.normalized == "RECURSIVE" and with_stmt is not None:
            with_stmt = token
            break
        elif not token.is_whitespace and with_stmt is not None:
            break

    if with_stmt is None:
        # no with stmt, add one, and inject CTEs right at the beginning
        # [original_sql]
        first_token = parsed.token_first()
        with_token = sqlparse.sql.Token(sqlparse.tokens.Keyword, "with")
        parsed.insert_before(first_token, with_token)
        # [with][original_sql]
        injected_ctes = ", ".join(c.sql for c in ctes) + " "
        injected_ctes_token = sqlparse.sql.Token(sqlparse.tokens.Keyword, injected_ctes)
        parsed.insert_after(with_token, injected_ctes_token)
        # [with][joined_ctes][original_sql]
    else:
        # with stmt exists so we don't need to add one, but we do need to add a comma
        # between the injected ctes and the original sql
        # [with][original_sql]
        injected_ctes = ", ".join(c.sql for c in ctes)
        injected_ctes_token = sqlparse.sql.Token(sqlparse.tokens.Keyword, injected_ctes)
        parsed.insert_after(with_stmt, injected_ctes_token)
        # [with][joined_ctes][original_sql]
        comma_token = sqlparse.sql.Token(sqlparse.tokens.Punctuation, ", ")
        parsed.insert_after(injected_ctes_token, comma_token)
        # [with][joined_ctes][, ][original_sql]

    return str(parsed)
