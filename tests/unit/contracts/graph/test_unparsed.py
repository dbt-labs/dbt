import pickle
from abc import abstractmethod
from datetime import timedelta
from typing import Any, Dict

import pytest
from metricflow_semantic_interfaces.type_enums.conversion_calculation_type import (
    ConversionCalculationType,
)
from typing_extensions import override

from dbt.artifacts.resources import (
    ExposureType,
    FreshnessThreshold,
    MaturityType,
    Owner,
    Quoting,
    Time,
)
from dbt.artifacts.resources.types import TimePeriod
from dbt.artifacts.schemas.results import FreshnessStatus
from dbt.contracts.graph.unparsed import (
    Docs,
    HasColumnTests,
    UnparsedColumn,
    UnparsedConversionTypeParams,
    UnparsedDerivedDimensionV2,
    UnparsedDocumentationFile,
    UnparsedExposure,
    UnparsedMacro,
    UnparsedMetric,
    UnparsedMetricInputMeasure,
    UnparsedMetricTypeParams,
    UnparsedMetricV2,
    UnparsedModelUpdate,
    UnparsedNode,
    UnparsedNodeUpdate,
    UnparsedRunHook,
    UnparsedSemanticModelConfig,
    UnparsedSourceDefinition,
    UnparsedSourceTableDefinition,
    UnparsedVersion,
)
from dbt.exceptions import ParsingError
from dbt.node_types import NodeType
from dbt.parser.schemas import ParserRef
from tests.unit.utils import ContractTestCase


class TestUnparsedMacro(ContractTestCase):
    ContractType = UnparsedMacro

    def test_ok(self):
        macro_dict = {
            "path": "/root/path.sql",
            "original_file_path": "/root/path.sql",
            "package_name": "test",
            "language": "sql",
            "raw_code": "{% macro foo() %}select 1 as id{% endmacro %}",
            "resource_type": "macro",
        }
        macro = self.ContractType(
            path="/root/path.sql",
            original_file_path="/root/path.sql",
            package_name="test",
            language="sql",
            raw_code="{% macro foo() %}select 1 as id{% endmacro %}",
            resource_type=NodeType.Macro,
        )
        self.assert_symmetric(macro, macro_dict)
        pickle.loads(pickle.dumps(macro))

    def test_invalid_missing_field(self):
        macro_dict = {
            "path": "/root/path.sql",
            "original_file_path": "/root/path.sql",
            # 'package_name': 'test',
            "language": "sql",
            "raw_code": "{% macro foo() %}select 1 as id{% endmacro %}",
            "resource_type": "macro",
        }
        self.assert_fails_validation(macro_dict)

    def test_invalid_extra_field(self):
        macro_dict = {
            "path": "/root/path.sql",
            "original_file_path": "/root/path.sql",
            "package_name": "test",
            "language": "sql",
            "raw_code": "{% macro foo() %}select 1 as id{% endmacro %}",
            "extra": "extra",
            "resource_type": "macro",
        }
        self.assert_fails_validation(macro_dict)


class TestUnparsedNode(ContractTestCase):
    ContractType = UnparsedNode

    def test_ok(self):
        node_dict = {
            "name": "foo",
            "resource_type": NodeType.Model,
            "path": "/root/x/path.sql",
            "original_file_path": "/root/path.sql",
            "package_name": "test",
            "language": "sql",
            "raw_code": 'select * from {{ ref("thing") }}',
        }
        node = self.ContractType(
            package_name="test",
            path="/root/x/path.sql",
            original_file_path="/root/path.sql",
            language="sql",
            raw_code='select * from {{ ref("thing") }}',
            name="foo",
            resource_type=NodeType.Model,
        )
        self.assert_symmetric(node, node_dict)
        self.assertFalse(node.empty)

        self.assert_fails_validation(node_dict, cls=UnparsedRunHook)
        self.assert_fails_validation(node_dict, cls=UnparsedMacro)
        pickle.loads(pickle.dumps(node))

    def test_empty(self):
        node_dict = {
            "name": "foo",
            "resource_type": NodeType.Model,
            "path": "/root/x/path.sql",
            "original_file_path": "/root/path.sql",
            "package_name": "test",
            "language": "sql",
            "raw_code": "  \n",
        }
        node = UnparsedNode(
            package_name="test",
            path="/root/x/path.sql",
            original_file_path="/root/path.sql",
            language="sql",
            raw_code="  \n",
            name="foo",
            resource_type=NodeType.Model,
        )
        self.assert_symmetric(node, node_dict)
        self.assertTrue(node.empty)

        self.assert_fails_validation(node_dict, cls=UnparsedRunHook)
        self.assert_fails_validation(node_dict, cls=UnparsedMacro)


class TestUnparsedRunHook(ContractTestCase):
    ContractType = UnparsedRunHook

    def test_ok(self):
        node_dict = {
            "name": "foo",
            "resource_type": NodeType.Operation,
            "path": "/root/dbt_project.yml",
            "original_file_path": "/root/dbt_project.yml",
            "package_name": "test",
            "language": "sql",
            "raw_code": "GRANT select on dbt_postgres",
            "index": 4,
        }
        node = self.ContractType(
            package_name="test",
            path="/root/dbt_project.yml",
            original_file_path="/root/dbt_project.yml",
            language="sql",
            raw_code="GRANT select on dbt_postgres",
            name="foo",
            resource_type=NodeType.Operation,
            index=4,
        )
        self.assert_symmetric(node, node_dict)
        self.assert_fails_validation(node_dict, cls=UnparsedNode)
        pickle.loads(pickle.dumps(node))

    def test_bad_type(self):
        node_dict = {
            "name": "foo",
            "resource_type": NodeType.Model,  # invalid
            "path": "/root/dbt_project.yml",
            "original_file_path": "/root/dbt_project.yml",
            "package_name": "test",
            "language": "sql",
            "raw_code": "GRANT select on dbt_postgres",
            "index": 4,
        }
        self.assert_fails_validation(node_dict)


class TestFreshnessThreshold(ContractTestCase):
    ContractType = FreshnessThreshold

    def test_empty(self):
        empty = self.ContractType()
        self.assert_symmetric(empty, {"error_after": {}, "warn_after": {}})
        self.assertEqual(empty.status(float("Inf")), FreshnessStatus.Pass)
        self.assertEqual(empty.status(0), FreshnessStatus.Pass)

    def test_both(self):
        threshold = self.ContractType(
            warn_after=Time(count=18, period=TimePeriod.hour),
            error_after=Time(count=2, period=TimePeriod.day),
        )
        dct = {
            "error_after": {"count": 2, "period": "day"},
            "warn_after": {"count": 18, "period": "hour"},
        }
        self.assert_symmetric(threshold, dct)

        error_seconds = timedelta(days=3).total_seconds()
        warn_seconds = timedelta(days=1).total_seconds()
        pass_seconds = timedelta(hours=3).total_seconds()
        self.assertEqual(threshold.status(error_seconds), FreshnessStatus.Error)
        self.assertEqual(threshold.status(warn_seconds), FreshnessStatus.Warn)
        self.assertEqual(threshold.status(pass_seconds), FreshnessStatus.Pass)
        pickle.loads(pickle.dumps(threshold))

    def test_merged(self):
        t1 = self.ContractType(
            warn_after=Time(count=36, period=TimePeriod.hour),
            error_after=Time(count=2, period=TimePeriod.day),
        )
        t2 = self.ContractType(
            warn_after=Time(count=18, period=TimePeriod.hour),
        )
        threshold = self.ContractType(
            warn_after=Time(count=18, period=TimePeriod.hour),
            error_after=Time(count=None, period=None),
        )
        self.assertEqual(threshold, t1.merged(t2))

        warn_seconds = timedelta(days=1).total_seconds()
        pass_seconds = timedelta(hours=3).total_seconds()
        self.assertEqual(threshold.status(warn_seconds), FreshnessStatus.Warn)
        self.assertEqual(threshold.status(pass_seconds), FreshnessStatus.Pass)


class TestQuoting(ContractTestCase):
    ContractType = Quoting

    def test_empty(self):
        empty = self.ContractType()
        self.assert_symmetric(empty, {})

    def test_partial(self):
        a = self.ContractType(None, True, False)
        b = self.ContractType(True, False, None)
        self.assert_symmetric(a, {"schema": True, "identifier": False})
        self.assert_symmetric(b, {"database": True, "schema": False})

        c = a.merged(b)
        self.assertEqual(c, self.ContractType(True, False, False))
        self.assert_symmetric(c, {"database": True, "schema": False, "identifier": False})
        pickle.loads(pickle.dumps(c))


class TestUnparsedSourceDefinition(ContractTestCase):
    ContractType = UnparsedSourceDefinition

    def test_defaults(self):
        minimum = self.ContractType(name="foo")
        from_dict = {"name": "foo"}
        to_dict = {
            "name": "foo",
            "description": "",
            "freshness": {"error_after": {}, "warn_after": {}},
            "quoting": {},
            "tables": [],
            "loader": "",
            "meta": {},
            "tags": [],
            "config": {},
        }
        self.assert_from_dict(minimum, from_dict)
        self.assert_to_dict(minimum, to_dict)

    def test_contents(self):
        empty = self.ContractType(
            name="foo",
            description="a description",
            quoting=Quoting(database=False),
            loader="some_loader",
            freshness=FreshnessThreshold(),
            tables=[],
            meta={},
        )
        dct = {
            "name": "foo",
            "description": "a description",
            "quoting": {"database": False},
            "loader": "some_loader",
            "freshness": {"error_after": {}, "warn_after": {}},
            "tables": [],
            "meta": {},
            "tags": [],
            "config": {},
        }
        self.assert_symmetric(empty, dct)

    def test_table_defaults(self):
        table_1 = UnparsedSourceTableDefinition(name="table1")
        table_2 = UnparsedSourceTableDefinition(
            name="table2",
            description="table 2",
            quoting=Quoting(database=True),
        )
        source = self.ContractType(name="foo", tables=[table_1, table_2])
        from_dict = {
            "name": "foo",
            "tables": [
                {"name": "table1"},
                {
                    "name": "table2",
                    "description": "table 2",
                    "quoting": {"database": True},
                },
            ],
        }
        to_dict = {
            "name": "foo",
            "description": "",
            "config": {},
            "loader": "",
            "freshness": {"error_after": {}, "warn_after": {}},
            "quoting": {},
            "meta": {},
            "tables": [
                {
                    "name": "table1",
                    "description": "",
                    "config": {},
                    "docs": {"show": True},
                    "data_tests": [],
                    "tests": [],
                    "columns": [],
                    "constraints": [],
                    "quoting": {},
                    "freshness": {"error_after": {}, "warn_after": {}},
                    "meta": {},
                    "tags": [],
                },
                {
                    "name": "table2",
                    "description": "table 2",
                    "config": {},
                    "docs": {"show": True},
                    "data_tests": [],
                    "tests": [],
                    "columns": [],
                    "constraints": [],
                    "quoting": {"database": True},
                    "freshness": {"error_after": {}, "warn_after": {}},
                    "meta": {},
                    "tags": [],
                },
            ],
            "tags": [],
        }
        self.assert_from_dict(source, from_dict)
        self.assert_symmetric(source, to_dict)
        pickle.loads(pickle.dumps(source))


class TestUnparsedDocumentationFile(ContractTestCase):
    ContractType = UnparsedDocumentationFile

    def test_ok(self):
        doc = self.ContractType(
            package_name="test",
            path="/root/docs",
            original_file_path="/root/docs/doc.md",
            file_contents="blah blah blah",
        )
        doc_dict = {
            "package_name": "test",
            "path": "/root/docs",
            "original_file_path": "/root/docs/doc.md",
            "file_contents": "blah blah blah",
        }
        self.assert_symmetric(doc, doc_dict)
        self.assertEqual(doc.resource_type, NodeType.Documentation)
        self.assert_fails_validation(doc_dict, UnparsedNode)
        pickle.loads(pickle.dumps(doc))

    def test_extra_field(self):
        self.assert_fails_validation({})
        doc_dict = {
            "package_name": "test",
            "path": "/root/docs",
            "original_file_path": "/root/docs/doc.md",
            "file_contents": "blah blah blah",
            "resource_type": "docs",
        }
        self.assert_fails_validation(doc_dict)


class TestUnparsedNodeUpdate(ContractTestCase):
    ContractType = UnparsedNodeUpdate

    def test_defaults(self):
        minimum = self.ContractType(
            name="foo",
            yaml_key="models",
            original_file_path="/some/fake/path",
            package_name="test",
        )
        from_dict = {
            "name": "foo",
            "yaml_key": "models",
            "original_file_path": "/some/fake/path",
            "package_name": "test",
        }
        to_dict = {
            "name": "foo",
            "yaml_key": "models",
            "original_file_path": "/some/fake/path",
            "package_name": "test",
            "columns": [],
            "description": "",
            "docs": {"show": True},
            "data_tests": [],
            "tests": [],
            "meta": {},
            "config": {},
            "constraints": [],
        }
        self.assert_from_dict(minimum, from_dict)
        self.assert_to_dict(minimum, to_dict)

    def test_contents(self):
        update = self.ContractType(
            name="foo",
            yaml_key="models",
            original_file_path="/some/fake/path",
            package_name="test",
            description="a description",
            data_tests=["table_test"],
            meta={"key": ["value1", "value2"]},
            columns=[
                UnparsedColumn(
                    name="x",
                    description="x description",
                    meta={"key2": "value3"},
                ),
                UnparsedColumn(
                    name="y",
                    description="y description",
                    data_tests=["unique", {"accepted_values": {"values": ["blue", "green"]}}],
                    meta={},
                    tags=["a", "b"],
                ),
            ],
            docs=Docs(show=False),
        )
        dct = {
            "name": "foo",
            "yaml_key": "models",
            "original_file_path": "/some/fake/path",
            "package_name": "test",
            "description": "a description",
            "data_tests": ["table_test"],
            "tests": [],
            "meta": {"key": ["value1", "value2"]},
            "constraints": [],
            "columns": [
                {
                    "name": "x",
                    "description": "x description",
                    "docs": {"show": True},
                    "data_tests": [],
                    "tests": [],
                    "meta": {"key2": "value3"},
                    "tags": [],
                    "constraints": [],
                    "config": {},
                },
                {
                    "name": "y",
                    "description": "y description",
                    "docs": {"show": True},
                    "data_tests": ["unique", {"accepted_values": {"values": ["blue", "green"]}}],
                    "tests": [],
                    "meta": {},
                    "tags": ["a", "b"],
                    "constraints": [],
                    "config": {},
                },
            ],
            "docs": {"show": False},
            "config": {},
        }
        self.assert_symmetric(update, dct)
        pickle.loads(pickle.dumps(update))

    def test_bad_test_type(self):
        dct = {
            "name": "foo",
            "yaml_key": "models",
            "original_file_path": "/some/fake/path",
            "package_name": "test",
            "description": "a description",
            "data_tests": ["table_test"],
            "tests": [],
            "meta": {"key": ["value1", "value2"]},
            "columns": [
                {
                    "name": "x",
                    "description": "x description",
                    "docs": {"show": True},
                    "data_tests": [],
                    "tests": [],
                    "meta": {"key2": "value3"},
                },
                {
                    "name": "y",
                    "description": "y description",
                    "docs": {"show": True},
                    "data_tests": [100, {"accepted_values": {"values": ["blue", "green"]}}],
                    "tests": [],
                    "meta": {},
                    "yaml_key": "models",
                    "original_file_path": "/some/fake/path",
                },
            ],
            "docs": {"show": True},
        }
        self.assert_fails_validation(dct)

        dct = {
            "name": "foo",
            "yaml_key": "models",
            "original_file_path": "/some/fake/path",
            "package_name": "test",
            "description": "a description",
            "data_tests": ["table_test"],
            "tests": [],
            "meta": {"key": ["value1", "value2"]},
            "columns": [
                # column missing a name
                {
                    "description": "x description",
                    "docs": {"show": True},
                    "data_tests": [],
                    "tests": [],
                    "meta": {"key2": "value3"},
                },
                {
                    "name": "y",
                    "description": "y description",
                    "docs": {"show": True},
                    "data_tests": ["unique", {"accepted_values": {"values": ["blue", "green"]}}],
                    "tests": [],
                    "meta": {},
                    "yaml_key": "models",
                    "original_file_path": "/some/fake/path",
                },
            ],
            "docs": {"show": True},
        }
        self.assert_fails_validation(dct)

        # missing a name
        dct = {
            "yaml_key": "models",
            "original_file_path": "/some/fake/path",
            "package_name": "test",
            "description": "a description",
            "data_tests": ["table_test"],
            "tests": [],
            "meta": {"key": ["value1", "value2"]},
            "columns": [
                {
                    "name": "x",
                    "description": "x description",
                    "docs": {"show": True},
                    "data_tests": [],
                    "tests": [],
                    "meta": {"key2": "value3"},
                },
                {
                    "name": "y",
                    "description": "y description",
                    "docs": {"show": True},
                    "data_tests": ["unique", {"accepted_values": {"values": ["blue", "green"]}}],
                    "tests": [],
                    "meta": {},
                    "yaml_key": "models",
                    "original_file_path": "/some/fake/path",
                },
            ],
            "docs": {"show": True},
        }
        self.assert_fails_validation(dct)


class TestUnparsedModelUpdate(ContractTestCase):
    ContractType = UnparsedModelUpdate

    def test_defaults(self):
        minimum = self.ContractType(
            name="foo",
            yaml_key="models",
            original_file_path="/some/fake/path",
            package_name="test",
        )
        from_dict = {
            "name": "foo",
            "yaml_key": "models",
            "original_file_path": "/some/fake/path",
            "package_name": "test",
        }
        to_dict = {
            "name": "foo",
            "yaml_key": "models",
            "original_file_path": "/some/fake/path",
            "package_name": "test",
            "columns": [],
            "description": "",
            "docs": {"show": True},
            "data_tests": [],
            "tests": [],
            "meta": {},
            "config": {},
            "constraints": [],
            "versions": [],
        }
        self.assert_from_dict(minimum, from_dict)
        self.assert_to_dict(minimum, to_dict)

    def test_contents(self):
        update = self.ContractType(
            name="foo",
            yaml_key="models",
            original_file_path="/some/fake/path",
            package_name="test",
            description="a description",
            data_tests=["table_test"],
            meta={"key": ["value1", "value2"]},
            columns=[
                UnparsedColumn(
                    name="x",
                    description="x description",
                    meta={"key2": "value3"},
                ),
                UnparsedColumn(
                    name="y",
                    description="y description",
                    data_tests=["unique", {"accepted_values": {"values": ["blue", "green"]}}],
                    meta={},
                    tags=["a", "b"],
                ),
            ],
            docs=Docs(show=False),
            versions=[UnparsedVersion(v=2)],
        )
        dct = {
            "name": "foo",
            "yaml_key": "models",
            "original_file_path": "/some/fake/path",
            "package_name": "test",
            "description": "a description",
            "data_tests": ["table_test"],
            "tests": [],
            "meta": {"key": ["value1", "value2"]},
            "constraints": [],
            "versions": [
                {
                    "v": 2,
                    "description": "",
                    "columns": [],
                    "config": {},
                    "constraints": [],
                    "docs": {"show": True},
                }
            ],
            "columns": [
                {
                    "name": "x",
                    "description": "x description",
                    "docs": {"show": True},
                    "data_tests": [],
                    "tests": [],
                    "meta": {"key2": "value3"},
                    "tags": [],
                    "constraints": [],
                    "config": {},
                },
                {
                    "name": "y",
                    "description": "y description",
                    "docs": {"show": True},
                    "data_tests": ["unique", {"accepted_values": {"values": ["blue", "green"]}}],
                    "tests": [],
                    "meta": {},
                    "tags": ["a", "b"],
                    "constraints": [],
                    "config": {},
                },
            ],
            "docs": {"show": False},
            "config": {},
        }
        self.assert_symmetric(update, dct)
        pickle.loads(pickle.dumps(update))

    def test_bad_test_type(self):
        dct = {
            "name": "foo",
            "yaml_key": "models",
            "original_file_path": "/some/fake/path",
            "package_name": "test",
            "description": "a description",
            "data_tests": ["table_test"],
            "tests": [],
            "meta": {"key": ["value1", "value2"]},
            "columns": [
                {
                    "name": "x",
                    "description": "x description",
                    "docs": {"show": True},
                    "data_tests": [],
                    "tests": [],
                    "meta": {"key2": "value3"},
                },
                {
                    "name": "y",
                    "description": "y description",
                    "docs": {"show": True},
                    "data_tests": [100, {"accepted_values": {"values": ["blue", "green"]}}],
                    "tests": [],
                    "meta": {},
                    "yaml_key": "models",
                    "original_file_path": "/some/fake/path",
                },
            ],
            "docs": {"show": True},
        }
        self.assert_fails_validation(dct)

        dct = {
            "name": "foo",
            "yaml_key": "models",
            "original_file_path": "/some/fake/path",
            "package_name": "test",
            "description": "a description",
            "data_tests": ["table_test"],
            "tests": [],
            "meta": {"key": ["value1", "value2"]},
            "columns": [
                # column missing a name
                {
                    "description": "x description",
                    "docs": {"show": True},
                    "data_tests": [],
                    "tests": [],
                    "meta": {"key2": "value3"},
                },
                {
                    "name": "y",
                    "description": "y description",
                    "docs": {"show": True},
                    "data_tests": ["unique", {"accepted_values": {"values": ["blue", "green"]}}],
                    "tests": [],
                    "meta": {},
                    "yaml_key": "models",
                    "original_file_path": "/some/fake/path",
                },
            ],
            "docs": {"show": True},
        }
        self.assert_fails_validation(dct)

        # missing a name
        dct = {
            "yaml_key": "models",
            "original_file_path": "/some/fake/path",
            "package_name": "test",
            "description": "a description",
            "data_tests": ["table_test"],
            "tests": [],
            "meta": {"key": ["value1", "value2"]},
            "columns": [
                {
                    "name": "x",
                    "description": "x description",
                    "docs": {"show": True},
                    "data_tests": [],
                    "tests": [],
                    "meta": {"key2": "value3"},
                },
                {
                    "name": "y",
                    "description": "y description",
                    "docs": {"show": True},
                    "data_tests": ["unique", {"accepted_values": {"values": ["blue", "green"]}}],
                    "tests": [],
                    "meta": {},
                    "yaml_key": "models",
                    "original_file_path": "/some/fake/path",
                },
            ],
            "docs": {"show": True},
        }
        self.assert_fails_validation(dct)

    def test_int_and_float_versions_are_distinct(self):
        # mashumaro <3.15 deserializes both `v: 4` and `v: 4.1` as int 4,
        # which UnparsedModelUpdate treats as a duplicate version (#16284).
        model = self.ContractType.from_dict(
            {
                "name": "foo",
                "yaml_key": "models",
                "original_file_path": "/some/fake/path",
                "package_name": "test",
                "versions": [{"v": 4}, {"v": 4.1}],
            }
        )
        self.assertEqual([version.v for version in model.versions], [4, 4.1])


class TestUnparsedSemanticModelConfig(ContractTestCase):
    ContractType = UnparsedSemanticModelConfig

    def test_valid_config_passes(self):
        self.ContractType.validate({"name": "purchases", "enabled": True})

    def test_extra_field_gives_clear_error(self):
        self.assert_fails_validation_with_message(
            {"enabled": True, "name": "purchases", "description": "my semantic model"},
            "Unknown field(s) in semantic_model config: description",
        )

    def test_extra_field_error_lists_valid_fields(self):
        self.assert_fails_validation_with_message(
            {"enabled": True, "unexpected_key": "value"},
            "Valid fields are:",
        )

    def test_multiple_extra_fields_all_listed(self):
        self.assert_fails_validation_with_message(
            {"enabled": True, "description": "oops", "label": "also wrong"},
            "Unknown field(s) in semantic_model config: description, label",
        )


class TestUnparsedExposure(ContractTestCase):
    ContractType = UnparsedExposure

    def get_ok_dict(self):
        return {
            "name": "my_exposure",
            "type": "dashboard",
            "owner": {"name": "example", "email": "name@example.com", "slack": "#channel"},
            "maturity": "medium",
            "meta": {"tool": "my_tool"},
            "tags": ["my_department"],
            "url": "https://example.com/dashboards/1",
            "description": "A exposure",
            "config": {},
            "depends_on": [
                'ref("my_model")',
                'source("raw", "source_table")',
            ],
        }

    def test_ok(self):
        exposure = self.ContractType(
            name="my_exposure",
            type=ExposureType.Dashboard,
            owner=Owner(name="example", email="name@example.com", _extra={"slack": "#channel"}),
            maturity=MaturityType.Medium,
            url="https://example.com/dashboards/1",
            description="A exposure",
            config={},
            meta={"tool": "my_tool"},
            tags=["my_department"],
            depends_on=['ref("my_model")', 'source("raw", "source_table")'],
        )
        dct = self.get_ok_dict()
        self.assert_symmetric(exposure, dct)
        pickle.loads(pickle.dumps(exposure))

    def test_ok_exposures(self):
        for exposure_allowed in ("dashboard", "notebook", "analysis", "ml", "application"):
            tst = self.get_ok_dict()
            tst["type"] = exposure_allowed
            assert self.ContractType.from_dict(tst).type == exposure_allowed

    def test_bad_exposure(self):
        # bad exposure: None isn't allowed
        for exposure_not_allowed in (None, "not an exposure"):
            tst = self.get_ok_dict()
            tst["type"] = exposure_not_allowed
            self.assert_fails_validation(tst)

    def test_no_exposure(self):
        tst = self.get_ok_dict()
        del tst["type"]
        self.assert_fails_validation(tst)

    def test_ok_maturities(self):
        for maturity_allowed in (None, "low", "medium", "high"):
            tst = self.get_ok_dict()
            tst["maturity"] = maturity_allowed
            assert self.ContractType.from_dict(tst).maturity == maturity_allowed

        tst = self.get_ok_dict()
        del tst["maturity"]
        assert self.ContractType.from_dict(tst).maturity is None

    def test_bad_maturity(self):
        tst = self.get_ok_dict()
        tst["maturity"] = "invalid maturity"
        self.assert_fails_validation(tst)

    def test_bad_owner_missing_things(self):
        tst = self.get_ok_dict()
        del tst["owner"]["email"]
        del tst["owner"]["name"]
        self.assert_fails_validation(tst)

        del tst["owner"]
        self.assert_fails_validation(tst)

    def test_bad_tags(self):
        tst = self.get_ok_dict()
        tst["tags"] = [123]
        self.assert_fails_validation(tst)


class TestUnparsedConversionTypeParams(ContractTestCase):
    """Only Applies to v1 Semantic Metrics."""

    ContractType = UnparsedConversionTypeParams

    def get_old_style_ok_dict(self):
        return {
            "entity": "customers",
            "base_measure": {
                "name": "customers",
                "filter": "is_new = true",
                "join_to_timespine": False,
            },
            "conversion_measure": "orders",
            "calculation": "conversion_rate",
            "window": "7d",
        }

    def test_old_style_ok(self):
        params = self.ContractType.from_dict(self.get_old_style_ok_dict())
        assert params.base_measure is not None
        assert params.conversion_measure is not None
        assert params.calculation == ConversionCalculationType.CONVERSION_RATE.value
        assert params.window == "7d"

    def test_old_style_bad_no_base_measure(self):
        tst = self.get_old_style_ok_dict()
        del tst["base_measure"]
        self.assert_fails_validation(tst)

    def test_old_style_bad_no_conversion_measure(self):
        tst = self.get_old_style_ok_dict()
        del tst["conversion_measure"]
        self.assert_fails_validation(tst)


class BaseTestUnparsedMetric:

    @abstractmethod
    def get_ok_dict(self) -> Dict[str, Any]:
        raise NotImplementedError()

    def test_bad_tags(self):
        tst = self.get_ok_dict()
        tst["tags"] = [123]
        self.assert_fails_validation(tst)

    def test_bad_metric_name_with_spaces(self):
        tst = self.get_ok_dict()
        tst["name"] = "metric name with spaces"
        self.assert_fails_validation(tst)

    def test_bad_metric_name_too_long(self):
        tst = self.get_ok_dict()
        tst["name"] = "a" * 251
        self.assert_fails_validation(tst)

    def test_bad_metric_name_does_not_start_with_letter(self):
        tst = self.get_ok_dict()
        tst["name"] = "123metric"
        self.assert_fails_validation(tst)

        tst["name"] = "_metric"
        self.assert_fails_validation(tst)

    def test_bad_metric_name_contains_special_characters(self):
        tst = self.get_ok_dict()
        tst["name"] = "metric!name"
        self.assert_fails_validation(tst)

        tst["name"] = "metric@name"
        self.assert_fails_validation(tst)

        tst["name"] = "metric#name"
        self.assert_fails_validation(tst)

        tst["name"] = "metric$name"
        self.assert_fails_validation(tst)

        tst["name"] = "metric-name"
        self.assert_fails_validation(tst)


class TestUnparsedMetric(BaseTestUnparsedMetric, ContractTestCase):
    ContractType = UnparsedMetric

    @override
    def get_ok_dict(self):
        return {
            "name": "new_customers",
            "label": "New Customers",
            "description": "New customers",
            "type": "simple",
            "type_params": {
                "measure": {
                    "name": "customers",
                    "filter": "is_new = true",
                    "join_to_timespine": False,
                },
            },
            "config": {},
            "tags": [],
            "meta": {"is_okr": True},
        }

    def test_ok(self):
        metric = self.ContractType(
            name="new_customers",
            label="New Customers",
            description="New customers",
            type="simple",
            type_params=UnparsedMetricTypeParams(
                measure=UnparsedMetricInputMeasure(
                    name="customers",
                    filter="is_new = true",
                )
            ),
            config={},
            meta={"is_okr": True},
        )
        dct = self.get_ok_dict()
        self.assert_symmetric(metric, dct)
        pickle.loads(pickle.dumps(metric))

    def test_bad_metric_no_type_params(self):
        tst = self.get_ok_dict()
        del tst["type_params"]
        self.assert_fails_validation(tst)


class TestUnparsedMetricV2(BaseTestUnparsedMetric, ContractTestCase):
    ContractType = UnparsedMetricV2

    @override
    def get_ok_dict(self):
        return {
            "name": "new_customers",
            "label": "New Customers",
            "description": "New customers",
            "type": "simple",
            "agg": "sum",
            "filter": "is_new = true",
            "join_to_timespine": False,
            "config": {
                "tags": [],
                "meta": {"is_okr": True},
            },
        }

    def get_ok_dict_with_defaults(self):
        dct = self.get_ok_dict()
        dct["hidden"] = False
        dct["period_agg"] = "first"
        return dct

    def test_ok(self):
        metric = self.ContractType(
            name="new_customers",
            label="New Customers",
            description="New customers",
            agg="sum",
            filter="is_new = true",
            join_to_timespine=False,
            config={
                "tags": [],
                "meta": {"is_okr": True},
            },
        )
        dct = self.get_ok_dict()
        # add defaults:
        dct["hidden"] = False
        dct["period_agg"] = "first"
        self.assert_symmetric(metric, dct)
        pickle.loads(pickle.dumps(metric))

    def test_simple_metric_with_no_agg_fails_validation(self):
        tst = self.get_ok_dict_with_defaults()
        del tst["agg"]
        self.assert_fails_validation(tst)


class TestUnparsedVersion(ContractTestCase):
    ContractType = UnparsedVersion

    def get_ok_dict(self):
        return {
            "v": 2,
            "defined_in": "test_defined_in",
            "description": "A version",
            "config": {},
            "constraints": [],
            "docs": {"show": False},
            "data_tests": [],
            "columns": [],
        }

    def test_ok(self):
        version = self.ContractType(
            v=2,
            defined_in="test_defined_in",
            description="A version",
            config={},
            constraints=[],
            docs=Docs(show=False),
            data_tests=[],
            columns=[],
        )
        dct = self.get_ok_dict()
        self.assert_symmetric(version, dct)
        pickle.loads(pickle.dumps(version))

    def test_bad_version_no_v(self):
        version = self.get_ok_dict()
        del version["v"]
        self.assert_fails_validation(version)

    def test_float_v_not_truncated(self):
        # Regression for #16284: mashumaro <3.15 truncates Union[int, float, str]
        # so YAML `v: 4.1` becomes int 4.
        version = self.ContractType.from_dict({"v": 4.1})
        self.assertEqual(version.v, 4.1)
        self.assertIsInstance(version.v, float)


@pytest.mark.parametrize(
    "left,right,expected_lt",
    [
        # same types
        (2, 12, True),
        (12, 2, False),
        ("a", "b", True),
        ("b", "a", False),
        # mismatched types - numeric
        (2, 12.0, True),
        (12.0, 2, False),
        (2, "12", True),
        ("12", 2, False),
        # mismatched types
        (1, "test", True),
        ("test", 1, False),
    ],
)
def test_unparsed_version_lt(left, right, expected_lt):
    assert (UnparsedVersion(left) < UnparsedVersion(right)) == expected_lt


def test_column_parse():
    unparsed_col = HasColumnTests(
        columns=[UnparsedColumn(name="TestCol", constraints=[{"type": "!INVALID!"}])]
    )

    with pytest.raises(ParsingError):
        ParserRef.from_target(unparsed_col)


class TestUnparsedColumnTimeDimensionGranularityValidation(ContractTestCase):
    """Test validation that SL YAML V2 column with time dimension must specify granularity,
    and that dimension validity_params require granularity."""

    ContractType = UnparsedColumn

    def test_time_dimension_without_granularity_fails_validation(self):
        column_dict = {
            "name": "created_at",
            "dimension": {"type": "time", "name": "created_at_dim"},
        }
        self.assert_fails_validation(column_dict)

    def test_time_dimension_with_granularity_passes_validation(self):
        column_dict = {
            "name": "created_at",
            "granularity": "day",
            "dimension": {"type": "time", "name": "created_at_dim"},
        }
        col = self.ContractType.from_dict(column_dict)
        self.assertEqual(col.granularity, "day")
        self.assertEqual(col.name, "created_at")

    def test_time_dimension_string_without_granularity_fails_validation(self):
        """Dimension as string 'time' without granularity must fail."""
        column_dict = {
            "name": "created_at",
            "dimension": "time",
        }
        self.assert_fails_validation(column_dict)

    def test_time_dimension_string_with_granularity_passes_validation(self):
        """Dimension as string 'time' with granularity must pass."""
        column_dict = {
            "name": "created_at",
            "granularity": "day",
            "dimension": "time",
        }
        col = self.ContractType.from_dict(column_dict)
        self.assertEqual(col.granularity, "day")
        self.assertEqual(col.name, "created_at")

    def test_non_time_dimension_string_passes_without_granularity(self):
        """Dimension as string (e.g. categorical) does not require granularity."""
        column_dict = {
            "name": "category",
            "dimension": "categorical",
        }
        col = self.ContractType.from_dict(column_dict)
        self.assertEqual(col.name, "category")
        self.assertIsNone(col.granularity)

    def test_dimension_with_validity_params_without_granularity_fails_validation(self):
        """Dimension (dict) with validity_params must have column granularity."""
        column_dict = {
            "name": "valid_from",
            "dimension": {
                "type": "time",
                "name": "valid_from_dim",
                "validity_params": {"is_start": True, "is_end": False},
            },
        }
        self.assert_fails_validation(column_dict)

    def test_dimension_with_validity_params_with_granularity_passes_validation(self):
        """Dimension (dict) with validity_params and granularity passes."""
        column_dict = {
            "name": "valid_from",
            "granularity": "day",
            "dimension": {
                "type": "time",
                "name": "valid_from_dim",
                "validity_params": {"is_start": True, "is_end": True},
            },
        }
        col = self.ContractType.from_dict(column_dict)
        self.assertEqual(col.granularity, "day")
        self.assertEqual(col.name, "valid_from")

    def test_dimension_without_validity_params_passes_without_granularity_when_not_time(self):
        """Dimension (dict) without validity_params and not time does not require granularity."""
        column_dict = {
            "name": "category",
            "dimension": {"type": "categorical", "name": "category_dim"},
        }
        col = self.ContractType.from_dict(column_dict)
        self.assertEqual(col.name, "category")
        self.assertIsNone(col.granularity)


class TestUnparsedDerivedDimensionV2ValidityParamsValidation(ContractTestCase):
    """Test validation that UnparsedDerivedDimensionV2 only has validity_params when it has granularity."""

    ContractType = UnparsedDerivedDimensionV2

    def test_derived_dimension_with_validity_params_without_granularity_fails_validation(self):
        """Derived dimension with validity_params must have granularity."""
        dimension_dict = {
            "type": "time",
            "name": "valid_from_dim",
            "expr": "valid_from",
            "validity_params": {"is_start": True, "is_end": False},
        }
        self.assert_fails_validation(dimension_dict)

    def test_derived_dimension_with_validity_params_with_granularity_passes_validation(self):
        """Derived dimension with validity_params and granularity passes."""
        dimension_dict = {
            "type": "time",
            "name": "valid_from_dim",
            "expr": "valid_from",
            "granularity": "day",
            "validity_params": {"is_start": True, "is_end": True},
        }
        dim = self.ContractType.from_dict(dimension_dict)
        self.assertEqual(dim.granularity, "day")
        self.assertEqual(dim.name, "valid_from_dim")

    def test_derived_dimension_without_validity_params_passes_without_granularity(self):
        """Derived dimension without validity_params does not require granularity."""
        dimension_dict = {
            "type": "time",
            "name": "some_dim",
            "expr": "some_column",
        }
        dim = self.ContractType.from_dict(dimension_dict)
        self.assertEqual(dim.name, "some_dim")
        self.assertIsNone(dim.granularity)
