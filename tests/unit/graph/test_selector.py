import string
from argparse import Namespace
from queue import Empty
from typing import List
from unittest.mock import MagicMock

import networkx as nx
import pytest

import dbt.compilation
import dbt.config
import dbt.exceptions
import dbt.graph.cli as graph_cli
import dbt.graph.selector as graph_selector
import dbt.parser
import dbt.parser.manifest
import dbt.utils
import dbt_common.exceptions
from dbt.config.runtime import RuntimeConfig
from dbt.flags import set_from_args
from dbt.graph import NodeSelector, parse_difference
from dbt.node_types import NodeType
from tests.unit.utils.manifest import (
    make_manifest,
    make_model,
    make_seed,
    make_singular_test,
    make_source,
)

set_from_args(Namespace(WARN_ERROR=False), None)


def _get_graph():
    integer_graph = nx.balanced_tree(2, 2, nx.DiGraph())

    package_mapping = {
        i: "m." + ("X" if i % 2 == 0 else "Y") + "." + letter
        for (i, letter) in enumerate(string.ascii_lowercase)
    }

    # Edges: [(X.a, Y.b), (X.a, X.c), (Y.b, Y.d), (Y.b, X.e), (X.c, Y.f), (X.c, X.g)]
    return graph_selector.Graph(nx.relabel_nodes(integer_graph, package_mapping))


def _get_manifest(graph):
    nodes = {}
    for unique_id in graph:
        fqn = unique_id.split(".")
        node = MagicMock(
            unique_id=unique_id,
            fqn=fqn,
            package_name=fqn[0],
            tags=[],
            resource_type=NodeType.Model,
            empty=False,
            config=MagicMock(enabled=True),
            is_versioned=False,
        )
        nodes[unique_id] = node

    nodes["m.X.a"].tags = ["abc"]
    nodes["m.Y.b"].tags = ["abc", "bcef"]
    nodes["m.X.c"].tags = ["abc", "bcef"]
    nodes["m.Y.d"].tags = []
    nodes["m.X.e"].tags = ["efg", "bcef"]
    nodes["m.Y.f"].tags = ["efg", "bcef"]
    nodes["m.X.g"].tags = ["efg"]
    return MagicMock(nodes=nodes)


@pytest.fixture
def graph():
    return _get_graph()


@pytest.fixture
def mock_manifest_with_mock_graph(graph):
    return _get_manifest(graph)


def id_macro(arg):
    if isinstance(arg, str):
        return arg
    try:
        return "_".join(arg)
    except TypeError:
        return arg


run_specs = [
    # include by fqn
    (["X.a"], [], {"m.X.a"}),
    # include by tag
    (["tag:abc"], [], {"m.X.a", "m.Y.b", "m.X.c"}),
    # exclude by tag
    (["*"], ["tag:abc"], {"m.Y.d", "m.X.e", "m.Y.f", "m.X.g"}),
    # tag + fqn
    (["tag:abc", "a"], [], {"m.X.a", "m.Y.b", "m.X.c"}),
    (["tag:abc", "d"], [], {"m.X.a", "m.Y.b", "m.X.c", "m.Y.d"}),
    # multiple node selection across packages
    (["X.a", "b"], [], {"m.X.a", "m.Y.b"}),
    (["X.a+"], ["b"], {"m.X.a", "m.X.c", "m.Y.d", "m.X.e", "m.Y.f", "m.X.g"}),
    # children
    (["X.c+"], [], {"m.X.c", "m.Y.f", "m.X.g"}),
    (["X.a+1"], [], {"m.X.a", "m.Y.b", "m.X.c"}),
    (["X.a+"], ["tag:efg"], {"m.X.a", "m.Y.b", "m.X.c", "m.Y.d"}),
    # parents
    (["+Y.f"], [], {"m.X.c", "m.Y.f", "m.X.a"}),
    (["1+Y.f"], [], {"m.X.c", "m.Y.f"}),
    # childrens parents
    (["@X.c"], [], {"m.X.a", "m.X.c", "m.Y.f", "m.X.g"}),
    # multiple selection/exclusion
    (["tag:abc", "tag:bcef"], [], {"m.X.a", "m.Y.b", "m.X.c", "m.X.e", "m.Y.f"}),
    (["tag:abc", "tag:bcef"], ["tag:efg"], {"m.X.a", "m.Y.b", "m.X.c"}),
    (["tag:abc", "tag:bcef"], ["tag:efg", "a"], {"m.Y.b", "m.X.c"}),
    # intersections
    (["a,a"], [], {"m.X.a"}),
    (["+c,c+"], [], {"m.X.c"}),
    (["a,b"], [], set()),
    (["tag:abc,tag:bcef"], [], {"m.Y.b", "m.X.c"}),
    (["*,tag:abc,a"], [], {"m.X.a"}),
    (["a,tag:abc,*"], [], {"m.X.a"}),
    (["tag:abc,tag:bcef"], ["c"], {"m.Y.b"}),
    (["tag:bcef,tag:efg"], ["tag:bcef,@b"], {"m.Y.f"}),
    (["tag:bcef,tag:efg"], ["tag:bcef,@a"], set()),
    (["*,@a,+b"], ["*,tag:abc,tag:bcef"], {"m.X.a"}),
    (["tag:bcef,tag:efg", "*,tag:abc"], [], {"m.X.a", "m.Y.b", "m.X.c", "m.X.e", "m.Y.f"}),
    (["tag:bcef,tag:efg", "*,tag:abc"], ["e"], {"m.X.a", "m.Y.b", "m.X.c", "m.Y.f"}),
    (["tag:bcef,tag:efg", "*,tag:abc"], ["e"], {"m.X.a", "m.Y.b", "m.X.c", "m.Y.f"}),
    (["tag:bcef,tag:efg", "*,tag:abc"], ["e", "f"], {"m.X.a", "m.Y.b", "m.X.c"}),
    (["tag:bcef,tag:efg", "*,tag:abc"], ["tag:abc,tag:bcef"], {"m.X.a", "m.X.e", "m.Y.f"}),
    (["tag:bcef,tag:efg", "*,tag:abc"], ["tag:abc,tag:bcef", "tag:abc,a"], {"m.X.e", "m.Y.f"}),
]


@pytest.mark.parametrize("include,exclude,expected", run_specs, ids=id_macro)
def test_run_specs(include, exclude, expected, graph, mock_manifest_with_mock_graph):
    selector = graph_selector.NodeSelector(graph, mock_manifest_with_mock_graph)
    spec = graph_cli.parse_difference(include, exclude)
    selected, _ = selector.select_nodes(spec)

    assert selected == expected


param_specs = [
    ("a", False, None, False, None, "fqn", "a", False),
    ("+a", True, None, False, None, "fqn", "a", False),
    ("256+a", True, 256, False, None, "fqn", "a", False),
    ("a+", False, None, True, None, "fqn", "a", False),
    ("a+256", False, None, True, 256, "fqn", "a", False),
    ("+a+", True, None, True, None, "fqn", "a", False),
    ("16+a+32", True, 16, True, 32, "fqn", "a", False),
    ("@a", False, None, False, None, "fqn", "a", True),
    ("a.b", False, None, False, None, "fqn", "a.b", False),
    ("+a.b", True, None, False, None, "fqn", "a.b", False),
    ("256+a.b", True, 256, False, None, "fqn", "a.b", False),
    ("a.b+", False, None, True, None, "fqn", "a.b", False),
    ("a.b+256", False, None, True, 256, "fqn", "a.b", False),
    ("+a.b+", True, None, True, None, "fqn", "a.b", False),
    ("16+a.b+32", True, 16, True, 32, "fqn", "a.b", False),
    ("@a.b", False, None, False, None, "fqn", "a.b", True),
    ("a.b.*", False, None, False, None, "fqn", "a.b.*", False),
    ("+a.b.*", True, None, False, None, "fqn", "a.b.*", False),
    ("256+a.b.*", True, 256, False, None, "fqn", "a.b.*", False),
    ("a.b.*+", False, None, True, None, "fqn", "a.b.*", False),
    ("a.b.*+256", False, None, True, 256, "fqn", "a.b.*", False),
    ("+a.b.*+", True, None, True, None, "fqn", "a.b.*", False),
    ("16+a.b.*+32", True, 16, True, 32, "fqn", "a.b.*", False),
    ("@a.b.*", False, None, False, None, "fqn", "a.b.*", True),
    ("tag:a", False, None, False, None, "tag", "a", False),
    ("+tag:a", True, None, False, None, "tag", "a", False),
    ("256+tag:a", True, 256, False, None, "tag", "a", False),
    ("tag:a+", False, None, True, None, "tag", "a", False),
    ("tag:a+256", False, None, True, 256, "tag", "a", False),
    ("+tag:a+", True, None, True, None, "tag", "a", False),
    ("16+tag:a+32", True, 16, True, 32, "tag", "a", False),
    ("@tag:a", False, None, False, None, "tag", "a", True),
    ("source:a", False, None, False, None, "source", "a", False),
    ("source:a+", False, None, True, None, "source", "a", False),
    ("source:a+1", False, None, True, 1, "source", "a", False),
    ("source:a+32", False, None, True, 32, "source", "a", False),
    ("@source:a", False, None, False, None, "source", "a", True),
]


@pytest.mark.parametrize(
    "spec,parents,parents_depth,children,children_depth,filter_type,filter_value,childrens_parents",
    param_specs,
    ids=id_macro,
)
def test_parse_specs(
    spec,
    parents,
    parents_depth,
    children,
    children_depth,
    filter_type,
    filter_value,
    childrens_parents,
):
    parsed = graph_selector.SelectionCriteria.from_single_spec(spec)
    assert parsed.parents == parents
    assert parsed.parents_depth == parents_depth
    assert parsed.children == children
    assert parsed.children_depth == children_depth
    assert parsed.method == filter_type
    assert parsed.value == filter_value
    assert parsed.childrens_parents == childrens_parents


invalid_specs = [
    "@a+",
    "@a.b+",
    "@a.b*+",
    "@tag:a+",
    "@source:a+",
]


@pytest.mark.parametrize("invalid", invalid_specs, ids=lambda k: str(k))
def test_invalid_specs(invalid):
    with pytest.raises(dbt_common.exceptions.DbtRuntimeError):
        graph_selector.SelectionCriteria.from_single_spec(invalid)


class TestCompiler:
    def test_single_model(self, runtime_config: RuntimeConfig):
        model = make_model(pkg="pkg", name="model_one", code="SELECT * FROM events")
        manifest = make_manifest(nodes=[model])

        compiler = dbt.compilation.Compiler(config=runtime_config)
        linker = compiler.compile(manifest)

        assert linker.nodes() == {model.unique_id}
        assert linker.edges() == set()

    def test_two_models_simple_ref(self, runtime_config: RuntimeConfig):
        model_one = make_model(pkg="pkg", name="model_one", code="SELECT * FROM events")
        model_two = make_model(
            pkg="pkg",
            name="model_two",
            code="SELECT * FROM {{ref('model_one')}}",
            refs=[model_one],
        )
        models = [model_one, model_two]
        manifest = make_manifest(nodes=models)

        compiler = dbt.compilation.Compiler(config=runtime_config)
        linker = compiler.compile(manifest)

        expected_nodes: List[str] = [model.unique_id for model in models]
        assert linker.nodes() == set(expected_nodes)
        assert list(linker.edges()) == [tuple(expected_nodes)]


class TestNodeSelector:
    def test_dependency_list(self, runtime_config: RuntimeConfig):
        model_one = make_model(pkg="pkg", name="model_one", code="SELECT * FROM events")
        model_two = make_model(
            pkg="pkg",
            name="model_two",
            code="SELECT * FROM {{ref('model_one')}}",
            refs=[model_one],
        )
        model_three = make_model(
            pkg="pkg",
            name="model_three",
            code="""
                SELECT * FROM {{ ref("model_1") }}
                union all
                SELECT * FROM {{ ref("model_2") }}
            """,
            refs=[model_one, model_two],
        )
        model_four = make_model(
            pkg="pkg",
            name="model_four",
            code="SELECT * FROM {{ref('model_three')}}",
            refs=[model_three],
        )
        models = [model_one, model_two, model_three, model_four]
        manifest = make_manifest(nodes=models)

        # Get the graph
        compiler = dbt.compilation.Compiler(runtime_config)
        graph = compiler.compile(manifest)

        # Create the selector and get the queue
        selector = NodeSelector(graph, manifest)
        queue = selector.get_graph_queue(
            parse_difference(
                None,
                None,
            )
        )

        for model in models:
            assert not queue.empty()
            got = queue.get(block=False)
            assert got.unique_id == model.unique_id
            with pytest.raises(Empty):
                queue.get(block=False)
            queue.mark_done(got.unique_id)
        assert queue.empty()

    def test_select_downstream_of_empty_model(self, runtime_config: RuntimeConfig):
        # empty model
        model_one = make_model(pkg="other", name="model_one", code="")
        # non-empty model
        model_two = make_model(
            pkg="pkg",
            name="model_two",
            code="""select * from {{ref('model_one')}}""",
            refs=[model_one],
        )
        models = [model_one, model_two]
        manifest = make_manifest(nodes=models)

        # Get the graph
        compiler = dbt.compilation.Compiler(runtime_config)
        graph = compiler.compile(manifest)

        # Ensure that model_two is selected as downstream of model_one
        selector = NodeSelector(graph, manifest)
        spec = graph_selector.SelectionCriteria.from_single_spec("model_one+")
        assert selector.get_selected(spec) == {"model.pkg.model_two"}

        # Ensure that --indirect-selection empty returns the same result
        spec.indirect_selection = graph_selector.IndirectSelection.Empty
        assert selector.get_selected(spec) == {"model.pkg.model_two"}

    @pytest.mark.parametrize(
        "select,expected",
        [
            # a source test that reads a seed
            ("source:raw.events", {"source.pkg.raw.events", "test.pkg.events_code_in_seed"}),
            # a model test that reads a seed the model does not read, next to the same
            # shape reading a source; a test that reads an unselected model stays out
            (
                "stg_events",
                {
                    "model.pkg.stg_events",
                    "test.pkg.stg_events_code_in_seed",
                    "test.pkg.stg_events_code_in_source",
                },
            ),
        ],
    )
    def test_buildable_counts_unselected_seed_as_present(
        self, runtime_config: RuntimeConfig, select, expected
    ):
        events = make_source(pkg="pkg", source_name="raw", table_name="events")
        lookup = make_source(pkg="pkg", source_name="raw", table_name="lookup")
        allowed_codes = make_seed(pkg="pkg", name="allowed_codes")
        stg_events = make_model(
            pkg="pkg",
            name="stg_events",
            code="select code from {{ source('raw', 'events') }}",
            sources=[events],
        )
        other_model = make_model(pkg="pkg", name="other_model", code="select 'A' as code")
        data_tests = [
            make_singular_test(
                pkg="pkg",
                name="events_code_in_seed",
                sql="select 1",
                refs=[allowed_codes],
                sources=[events],
            ),
            make_singular_test(
                pkg="pkg",
                name="stg_events_code_in_seed",
                sql="select 1",
                refs=[stg_events, allowed_codes],
            ),
            make_singular_test(
                pkg="pkg",
                name="stg_events_code_in_source",
                sql="select 1",
                refs=[stg_events],
                sources=[lookup],
            ),
            make_singular_test(
                pkg="pkg",
                name="stg_events_code_in_model",
                sql="select 1",
                refs=[stg_events, other_model],
            ),
        ]
        manifest = make_manifest(
            nodes=[allowed_codes, stg_events, other_model, *data_tests],
            sources=[events, lookup],
        )
        graph = dbt.compilation.Compiler(runtime_config).compile(manifest)
        selector = NodeSelector(graph, manifest)

        spec = graph_selector.SelectionCriteria.from_single_spec(select)
        spec.indirect_selection = graph_selector.IndirectSelection.Buildable
        assert selector.get_selected(spec) == expected
