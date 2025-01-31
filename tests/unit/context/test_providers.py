from argparse import Namespace
from datetime import datetime
from typing import Optional
from unittest import mock

import pytest
import pytz
from pytest_mock import MockerFixture

from dbt.adapters.base import BaseRelation
from dbt.artifacts.resources import NodeConfig, Quoting
from dbt.artifacts.resources.types import BatchSize
from dbt.context.providers import (
    BaseResolver,
    EventTimeFilter,
    RuntimeRefResolver,
    RuntimeSourceResolver,
)
from dbt.contracts.graph.nodes import BatchContext, ModelNode
from dbt.event_time.sample_window import SampleWindow
from dbt.flags import set_from_args


class TestBaseResolver:
    class ResolverSubclass(BaseResolver):
        def __call__(self, *args: str):
            pass

    @pytest.fixture
    def resolver(self):
        return self.ResolverSubclass(
            db_wrapper=mock.Mock(),
            model=mock.Mock(),
            config=mock.Mock(),
            manifest=mock.Mock(),
        )

    @pytest.mark.parametrize(
        "empty,expected_resolve_limit",
        [(False, None), (True, 0)],
    )
    def test_resolve_limit(self, resolver, empty, expected_resolve_limit):
        resolver.config.args.EMPTY = empty

        assert resolver.resolve_limit == expected_resolve_limit

    @pytest.mark.parametrize(
        "use_microbatch_batches,materialized,incremental_strategy,sample_mode,sample_window,resolver_model_node,expect_filter",
        [
            (
                True,
                "incremental",
                "microbatch",
                False,
                None,
                True,
                True,
            ),  # Microbatch model without sample
            (
                True,
                "incremental",
                "microbatch",
                True,
                SampleWindow(
                    start=datetime(2024, 1, 1, tzinfo=pytz.UTC),
                    end=datetime(2025, 1, 1, tzinfo=pytz.UTC),
                ),
                True,
                True,
            ),  # Microbatch model with sample
            (
                False,
                "table",
                None,
                True,
                SampleWindow(
                    start=datetime(2024, 1, 1, tzinfo=pytz.UTC),
                    end=datetime(2025, 1, 1, tzinfo=pytz.UTC),
                ),
                True,
                True,
            ),  # Normal model with sample
            (
                True,
                "incremental",
                "merge",
                True,
                SampleWindow(
                    start=datetime(2024, 1, 1, tzinfo=pytz.UTC),
                    end=datetime(2025, 1, 1, tzinfo=pytz.UTC),
                ),
                True,
                True,
            ),  # Incremental merge model with sample
            (
                False,
                "table",
                None,
                True,
                SampleWindow(
                    start=datetime(2024, 1, 1, tzinfo=pytz.UTC),
                    end=datetime(2025, 1, 1, tzinfo=pytz.UTC),
                ),
                False,
                False,
            ),  # Sample, but not model node
            (
                True,
                "incremental",
                "microbatch",
                False,
                None,
                False,
                False,
            ),  # Microbatch, but not model node
            (
                False,
                "incremental",
                "microbatch",
                False,
                None,
                True,
                False,
            ),  # Mircrobatch model, but not using batches
            (
                True,
                "table",
                "microbatch",
                False,
                None,
                True,
                False,
            ),  # Non microbatch model, but supposed to use batches
            (True, "incremental", "merge", False, None, True, False),  # Incremental merge
        ],
    )
    def test_resolve_event_time_filter(
        self,
        mocker: MockerFixture,
        resolver: ResolverSubclass,
        use_microbatch_batches: bool,
        materialized: str,
        incremental_strategy: Optional[str],
        sample_mode: bool,
        sample_window: Optional[SampleWindow],
        resolver_model_node: bool,
        expect_filter: bool,
    ) -> None:
        # Target mocking
        target = mock.Mock()
        target.config = mock.MagicMock(NodeConfig)
        target.config.event_time = "created_at"

        # Resolver mocking
        resolver.config.args.EVENT_TIME_END = None
        resolver.config.args.EVENT_TIME_START = None
        resolver.config.args.sample = sample_mode
        resolver.config.args.sample_window = sample_window
        if resolver_model_node:
            resolver.model = mock.MagicMock(spec=ModelNode)
        resolver.model.batch = BatchContext(
            id="1",
            event_time_start=datetime(2024, 1, 1, tzinfo=pytz.UTC),
            event_time_end=datetime(2025, 1, 1, tzinfo=pytz.UTC),
        )
        resolver.model.config = mock.MagicMock(NodeConfig)
        resolver.model.config.materialized = materialized
        resolver.model.config.incremental_strategy = incremental_strategy
        resolver.model.config.batch_size = BatchSize.day
        resolver.model.config.lookback = 1
        resolver.manifest.use_microbatch_batches = mock.Mock()
        resolver.manifest.use_microbatch_batches.return_value = use_microbatch_batches

        # Try to get an EventTimeFilter
        event_time_filter = resolver.resolve_event_time_filter(target=target)

        if expect_filter:
            assert isinstance(event_time_filter, EventTimeFilter)
        else:
            assert event_time_filter is None


class TestRuntimeRefResolver:
    @pytest.fixture
    def resolver(self):
        mock_db_wrapper = mock.Mock()
        mock_db_wrapper.Relation = BaseRelation

        return RuntimeRefResolver(
            db_wrapper=mock_db_wrapper,
            model=mock.Mock(),
            config=mock.Mock(),
            manifest=mock.Mock(),
        )

    @pytest.mark.parametrize(
        "empty,is_ephemeral_model,expected_limit",
        [
            (False, False, None),
            (True, False, 0),
            (False, True, None),
            (True, True, 0),
        ],
    )
    def test_create_relation_with_empty(self, resolver, empty, is_ephemeral_model, expected_limit):
        # setup resolver and input node
        resolver.config.args.EMPTY = empty
        resolver.config.quoting = {}
        mock_node = mock.Mock()
        mock_node.database = "test"
        mock_node.schema = "test"
        mock_node.identifier = "test"
        mock_node.quoting_dict = {}
        mock_node.alias = "test"
        mock_node.is_ephemeral_model = is_ephemeral_model
        mock_node.defer_relation = None

        set_from_args(
            Namespace(require_batched_execution_for_custom_microbatch_strategy=False), None
        )

        # create limited relation
        with mock.patch("dbt.contracts.graph.nodes.ParsedNode", new=mock.Mock):
            relation = resolver.create_relation(mock_node)
        assert relation.limit == expected_limit


class TestRuntimeSourceResolver:
    @pytest.fixture
    def resolver(self):
        mock_db_wrapper = mock.Mock()
        mock_db_wrapper.Relation = BaseRelation

        return RuntimeSourceResolver(
            db_wrapper=mock_db_wrapper,
            model=mock.Mock(),
            config=mock.Mock(),
            manifest=mock.Mock(),
        )

    @pytest.mark.parametrize(
        "empty,expected_limit",
        [
            (False, None),
            (True, 0),
        ],
    )
    def test_create_relation_with_empty(self, resolver, empty, expected_limit):
        # setup resolver and input source
        resolver.config.args.EMPTY = empty
        resolver.config.quoting = {}

        mock_source = mock.Mock()
        mock_source.database = "test"
        mock_source.schema = "test"
        mock_source.identifier = "test"
        mock_source.quoting = Quoting()
        mock_source.quoting_dict = {}
        resolver.manifest.resolve_source.return_value = mock_source

        set_from_args(
            Namespace(require_batched_execution_for_custom_microbatch_strategy=False), None
        )

        # create limited relation
        relation = resolver.resolve("test", "test")
        assert relation.limit == expected_limit
