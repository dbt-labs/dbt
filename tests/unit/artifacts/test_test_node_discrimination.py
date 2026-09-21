"""SingularTest vs GenericTest share resource_type=test, so mashumaro
Union resolution can't tell them apart by type alone. SingularTest's
__pre_deserialize__ rejects payloads that carry GenericTest-only fields
so resolution falls through to GenericTest.

Without this guard, every test deserializes as SingularTest, silently
dropping test_metadata. That's latent for state comparison (which
doesn't compile tests) but breaks generic-test compile in any path that
loads a manifest from disk, including the v2 parser branch.
"""

from __future__ import annotations

import pytest

from dbt.artifacts.resources.v1.generic_test import GenericTest, TestMetadata
from dbt.artifacts.resources.v1.singular_test import SingularTest


def _generic_test_payload():
    return {
        "name": "unique_my_model_id",
        "resource_type": "test",
        "package_name": "p",
        "path": "schema.yml",
        "original_file_path": "models/schema.yml",
        "unique_id": "test.p.unique_my_model_id.abc",
        "fqn": ["p", "unique_my_model_id"],
        "alias": "unique_my_model_id",
        "checksum": {"name": "none", "checksum": ""},
        "schema": "dbt_test",
        "database": "db",
        "raw_code": "{{ test_unique(**_dbt_generic_test_kwargs) }}",
        "language": "sql",
        "refs": [],
        "sources": [],
        "metrics": [],
        "depends_on": {"macros": [], "nodes": []},
        "test_metadata": {
            "name": "unique",
            "kwargs": {"column_name": "id", "model": "{{ ref('m') }}"},
        },
    }


def _singular_test_payload():
    return {
        "name": "assert_positive_total",
        "resource_type": "test",
        "package_name": "p",
        "path": "tests/assert_positive_total.sql",
        "original_file_path": "tests/assert_positive_total.sql",
        "unique_id": "test.p.assert_positive_total",
        "fqn": ["p", "assert_positive_total"],
        "alias": "assert_positive_total",
        "checksum": {"name": "sha256", "checksum": "x"},
        "schema": "dbt_test",
        "database": "db",
        "raw_code": "select 1 where 1=0",
        "language": "sql",
        "refs": [],
        "sources": [],
        "metrics": [],
        "depends_on": {"macros": [], "nodes": []},
    }


class TestSingularTestPreDeserialize:
    def test_rejects_test_metadata(self):
        with pytest.raises(ValueError, match="generic-test fields"):
            SingularTest.__pre_deserialize__(_generic_test_payload())

    def test_rejects_column_name(self):
        with pytest.raises(ValueError, match="generic-test fields"):
            SingularTest.__pre_deserialize__({"column_name": "id"})

    def test_rejects_attached_node(self):
        with pytest.raises(ValueError, match="generic-test fields"):
            SingularTest.__pre_deserialize__({"attached_node": "model.p.m"})

    def test_accepts_singular_payload(self):
        assert SingularTest.__pre_deserialize__(_singular_test_payload())


class TestRoundTripDiscrimination:
    def test_generic_payload_round_trips_as_generic_test(self):
        node = GenericTest.from_dict(_generic_test_payload())
        assert isinstance(node, GenericTest)
        assert node.test_metadata == TestMetadata(
            name="unique", kwargs={"column_name": "id", "model": "{{ ref('m') }}"}
        )

    def test_singular_payload_round_trips_as_singular_test(self):
        node = SingularTest.from_dict(_singular_test_payload())
        assert isinstance(node, SingularTest)
