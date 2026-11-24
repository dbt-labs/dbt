import os
import tempfile
from queue import Empty
from unittest import mock

import pytest

from dbt.compilation import Graph, Linker
from dbt.graph.cli import parse_difference
from dbt.graph.queue import GraphQueue
from dbt.graph.selector import NodeSelector
from tests.unit.utils.manifest import (
    make_manifest,
    make_model,
    make_singular_test,
    make_source,
)


def _mock_manifest(nodes):
    config = mock.MagicMock(enabled=True)
    manifest = mock.MagicMock(
        nodes={
            n: mock.MagicMock(
                unique_id=n,
                package_name="pkg",
                name=n,
                empty=False,
                config=config,
                fqn=["pkg", n],
                is_versioned=False,
            )
            for n in nodes
        }
    )
    manifest.expect.side_effect = lambda n: mock.MagicMock(unique_id=n)
    return manifest


class TestLinker:
    @pytest.fixture
    def linker(self) -> Linker:
        return Linker()

    def test_linker_add_node(self, linker: Linker) -> None:
        expected_nodes = ["A", "B", "C"]
        for node in expected_nodes:
            linker.add_node(node)

        actual_nodes = linker.nodes()
        for node in expected_nodes:
            assert node in actual_nodes

        assert len(actual_nodes) == len(expected_nodes)

    def test_fast_test_edges_traverse_sources(self, linker: Linker) -> None:
        source = make_source("pkg", "raw", "items")
        left = make_model(
            "pkg", "left", "select id from {{ source('raw', 'items') }}", sources=[source]
        )
        right = make_model(
            "pkg", "right", "select id from {{ source('raw', 'items') }}", sources=[source]
        )
        downstream = make_model(
            "pkg",
            "downstream",
            "select id from {{ ref('left') }} join {{ ref('right') }} using (id)",
            refs=[left, right],
        )
        multi_dependency_test = make_singular_test(
            "pkg",
            "compare_items",
            "select id from {{ ref('left') }} except select id from {{ ref('right') }}",
            refs=[left, right],
        )
        manifest = make_manifest(
            nodes=[left, right, downstream, multi_dependency_test], sources=[source]
        )

        linker.link_graph(manifest)
        linker.add_test_edges_2(manifest)

        assert linker.graph.get_edge_data(
            multi_dependency_test.unique_id, downstream.unique_id
        ) == {"edge_type": "parent_test"}

    def test_linker_write_graph(self, linker: Linker) -> None:
        expected_nodes = ["A", "B", "C"]
        for node in expected_nodes:
            linker.add_node(node)

        manifest = _mock_manifest("ABC")
        (fd, fname) = tempfile.mkstemp()
        os.close(fd)
        try:
            linker.write_graph(fname, manifest)
            assert os.path.exists(fname)
        finally:
            os.unlink(fname)

    def assert_would_join(self, queue: GraphQueue) -> None:
        """test join() without timeout risk"""
        assert queue.inner.unfinished_tasks == 0

    def _get_graph_queue(
        self,
        manifest,
        linker: Linker,
        include=None,
        exclude=None,
    ) -> GraphQueue:
        graph = Graph(linker.graph)
        selector = NodeSelector(graph, manifest)
        # TODO:  The "eager" string below needs to be replaced with programatic access
        #  to the default value for the indirect selection parameter in
        # dbt.cli.params.indirect_selection
        #
        # Doing that is actually a little tricky, so I'm punting it to a new ticket GH #6397
        spec = parse_difference(include, exclude)
        return selector.get_graph_queue(spec)

    def test_linker_add_dependency(self, linker: Linker) -> None:
        actual_deps = [("A", "B"), ("A", "C"), ("B", "C")]

        for l, r in actual_deps:
            linker.dependency(l, r)

        queue = self._get_graph_queue(_mock_manifest("ABC"), linker)

        got = queue.get(block=False)
        assert got.unique_id == "C"
        with pytest.raises(Empty):
            queue.get(block=False)
        assert not queue.empty()
        queue.mark_done("C")
        assert not queue.empty()

        got = queue.get(block=False)
        assert got.unique_id == "B"
        with pytest.raises(Empty):
            queue.get(block=False)
        assert not queue.empty()
        queue.mark_done("B")
        assert not queue.empty()

        got = queue.get(block=False)
        assert got.unique_id == "A"
        with pytest.raises(Empty):
            queue.get(block=False)
        assert queue.empty()
        queue.mark_done("A")
        self.assert_would_join(queue)
        assert queue.empty()

    def test_linker_add_disjoint_dependencies(self, linker: Linker) -> None:
        actual_deps = [("A", "B")]
        additional_node = "Z"

        for l, r in actual_deps:
            linker.dependency(l, r)
        linker.add_node(additional_node)

        queue = self._get_graph_queue(_mock_manifest("ABCZ"), linker)
        # the first one we get must be B, it has the longest dep chain
        first = queue.get(block=False)
        assert first.unique_id == "B"
        assert not queue.empty()
        queue.mark_done("B")
        assert not queue.empty()

        second = queue.get(block=False)
        assert second.unique_id in {"A", "Z"}
        assert not queue.empty()
        queue.mark_done(second.unique_id)
        assert not queue.empty()

        third = queue.get(block=False)
        assert third.unique_id in {"A", "Z"}
        with pytest.raises(Empty):
            queue.get(block=False)
        assert second.unique_id != third.unique_id
        assert queue.empty()
        queue.mark_done(third.unique_id)
        self.assert_would_join(queue)
        assert queue.empty()

    def test_linker_dependencies_limited_to_some_nodes(self, linker: Linker) -> None:
        actual_deps = [("A", "B"), ("B", "C"), ("C", "D")]

        for l, r in actual_deps:
            linker.dependency(l, r)

        queue = self._get_graph_queue(_mock_manifest("ABCD"), linker, ["B"])
        got = queue.get(block=False)
        assert got.unique_id == "B"
        assert queue.empty()
        queue.mark_done("B")
        self.assert_would_join(queue)

        queue_2 = queue = self._get_graph_queue(_mock_manifest("ABCD"), linker, ["A", "B"])
        got = queue_2.get(block=False)
        assert got.unique_id == "B"
        assert not queue_2.empty()
        with pytest.raises(Empty):
            queue_2.get(block=False)
        queue_2.mark_done("B")
        assert not queue_2.empty()

        got = queue_2.get(block=False)
        assert got.unique_id == "A"
        assert queue_2.empty()
        with pytest.raises(Empty):
            queue_2.get(block=False)
        assert queue_2.empty()
        queue_2.mark_done("A")
        self.assert_would_join(queue_2)

    def test__find_cycles__cycles(self, linker: Linker) -> None:
        actual_deps = [("A", "B"), ("B", "C"), ("C", "A")]

        for l, r in actual_deps:
            linker.dependency(l, r)

        assert linker.find_cycles() is not None

    def test__find_cycles__no_cycles(self, linker: Linker) -> None:
        actual_deps = [("A", "B"), ("B", "C"), ("C", "D")]

        for l, r in actual_deps:
            linker.dependency(l, r)

        assert linker.find_cycles() is None
