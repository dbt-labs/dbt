from copy import deepcopy
from datetime import datetime
from typing import List

import pytest
from freezegun import freeze_time

from dbt.artifacts.resources import (
    Defaults,
    Dimension,
    Entity,
    FileHash,
    Measure,
    TestMetadata,
)
from dbt.artifacts.resources.v1.semantic_model import NodeRelation
from dbt.contracts.graph.model_config import TestConfig
from dbt.contracts.graph.nodes import ColumnInfo, ModelNode, ParsedNode, SemanticModel
from dbt.node_types import NodeType
from dbt_common.contracts.constraints import (
    ColumnLevelConstraint,
    ConstraintType,
    ModelLevelConstraint,
)
from dbt_semantic_interfaces.references import MeasureReference
from dbt_semantic_interfaces.type_enums import (
    AggregationType,
    DimensionType,
    EntityType,
)
from tests.unit.fixtures import generic_test_node, model_node


class TestModelNode:
    @pytest.fixture(scope="class")
    def default_model_node(self):
        return ModelNode(
            resource_type=NodeType.Model,
            unique_id="model.test_package.test_name",
            name="test_name",
            package_name="test_package",
            schema="test_schema",
            alias="test_alias",
            fqn=["models", "test_name"],
            original_file_path="test_original_file_path",
            checksum=FileHash.from_contents("checksum"),
            path="test_path",
            database=None,
        )

    @pytest.mark.parametrize(
        "deprecation_date,current_date,expected_is_past_deprecation_date",
        [
            (None, "2024-05-02", False),
            ("2024-05-01", "2024-05-02", True),
            ("2024-05-01", "2024-05-01", False),
            ("2024-05-01", "2024-04-30", False),
        ],
    )
    def test_is_past_deprecation_date(
        self, default_model_node, deprecation_date, current_date, expected_is_past_deprecation_date
    ):
        with freeze_time(current_date):
            if deprecation_date is not None:
                default_model_node.deprecation_date = datetime.strptime(
                    deprecation_date, "%Y-%m-%d"
                ).astimezone()

            assert default_model_node.is_past_deprecation_date is expected_is_past_deprecation_date

    @pytest.mark.parametrize(
        "model_constraints,columns,expected_all_constraints",
        [
            ([], {}, []),
            (
                [ModelLevelConstraint(type=ConstraintType.foreign_key)],
                {},
                [ModelLevelConstraint(type=ConstraintType.foreign_key)],
            ),
            (
                [],
                {
                    "id": ColumnInfo(
                        name="id",
                        constraints=[ColumnLevelConstraint(type=ConstraintType.foreign_key)],
                    )
                },
                [ColumnLevelConstraint(type=ConstraintType.foreign_key)],
            ),
            (
                [ModelLevelConstraint(type=ConstraintType.foreign_key)],
                {
                    "id": ColumnInfo(
                        name="id",
                        constraints=[ColumnLevelConstraint(type=ConstraintType.foreign_key)],
                    )
                },
                [
                    ModelLevelConstraint(type=ConstraintType.foreign_key),
                    ColumnLevelConstraint(type=ConstraintType.foreign_key),
                ],
            ),
        ],
    )
    def test_all_constraints(
        self, default_model_node, model_constraints, columns, expected_all_constraints
    ):
        default_model_node.constraints = model_constraints
        default_model_node.columns = columns

        assert default_model_node.all_constraints == expected_all_constraints


class TestSemanticModel:
    @pytest.fixture(scope="function")
    def dimensions(self) -> List[Dimension]:
        return [Dimension(name="ds", type=DimensionType)]

    @pytest.fixture(scope="function")
    def entities(self) -> List[Entity]:
        return [Entity(name="test_entity", type=EntityType.PRIMARY, expr="id")]

    @pytest.fixture(scope="function")
    def measures(self) -> List[Measure]:
        return [Measure(name="test_measure", agg=AggregationType.COUNT, expr="id")]

    @pytest.fixture(scope="function")
    def default_semantic_model(
        self, dimensions: List[Dimension], entities: List[Entity], measures: List[Measure]
    ) -> SemanticModel:
        return SemanticModel(
            name="test_semantic_model",
            resource_type=NodeType.SemanticModel,
            model="ref('test_model')",
            package_name="test",
            path="test_path",
            original_file_path="test_fixture",
            unique_id=f"{NodeType.SemanticModel}.test.test_semantic_model",
            fqn=[],
            defaults=Defaults(agg_time_dimension="ds"),
            dimensions=dimensions,
            entities=entities,
            measures=measures,
            node_relation=NodeRelation(
                alias="test_alias", schema_name="test_schema", database="test_database"
            ),
        )

    def test_checked_agg_time_dimension_for_measure_via_defaults(
        self,
        default_semantic_model: SemanticModel,
    ):
        assert default_semantic_model.defaults.agg_time_dimension is not None
        measure = default_semantic_model.measures[0]
        measure.agg_time_dimension = None
        default_semantic_model.checked_agg_time_dimension_for_measure(
            MeasureReference(element_name=measure.name)
        )

    def test_checked_agg_time_dimension_for_measure_via_measure(
        self, default_semantic_model: SemanticModel
    ):
        default_semantic_model.defaults = None
        measure = default_semantic_model.measures[0]
        measure.agg_time_dimension = default_semantic_model.dimensions[0].name
        default_semantic_model.checked_agg_time_dimension_for_measure(
            MeasureReference(element_name=measure.name)
        )

    def test_checked_agg_time_dimension_for_measure_exception(
        self, default_semantic_model: SemanticModel
    ):
        default_semantic_model.defaults = None
        measure = default_semantic_model.measures[0]
        measure.agg_time_dimension = None

        with pytest.raises(AssertionError) as execinfo:
            default_semantic_model.checked_agg_time_dimension_for_measure(
                MeasureReference(measure.name)
            )

        assert (
            f"Aggregation time dimension for measure {measure.name} on semantic model {default_semantic_model.name}"
            in str(execinfo.value)
        )

    def test_semantic_model_same_contents(self, default_semantic_model: SemanticModel):
        default_semantic_model_copy = deepcopy(default_semantic_model)

        assert default_semantic_model.same_contents(default_semantic_model_copy)

    def test_semantic_model_same_contents_update_model(
        self, default_semantic_model: SemanticModel
    ):
        default_semantic_model_copy = deepcopy(default_semantic_model)
        default_semantic_model_copy.model = "ref('test_another_model')"

        assert not default_semantic_model.same_contents(default_semantic_model_copy)

    def test_semantic_model_same_contents_different_node_relation(
        self,
        default_semantic_model: SemanticModel,
    ):
        default_semantic_model_copy = deepcopy(default_semantic_model)
        default_semantic_model_copy.node_relation.alias = "test_another_alias"
        # Relation should not be consided in same_contents
        assert default_semantic_model.same_contents(default_semantic_model_copy)


# Infer primary key
def test_no_primary_key():
    model = model_node()
    assert model.infer_primary_key([]) == []


def test_primary_key_model_constraint():
    model = model_node()
    model.constraints = [ModelLevelConstraint(type=ConstraintType.primary_key, columns=["pk"])]
    assertSameContents(model.infer_primary_key([]), ["pk"])

    model.constraints = [
        ModelLevelConstraint(type=ConstraintType.primary_key, columns=["pk1", "pk2"])
    ]
    assertSameContents(model.infer_primary_key([]), ["pk1", "pk2"])


def test_primary_key_column_constraint():
    model = model_node()
    model.columns = {
        "column1": ColumnInfo(
            "column1", constraints=[ColumnLevelConstraint(type=ConstraintType.primary_key)]
        ),
        "column2": ColumnInfo("column2"),
    }
    assertSameContents(model.infer_primary_key([]), ["column1"])


def test_unique_non_null_single():
    model = model_node()
    test1 = generic_test_node()
    test1.test_metadata = TestMetadata(name="unique", kwargs={"column_name": "column1"})
    test2 = generic_test_node()
    test2.test_metadata = TestMetadata(name="not_null", kwargs={"column_name": "column1"})
    test3 = generic_test_node()
    test3.test_metadata = TestMetadata(name="unique", kwargs={"column_name": "column2"})
    tests = [test1, test2]
    assertSameContents(model.infer_primary_key(tests), ["column1"])


def test_unique_non_null_multiple():
    model = model_node()
    tests = []
    for i in range(2):
        for enabled in [True, False]:
            test1 = generic_test_node()
            test1.test_metadata = TestMetadata(
                name="unique", kwargs={"column_name": "column" + str(i) + str(enabled)}
            )
            test1.config = TestConfig(enabled=enabled)
            test2 = generic_test_node()
            test2.test_metadata = TestMetadata(
                name="not_null", kwargs={"column_name": "column" + str(i) + str(enabled)}
            )
            test2.config = TestConfig(enabled=enabled)
            tests.extend([test1, test2])

    assertSameContents(
        model.infer_primary_key(tests),
        ["column0True", "column1True", "column0False", "column1False"],
    )


def test_enabled_unique_single():
    model = model_node()
    test1 = generic_test_node()
    test1.test_metadata = TestMetadata(name="unique", kwargs={"column_name": "column1"})
    test2 = generic_test_node()
    test2.config = TestConfig(enabled=False)
    test2.test_metadata = TestMetadata(name="unique", kwargs={"column_name": "column3"})

    tests = [test1, test2]
    assertSameContents(model.infer_primary_key(tests), ["column1"])


def test_enabled_unique_multiple():
    model = model_node()
    test1 = generic_test_node()
    test1.test_metadata = TestMetadata(name="unique", kwargs={"column_name": "column1"})
    test2 = generic_test_node()
    test2.test_metadata = TestMetadata(name="unique", kwargs={"column_name": "column2 || column3"})

    tests = [test1, test2]
    assertSameContents(model.infer_primary_key(tests), ["column1", "column2 || column3"])


def test_enabled_unique_combo_single():
    model = model_node()
    test1 = generic_test_node()
    test1.test_metadata = TestMetadata(
        name="unique_combination_of_columns",
        kwargs={"combination_of_columns": ["column1", "column2"]},
    )
    test2 = generic_test_node()
    test2.config = TestConfig(enabled=False)
    test2.test_metadata = TestMetadata(
        name="unique_combination_of_columns",
        kwargs={"combination_of_columns": ["column3", "column4"]},
    )

    tests = [test1, test2]
    assertSameContents(model.infer_primary_key(tests), ["column1", "column2"])


def test_enabled_unique_combo_multiple():
    model = model_node()
    test1 = generic_test_node()
    test1.test_metadata = TestMetadata(
        name="unique", kwargs={"combination_of_columns": ["column1", "column2"]}
    )
    test2 = generic_test_node()
    test2.test_metadata = TestMetadata(
        name="unique", kwargs={"combination_of_columns": ["column3", "column4"]}
    )

    tests = [test1, test2]
    assertSameContents(
        model.infer_primary_key(tests), ["column1", "column2", "column3", "column4"]
    )


def test_disabled_unique_single():
    model = model_node()
    test1 = generic_test_node()
    test1.config = TestConfig(enabled=False)
    test1.test_metadata = TestMetadata(name="unique", kwargs={"column_name": "column1"})
    test2 = generic_test_node()
    test2.test_metadata = TestMetadata(name="not_null", kwargs={"column_name": "column2"})

    tests = [test1, test2]
    assertSameContents(model.infer_primary_key(tests), ["column1"])


def test_disabled_unique_multiple():
    model = model_node()
    test1 = generic_test_node()
    test1.config = TestConfig(enabled=False)
    test1.test_metadata = TestMetadata(name="unique", kwargs={"column_name": "column1"})
    test2 = generic_test_node()
    test2.config = TestConfig(enabled=False)
    test2.test_metadata = TestMetadata(name="unique", kwargs={"column_name": "column2 || column3"})

    tests = [test1, test2]
    assertSameContents(model.infer_primary_key(tests), ["column1", "column2 || column3"])


def test_disabled_unique_combo_single():
    model = model_node()
    test1 = generic_test_node()
    test1.config = TestConfig(enabled=False)
    test1.test_metadata = TestMetadata(
        name="unique", kwargs={"combination_of_columns": ["column1", "column2"]}
    )
    test2 = generic_test_node()
    test2.config = TestConfig(enabled=False)
    test2.test_metadata = TestMetadata(
        name="random", kwargs={"combination_of_columns": ["column3", "column4"]}
    )

    tests = [test1, test2]
    assertSameContents(model.infer_primary_key(tests), ["column1", "column2"])


def test_disabled_unique_combo_multiple():
    model = model_node()
    test1 = generic_test_node()
    test1.config = TestConfig(enabled=False)
    test1.test_metadata = TestMetadata(
        name="unique", kwargs={"combination_of_columns": ["column1", "column2"]}
    )
    test2 = generic_test_node()
    test2.config = TestConfig(enabled=False)
    test2.test_metadata = TestMetadata(
        name="unique", kwargs={"combination_of_columns": ["column3", "column4"]}
    )

    tests = [test1, test2]
    assertSameContents(
        model.infer_primary_key(tests), ["column1", "column2", "column3", "column4"]
    )


def assertSameContents(list1, list2):
    assert sorted(list1) == sorted(list2)


class TestParsedNode:
    @pytest.fixture(scope="class")
    def parsed_node(self) -> ParsedNode:
        return ParsedNode(
            resource_type=NodeType.Model,
            unique_id="model.test_package.test_name",
            name="test_name",
            package_name="test_package",
            schema="test_schema",
            alias="test_alias",
            fqn=["models", "test_name"],
            original_file_path="test_original_file_path",
            checksum=FileHash.from_contents("checksum"),
            path="test_path.sql",
            database=None,
        )

    @pytest.fixture(scope="class")
    def parsed_yml_node(self) -> ParsedNode:
        return ParsedNode(
            resource_type=NodeType.Model,
            unique_id="model.test_package.test_name",
            name="test_name",
            package_name="test_package",
            schema="test_schema",
            alias="test_alias",
            fqn=["models", "test_name"],
            original_file_path="folder/test_original_file_path.yml",
            checksum=FileHash.from_contents("checksum"),
            path="test_original_file_path.yml/test_path.sql",
            database=None,
        )

    def test_get_target_write_path(self, parsed_node):
        write_path = parsed_node.get_target_write_path("target_path", "subdirectory")
        assert (
            write_path
            == "target_path/subdirectory/test_package/test_original_file_path/test_path.sql"
        )

    def test_get_target_write_path_split(self, parsed_node):
        write_path = parsed_node.get_target_write_path("target_path", "subdirectory", "split")
        assert (
            write_path
            == "target_path/subdirectory/test_package/test_original_file_path/test_path/test_path_split.sql"
        )

    def test_get_target_write_path_for_yml_node(self, parsed_yml_node):
        write_path = parsed_yml_node.get_target_write_path("target_path", "subdirectory")
        assert (
            write_path
            == "target_path/subdirectory/test_package/folder/test_original_file_path_yml/test_path.sql"
        )
