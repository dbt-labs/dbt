import string
from unittest import mock

import networkx as nx
import pytest

import dbt.graph.cli as graph_cli
import dbt.graph.selector as graph_selector
import dbt_common.exceptions
from dbt.node_types import NodeType


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
        node = mock.MagicMock(
            unique_id=unique_id,
            fqn=fqn,
            package_name=fqn[0],
            tags=[],
            resource_type=NodeType.Model,
            empty=False,
            config=mock.MagicMock(enabled=True),
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
    return mock.MagicMock(nodes=nodes)


@pytest.fixture
def graph():
    return _get_graph()


@pytest.fixture
def manifest(graph):
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
    (["1@X.c"], [], {"m.X.a", "m.X.c", "m.Y.f", "m.X.g"}),
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
def test_run_specs(include, exclude, expected, graph, manifest):
    selector = graph_selector.NodeSelector(graph, manifest)
    spec = graph_cli.parse_difference(include, exclude)
    selected, _ = selector.select_nodes(spec)

    assert selected == expected


param_specs = [
    ("a", False, None, False, None, "fqn", "a", False, None),
    ("+a", True, None, False, None, "fqn", "a", False, None),
    ("256+a", True, 256, False, None, "fqn", "a", False, None),
    ("a+", False, None, True, None, "fqn", "a", False, None),
    ("a+256", False, None, True, 256, "fqn", "a", False, None),
    ("+a+", True, None, True, None, "fqn", "a", False, None),
    ("16+a+32", True, 16, True, 32, "fqn", "a", False, None),
    ("@a", False, None, False, None, "fqn", "a", True, None),
    ("20@a", False, None, False, None, "fqn", "a", True, 20),
    ("a.b", False, None, False, None, "fqn", "a.b", False, None),
    ("+a.b", True, None, False, None, "fqn", "a.b", False, None),
    ("256+a.b", True, 256, False, None, "fqn", "a.b", False, None),
    ("a.b+", False, None, True, None, "fqn", "a.b", False, None),
    ("a.b+256", False, None, True, 256, "fqn", "a.b", False, None),
    ("+a.b+", True, None, True, None, "fqn", "a.b", False, None),
    ("16+a.b+32", True, 16, True, 32, "fqn", "a.b", False, None),
    ("@a.b", False, None, False, None, "fqn", "a.b", True, None),
    ("5@a.b", False, None, False, None, "fqn", "a.b", True, 5),
    ("a.b.*", False, None, False, None, "fqn", "a.b.*", False, None),
    ("+a.b.*", True, None, False, None, "fqn", "a.b.*", False, None),
    ("256+a.b.*", True, 256, False, None, "fqn", "a.b.*", False, None),
    ("a.b.*+", False, None, True, None, "fqn", "a.b.*", False, None),
    ("a.b.*+256", False, None, True, 256, "fqn", "a.b.*", False, None),
    ("+a.b.*+", True, None, True, None, "fqn", "a.b.*", False, None),
    ("16+a.b.*+32", True, 16, True, 32, "fqn", "a.b.*", False, None),
    ("@a.b.*", False, None, False, None, "fqn", "a.b.*", True, None),
    ("12@a.b.*", False, None, False, None, "fqn", "a.b.*", True, 12),
    ("tag:a", False, None, False, None, "tag", "a", False, None),
    ("+tag:a", True, None, False, None, "tag", "a", False, None),
    ("256+tag:a", True, 256, False, None, "tag", "a", False, None),
    ("tag:a+", False, None, True, None, "tag", "a", False, None),
    ("tag:a+256", False, None, True, 256, "tag", "a", False, None),
    ("+tag:a+", True, None, True, None, "tag", "a", False, None),
    ("16+tag:a+32", True, 16, True, 32, "tag", "a", False, None),
    ("3@tag:a", False, None, False, None, "tag", "a", True, 3),
    ("source:a", False, None, False, None, "source", "a", False, None),
    ("source:a+", False, None, True, None, "source", "a", False, None),
    ("source:a+1", False, None, True, 1, "source", "a", False, None),
    ("source:a+32", False, None, True, 32, "source", "a", False, None),
    ("1@source:a", False, None, False, None, "source", "a", True, 1),
]


@pytest.mark.parametrize(
    "spec,parents,parents_depth,children,children_depth,filter_type,filter_value,childrens_parents,childrens_parents_depth",
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
    childrens_parents_depth,
):
    parsed = graph_selector.SelectionCriteria.from_single_spec(spec)
    assert parsed.parents == parents
    assert parsed.parents_depth == parents_depth
    assert parsed.children == children
    assert parsed.children_depth == children_depth
    assert parsed.method == filter_type
    assert parsed.value == filter_value
    assert parsed.childrens_parents == childrens_parents
    assert parsed.childrens_parents_depth == childrens_parents_depth


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
