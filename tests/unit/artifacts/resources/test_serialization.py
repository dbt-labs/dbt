import pytest
from dataclasses import dataclass
from typing import Optional
from mashumaro.exceptions import MissingField

from dbt.artifacts.resources.base import BaseResource
from dbt.artifacts.resources.types import NodeType


# Test that a (mocked) new minor version of a BaseResource (serialized with a value for
# a new optional field) can be deserialized successfully. e.g. something like
# PreviousBaseResource.from_dict(CurrentBaseResource(...).to_dict())
@dataclass
class SlimClass(BaseResource):
    my_str: str


@dataclass
class OptionalFieldClass(BaseResource):
    my_str: str
    optional_field: Optional[str] = None


@dataclass
class RequiredFieldClass(BaseResource):
    my_str: str
    new_field: str


# Test that a new minor version of a BaseResource serialized with a
# field that is now optional, but did not previously exist can be
# deserialized successfully.
def test_adding_optional_field():
    current_instance = OptionalFieldClass(
        name="test",
        resource_type=NodeType.Macro,
        package_name="awsome_package",
        path="my_path",
        original_file_path="my_file_path",
        unique_id="abc",
        my_str="test",
        optional_field="test",  # new optional field
    )

    current_instance_dict = current_instance.to_dict()
    expected_current_dict = {
        "name": "test",
        "resource_type": "macro",
        "package_name": "awsome_package",
        "path": "my_path",
        "original_file_path": "my_file_path",
        "unique_id": "abc",
        "my_str": "test",
        "optional_field": "test",
    }
    assert current_instance_dict == expected_current_dict

    expected_slim_instance = SlimClass(
        name="test",
        resource_type=NodeType.Macro,
        package_name="awsome_package",
        path="my_path",
        original_file_path="my_file_path",
        unique_id="abc",
        my_str="test",
    )
    slim_instance = SlimClass.from_dict(current_instance_dict)
    assert slim_instance == expected_slim_instance


# Test that a new minor version of a BaseResource serialized without a
# field that was previously optional can be deserialized successfully.
def test_missing_optional_field():
    current_instance = SlimClass(
        name="test",
        resource_type=NodeType.Macro,
        package_name="awsome_package",
        path="my_path",
        original_file_path="my_file_path",
        unique_id="abc",
        my_str="test",
        # optional_field="test" -> puposely excluded
    )
    current_instance_dict = current_instance.to_dict()
    expected_current_dict = {
        "name": "test",
        "resource_type": "macro",
        "package_name": "awsome_package",
        "path": "my_path",
        "original_file_path": "my_file_path",
        "unique_id": "abc",
        "my_str": "test",
    }
    assert current_instance_dict == expected_current_dict

    expected_optional_field_instance = OptionalFieldClass(
        name="test",
        resource_type=NodeType.Macro,
        package_name="awsome_package",
        path="my_path",
        original_file_path="my_file_path",
        unique_id="abc",
        my_str="test",
        optional_field=None,
    )
    slim_instance = OptionalFieldClass.from_dict(current_instance_dict)
    assert slim_instance == expected_optional_field_instance


# Test that a new minor version of a BaseResource serialized with a
# new field without a default, but did not previously exist can be
# deserialized successfully
def test_adding_required_field():
    current_instance = RequiredFieldClass(
        name="test",
        resource_type=NodeType.Macro,
        package_name="awsome_package",
        path="my_path",
        original_file_path="my_file_path",
        unique_id="abc",
        my_str="test",
        new_field="test",  # new required field
    )

    current_instance_dict = current_instance.to_dict()
    expected_current_dict = {
        "name": "test",
        "resource_type": "macro",
        "package_name": "awsome_package",
        "path": "my_path",
        "original_file_path": "my_file_path",
        "unique_id": "abc",
        "my_str": "test",
        "new_field": "test",
    }
    assert current_instance_dict == expected_current_dict

    expected_slim_instance = SlimClass(
        name="test",
        resource_type=NodeType.Macro,
        package_name="awsome_package",
        path="my_path",
        original_file_path="my_file_path",
        unique_id="abc",
        my_str="test",
    )
    slim_instance = SlimClass.from_dict(current_instance_dict)
    assert slim_instance == expected_slim_instance


# Test that a new minor version of a BaseResource serialized without a
# field with no default cannot be deserialized successfully.  We don't
# want to allow removing required fields.  Expect error.
def test_removing_required_field():
    current_instance = SlimClass(
        name="test",
        resource_type=NodeType.Macro,
        package_name="awsome_package",
        path="my_path",
        original_file_path="my_file_path",
        unique_id="abc",
        my_str="test",
    )
    expecter_err = 'Field "new_field" of type str is missing in RequiredFieldClass instance'
    with pytest.raises(MissingField, match=expecter_err):
        RequiredFieldClass.from_dict(current_instance.to_dict())
