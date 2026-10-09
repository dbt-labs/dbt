from types import SimpleNamespace
from typing import List, Optional, Set, Tuple
from unittest.mock import MagicMock, patch

import pytest
from metricflow_semantic_interfaces.type_enums import DimensionType, EntityType

from dbt.artifacts.resources import CustomGranularity, Dimension, Entity, TimeSpine
from dbt.config import RuntimeConfig
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.nodes import SemanticModel
from dbt.exceptions import ParsingError, TargetNotFoundError
from dbt.parser.exposure_dimensions import (
    granularity_names,
    resolve_exposure_dimension,
    split_exposure_dimension,
)
from dbt.parser.manifest import ManifestLoader
from tests.unit.utils.manifest import make_exposure, make_model, make_semantic_model

STANDARD = granularity_names([])

CAT = DimensionType.CATEGORICAL
TIME = DimensionType.TIME
PRIMARY = EntityType.PRIMARY
FOREIGN = EntityType.FOREIGN


def semantic_model(
    name: str,
    entities: List[Tuple[str, EntityType]],
    dimensions: List[Tuple[str, DimensionType]],
    pkg: str = "test",
) -> SemanticModel:
    sm = make_semantic_model(pkg, name, SimpleNamespace(name=name, alias=name))
    sm.entities = [Entity(name=n, type=t) for n, t in entities]
    sm.dimensions = [Dimension(name=n, type=t) for n, t in dimensions]
    return sm


def resolve(
    specifier: str,
    semantic_models: List[SemanticModel],
    granularities: Optional[Set[str]] = None,
) -> List[str]:
    known_entities = {e.name for sm in semantic_models for e in sm.entities}
    matches = resolve_exposure_dimension(
        specifier, semantic_models, granularities or STANDARD, known_entities
    )
    return [sm.unique_id for sm in matches]


@pytest.fixture
def customers():
    return semantic_model("customers", [("customer", PRIMARY)], [("country", CAT)])


@pytest.fixture
def orders():
    return semantic_model(
        "orders",
        [("order", PRIMARY), ("customer", FOREIGN)],
        [("ordered_at", TIME), ("status", CAT), ("month", CAT)],
    )


class TestSplit:
    def test_valid(self):
        assert split_exposure_dimension("customer__country") == ["customer", "country"]

    @pytest.mark.parametrize("specifier", ["country", "", "__country", "customer__", "a____b"])
    def test_invalid_shape(self, specifier):
        with pytest.raises(ParsingError):
            split_exposure_dimension(specifier)

    @pytest.mark.parametrize(
        "specifier", ["metric_time", "metric_time__month", "customer__metric_time"]
    )
    def test_metric_time(self, specifier):
        with pytest.raises(ParsingError, match="metric_time"):
            split_exposure_dimension(specifier)


class TestGranularityNames:
    def test_standard(self):
        assert {"day", "month", "year"} <= STANDARD

    def test_custom_from_time_spine(self):
        model = make_model("test", "time_spine", "select 1")
        model.time_spine = TimeSpine(
            standard_granularity_column="date_day",
            custom_granularities=[CustomGranularity(name="Fiscal_Quarter")],
        )
        assert "fiscal_quarter" in granularity_names([model, SimpleNamespace()])

    def test_models_without_time_spine(self):
        assert granularity_names([make_model("test", "m", "select 1")]) == STANDARD


class TestResolve:
    def test_simple(self, customers, orders):
        assert resolve("customer__country", [customers, orders]) == [customers.unique_id]

    def test_grain_stripped(self, customers, orders):
        assert resolve("order__ordered_at__month", [customers, orders]) == [orders.unique_id]

    def test_custom_grain_stripped(self, orders):
        assert resolve(
            "order__ordered_at__fiscal_quarter", [orders], STANDARD | {"fiscal_quarter"}
        ) == [orders.unique_id]

    def test_unknown_custom_grain_not_found(self, orders):
        assert resolve("order__ordered_at__fiscal_quarter", [orders]) == []

    def test_no_greedy_strip(self, orders):
        assert resolve("order__month", [orders]) == [orders.unique_id]

    def test_no_greedy_strip_with_entity_path(self, orders):
        customers = semantic_model("customers", [("customer", PRIMARY)], [("order", TIME)])
        assert resolve("customer__order__month", [customers, orders]) == [orders.unique_id]

    def test_grain_on_categorical_errors(self, customers):
        with pytest.raises(ParsingError, match="time dimension"):
            resolve("customer__country__month", [customers])

    def test_entity_missing_from_every_semantic_model_not_found(self, orders):
        assert resolve("nowhere__order__status", [orders]) == []

    def test_path_entity_not_known_is_not_found_even_if_last_entity_matches(self, orders):
        assert (
            resolve_exposure_dimension(
                "region__order__status", [orders], STANDARD, {"order", "customer"}
            )
            == []
        )

    def test_unknown_dimension_not_found(self, customers):
        assert resolve("customer__missing", [customers]) == []

    def test_multi_hop_owner_is_last_entity(self, customers, orders):
        other = semantic_model("other", [("order", PRIMARY)], [("country", CAT)])
        assert resolve("order__customer__country", [orders, other, customers]) == [
            customers.unique_id
        ]

    def test_linkable_preferred_over_foreign(self, customers):
        foreign = semantic_model("aaa_events", [("customer", FOREIGN)], [("country", CAT)])
        assert resolve("customer__country", [foreign, customers]) == [customers.unique_id]

    @pytest.mark.parametrize("entity_type", [EntityType.UNIQUE, EntityType.NATURAL])
    def test_unique_and_natural_are_linkable(self, entity_type):
        foreign = semantic_model("aaa", [("customer", FOREIGN)], [("country", CAT)])
        linkable = semantic_model("zzz", [("customer", entity_type)], [("country", CAT)])
        assert resolve("customer__country", [foreign, linkable]) == [linkable.unique_id]

    def test_all_same_priority_matches_sorted(self):
        b = semantic_model("b_customers", [("customer", PRIMARY)], [("country", CAT)], pkg="zpkg")
        a = semantic_model("a_customers", [("customer", EntityType.UNIQUE)], [("country", CAT)])
        assert resolve("customer__country", [b, a]) == [a.unique_id, b.unique_id]

    def test_all_foreign_matches_added_sorted(self):
        x = semantic_model("x", [("customer", FOREIGN)], [("country", CAT)])
        y = semantic_model("y", [("customer", FOREIGN)], [("country", CAT)])
        assert resolve("customer__country", [y, x]) == [x.unique_id, y.unique_id]

    def test_searches_all_packages(self):
        other_pkg = semantic_model(
            "customers", [("customer", PRIMARY)], [("country", CAT)], pkg="other_pkg"
        )
        assert resolve("customer__country", [other_pkg]) == [other_pkg.unique_id]


class TestProcessExposureDimensions:
    @pytest.fixture
    @patch("dbt.parser.manifest.ManifestLoader.build_manifest_state_check")
    @patch("dbt.parser.manifest.os.path.exists")
    @patch("dbt.parser.manifest.open")
    def loader(self, patched_open, patched_os_exist, patched_state_check):
        mock_project = MagicMock(RuntimeConfig)
        mock_project.project_target_path = "mock_target_path"
        loader = ManifestLoader(mock_project, {})
        loader.manifest = Manifest()
        return loader

    @staticmethod
    def add_semantic_model(loader, sm):
        loader.manifest.semantic_models[sm.unique_id] = sm
        return sm

    @staticmethod
    def add_exposure(loader, dimensions, depends_on_nodes=None):
        exposure = make_exposure("test", "dash")
        exposure.dimensions = dimensions
        exposure.depends_on.nodes = list(depends_on_nodes or [])
        loader.manifest.exposures[exposure.unique_id] = exposure
        return exposure

    def test_replaces_stale_semantic_model_dependencies(self, loader, customers):
        self.add_semantic_model(loader, customers)
        exposure = self.add_exposure(
            loader,
            ["customer__country"],
            ["model.test.m", "semantic_model.test.renamed_away"],
        )

        loader.process_exposure_dimensions()

        assert exposure.depends_on.nodes == ["model.test.m", customers.unique_id]

    def test_adds_all_matches(self, loader):
        a = self.add_semantic_model(
            loader, semantic_model("a", [("customer", PRIMARY)], [("country", CAT)])
        )
        b = self.add_semantic_model(
            loader, semantic_model("b", [("customer", PRIMARY)], [("country", CAT)])
        )
        exposure = self.add_exposure(loader, ["customer__country"])

        loader.process_exposure_dimensions()

        assert exposure.depends_on.nodes == [a.unique_id, b.unique_id]

    def test_grain_uses_time_spine_custom_granularity(self, loader, orders):
        self.add_semantic_model(loader, orders)
        model = make_model("test", "time_spine", "select 1")
        model.time_spine = TimeSpine(
            standard_granularity_column="date_day",
            custom_granularities=[CustomGranularity(name="fiscal_quarter")],
        )
        loader.manifest.nodes[model.unique_id] = model
        exposure = self.add_exposure(loader, ["order__ordered_at__fiscal_quarter"])

        loader.process_exposure_dimensions()

        assert exposure.depends_on.nodes == [orders.unique_id]

    def test_not_found_raises_and_disables_exposure(self, loader, customers):
        self.add_semantic_model(loader, customers)
        exposure = self.add_exposure(loader, ["customer__missing"])

        with pytest.raises(TargetNotFoundError, match="dimension named 'customer__missing'"):
            loader.process_exposure_dimensions()

        assert exposure.config.enabled is False

    def test_disabled_semantic_model_reported_as_disabled(self, loader, customers):
        loader.manifest.disabled[customers.unique_id] = [customers]
        self.add_exposure(loader, ["customer__country"])

        with pytest.raises(TargetNotFoundError, match="is disabled"):
            loader.process_exposure_dimensions()

    def test_format_error_names_the_exposure(self, loader, customers):
        self.add_semantic_model(loader, customers)
        exposure = self.add_exposure(loader, ["metric_time__month"])

        with pytest.raises(ParsingError, match="metric_time") as excinfo:
            loader.process_exposure_dimensions()

        assert exposure.name in str(excinfo.value)
        assert excinfo.value.node is exposure

    def test_exposure_without_dimensions_keeps_dependencies(self, loader, customers):
        self.add_semantic_model(loader, customers)
        exposure = self.add_exposure(loader, [], ["semantic_model.test.other"])

        loader.process_exposure_dimensions()

        assert exposure.depends_on.nodes == ["semantic_model.test.other"]
