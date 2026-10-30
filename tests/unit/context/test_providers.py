from argparse import Namespace
from datetime import datetime
from typing import Any, Optional, Type, Union
from unittest import mock

import pytest
import pytz

from dbt.adapters.base import BaseRelation
from dbt.artifacts.resources import NodeConfig, Quoting, SeedConfig, SnapshotConfig
from dbt.artifacts.resources.types import BatchSize
from dbt.context.providers import (
    BaseResolver,
    EventTimeFilter,
    ExposureDimensionResolver,
    RuntimeRefResolver,
    RuntimeSourceResolver,
)
from dbt.contracts.graph.nodes import BatchContext, ModelNode, SnapshotNode
from dbt.event_time.sample_window import SampleWindow
from dbt.exceptions import DimensionArgsError
from dbt.flags import set_from_args
from tests.unit.utils.manifest import make_exposure


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
        "event_time_column,column_quote,source_quote,expected_field_name",
        [
            # Simple column name, no quoting needed
            ("simple_column", None, None, "simple_column"),
            ("no_quote_column", False, None, "no_quote_column"),
            ("source_no_quote", None, False, "source_no_quote"),
            ("both_no_quote", False, False, "both_no_quote"),
            # Column-level quote configuration (takes precedence)
            ("column_quoted", True, None, '"column_quoted"'),
            ("column_quoted_source_no", True, False, '"column_quoted_source_no"'),
            ("column_quoted_source_yes", True, True, '"column_quoted_source_yes"'),
            # Source-level quote configuration (fallback when column-level is None)
            ("source_quoted", None, True, '"source_quoted"'),
            (
                "source_quoted_override",
                False,
                True,
                "source_quoted_override",
            ),  # False overrides True
            # Camel case and spaced column names
            ("camelCaseColumn", None, None, "camelCaseColumn"),
            ("camelCaseQuoted", True, None, '"camelCaseQuoted"'),
            ("snake_case_column", None, None, "snake_case_column"),
            ("snake_case_quoted", True, None, '"snake_case_quoted"'),
            ("Spaced Column Name", None, None, "Spaced Column Name"),
            ("Spaced Column Quoted", True, None, '"Spaced Column Quoted"'),
            ("Spaced Column Source Quoted", None, True, '"Spaced Column Source Quoted"'),
            # Edge cases
            ("", None, None, ""),
            ("", True, None, '""'),
            ("edge_case_column", None, None, "edge_case_column"),
            ("edge_case_quoted", True, None, '"edge_case_quoted"'),
        ],
    )
    def test_resolve_event_time_field_name(
        self, resolver, event_time_column, column_quote, source_quote, expected_field_name
    ):
        """Test the _resolve_event_time_field_name method with various quoting configurations."""
        # Create a mock target with columns
        target = mock.Mock()
        target.config = mock.Mock()
        target.config.event_time = event_time_column

        # Mock columns dictionary
        mock_column = mock.Mock()
        mock_column.name = event_time_column
        mock_column.data_type = "timestamp"

        # Set column-level quote configuration
        if column_quote is not None:
            mock_column.quote = column_quote
        else:
            # Explicitly set to None to avoid Mock object being returned
            mock_column.quote = None

        target.columns = {event_time_column: mock_column}

        # Set source-level quote configuration
        if source_quote is not None:
            target.quoting = mock.Mock()
            target.quoting.column = source_quote
        else:
            # Explicitly set to None to avoid Mock object being returned
            target.quoting = mock.Mock()
            target.quoting.column = None

        # Call the method
        result = resolver._resolve_event_time_field_name(target)

        # Assert the result
        assert result == expected_field_name

    @pytest.mark.parametrize(
        "event_time_column,column_quote,source_quote,expected_field_name",
        [
            # Column not found in columns dict - should fall back to source-level quoting
            ("missing_column", None, None, "missing_column"),
            ("missing_column_source_quoted", None, True, '"missing_column_source_quoted"'),
            ("missing_column_source_no_quote", None, False, "missing_column_source_no_quote"),
            # Column found but no quote attribute - should fall back to source-level quoting
            ("found_no_quote_attr", None, None, "found_no_quote_attr"),
            (
                "found_no_quote_attr_source_quoted",
                None,
                True,
                '"found_no_quote_attr_source_quoted"',
            ),
            (
                "found_no_quote_attr_source_no_quote",
                None,
                False,
                "found_no_quote_attr_source_no_quote",
            ),
        ],
    )
    def test_resolve_event_time_field_name_column_not_found(
        self, resolver, event_time_column, column_quote, source_quote, expected_field_name
    ):
        """Test _resolve_event_time_field_name when column is not found or has no quote attribute."""
        # Create a mock target with different columns
        target = mock.Mock()
        target.config = mock.Mock()
        target.config.event_time = event_time_column

        # Mock columns dictionary with different column
        mock_column = mock.Mock()
        mock_column.name = "different_column_name"
        mock_column.data_type = "timestamp"

        # Set column-level quote configuration (but for different column)
        if column_quote is not None:
            mock_column.quote = column_quote
        else:
            # Explicitly set to None to avoid Mock object being returned
            mock_column.quote = None

        target.columns = {"different_column_name": mock_column}

        # Set source-level quote configuration
        if source_quote is not None:
            target.quoting = mock.Mock()
            target.quoting.column = source_quote
        else:
            # Explicitly set to None to avoid Mock object being returned
            target.quoting = mock.Mock()
            target.quoting.column = None

        # Call the method
        result = resolver._resolve_event_time_field_name(target)

        # Assert the result
        assert result == expected_field_name

    def test_resolve_event_time_field_name_no_columns(self, resolver):
        """Test _resolve_event_time_field_name when target has no columns attribute."""
        # Create a mock target without columns
        target = mock.Mock()
        target.config = mock.Mock()
        target.config.event_time = "no_columns_column"

        # No columns attribute
        target.columns = {}

        # Set source-level quote configuration
        target.quoting = mock.Mock()
        target.quoting.column = True

        # Call the method
        result = resolver._resolve_event_time_field_name(target)

        # Should return quoted column name when source-level quoting is True
        assert result == '"no_columns_column"'

    def test_resolve_event_time_field_name_no_quoting_attribute(self, resolver):
        """Test _resolve_event_time_field_name when target has no quoting attribute."""
        # Create a mock target without quoting
        target = mock.Mock()
        target.config = mock.Mock()
        target.config.event_time = "no_quoting_attr_column"

        # Mock columns dictionary
        mock_column = mock.Mock()
        mock_column.name = "no_quoting_attr_column"
        mock_column.data_type = "timestamp"
        # No quote attribute - explicitly set to None
        mock_column.quote = None

        target.columns = {"no_quoting_attr_column": mock_column}

        # No quoting attribute - explicitly set to None
        target.quoting = mock.Mock()
        target.quoting.column = None

        # Call the method
        result = resolver._resolve_event_time_field_name(target)

        # Should return unquoted column name
        assert result == "no_quoting_attr_column"

    @pytest.mark.parametrize(
        "use_microbatch_batches,materialized,incremental_strategy,sample,resolver_model_node,target_type,resolver_model_type,expect_filter",
        [
            # Microbatch model without sample
            (
                True,
                "incremental",
                "microbatch",
                None,
                True,
                NodeConfig,
                ModelNode,
                True,
            ),
            # Microbatch model with sample
            (
                True,
                "incremental",
                "microbatch",
                SampleWindow(
                    start=datetime(2024, 1, 1, tzinfo=pytz.UTC),
                    end=datetime(2025, 1, 1, tzinfo=pytz.UTC),
                ),
                True,
                NodeConfig,
                ModelNode,
                True,
            ),
            # Normal model with sample
            (
                False,
                "table",
                None,
                SampleWindow(
                    start=datetime(2024, 1, 1, tzinfo=pytz.UTC),
                    end=datetime(2025, 1, 1, tzinfo=pytz.UTC),
                ),
                True,
                NodeConfig,
                ModelNode,
                True,
            ),
            # Incremental merge model with sample
            (
                True,
                "incremental",
                "merge",
                SampleWindow(
                    start=datetime(2024, 1, 1, tzinfo=pytz.UTC),
                    end=datetime(2025, 1, 1, tzinfo=pytz.UTC),
                ),
                True,
                NodeConfig,
                ModelNode,
                True,
            ),
            # Sample, but not model node
            (
                False,
                "table",
                None,
                SampleWindow(
                    start=datetime(2024, 1, 1, tzinfo=pytz.UTC),
                    end=datetime(2025, 1, 1, tzinfo=pytz.UTC),
                ),
                False,
                NodeConfig,
                ModelNode,
                False,
            ),
            # Microbatch, but not model node
            (
                True,
                "incremental",
                "microbatch",
                None,
                False,
                NodeConfig,
                ModelNode,
                False,
            ),
            # Mircrobatch model, but not using batches
            (
                False,
                "incremental",
                "microbatch",
                None,
                True,
                NodeConfig,
                ModelNode,
                False,
            ),
            # Non microbatch model, but supposed to use batches
            (
                True,
                "table",
                "microbatch",
                None,
                True,
                NodeConfig,
                ModelNode,
                False,
            ),
            # Incremental merge
            (True, "incremental", "merge", None, True, NodeConfig, ModelNode, False),
            # Target seed node, with sample
            (
                False,
                "table",
                None,
                SampleWindow.from_relative_string("2 days"),
                True,
                SeedConfig,
                ModelNode,
                True,
            ),
            # Target seed node, without sample
            (False, "table", None, None, True, SeedConfig, ModelNode, False),
            # Sample model from snapshot node
            (
                False,
                "table",
                None,
                SampleWindow.from_relative_string("2 days"),
                True,
                NodeConfig,
                SnapshotNode,
                True,
            ),
            # Target model from snapshot, without sample
            (False, "table", None, None, True, NodeConfig, SnapshotNode, False),
            # Target snapshot from model, with sample
            (
                False,
                "table",
                None,
                SampleWindow.from_relative_string("2 days"),
                True,
                SnapshotConfig,
                ModelNode,
                True,
            ),
        ],
    )
    def test_resolve_event_time_filter(
        self,
        resolver: ResolverSubclass,
        use_microbatch_batches: bool,
        materialized: str,
        incremental_strategy: Optional[str],
        sample: Optional[SampleWindow],
        resolver_model_node: bool,
        target_type: Any,
        resolver_model_type: Union[Type[ModelNode], Type[SnapshotNode]],
        expect_filter: bool,
    ) -> None:
        # Target mocking
        target = mock.Mock()
        target.config = mock.MagicMock(target_type)
        target.config.event_time = "created_at"

        # Resolver mocking
        resolver.config.args.EVENT_TIME_END = None
        resolver.config.args.EVENT_TIME_START = None
        resolver.config.args.sample = sample
        if resolver_model_node:
            resolver.model = mock.MagicMock(spec=resolver_model_type)
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


class TestExposureDimensionResolver:
    @pytest.fixture
    def exposure(self):
        return make_exposure("test", "dash")

    @pytest.fixture
    def resolver(self, exposure):
        return ExposureDimensionResolver(
            db_wrapper=None, model=exposure, config=mock.Mock(), manifest=mock.Mock()
        )

    def test_records_string_as_written(self, resolver, exposure):
        assert resolver("Customer__country") == ""
        resolver("order__ordered_at__month")
        assert exposure.dimensions == ["Customer__country", "order__ordered_at__month"]

    @pytest.mark.parametrize("args", [(), ("a__b", "c__d"), (1,), (["a__b"],)])
    def test_bad_arguments(self, resolver, exposure, args):
        with pytest.raises(DimensionArgsError) as excinfo:
            resolver(*args)
        assert f"got {args!r}" in str(excinfo.value)
        assert exposure.dimensions == []
