from unittest import mock

from dbt.artifacts.resources import ExternalTable, Quoting, SourceConfig
from dbt.contracts.files import SchemaSourceFile
from dbt.contracts.graph.nodes import SourceDefinition
from dbt.node_types import NodeType


def make_source(external):
    return SourceDefinition(
        columns={},
        database="db",
        description="",
        fqn=["test", "src", "tbl"],
        identifier="tbl",
        loader="",
        name="tbl",
        original_file_path="models/sources.yml",
        package_name="test",
        path="models/sources.yml",
        quoting=Quoting(),
        resource_type=NodeType.Source,
        schema="sch",
        source_description="",
        source_name="src",
        unique_id="source.test.src.tbl",
        config=SourceConfig(),
        external=external,
    )


def compare(old, new, flag):
    with mock.patch("dbt.contracts.graph.nodes.get_flags") as get_flags:
        get_flags.return_value.state_modified_compare_more_unrendered_values = flag
        return new.same_external(old)


UNRENDERED = "@{{ env_var('STAGE') }}/data/"


def test_rendered_location_difference_is_a_change_by_default():
    old = make_source(ExternalTable(location="@dev/data/", unrendered_location=UNRENDERED))
    new = make_source(ExternalTable(location="@prod/data/", unrendered_location=UNRENDERED))
    assert compare(old, new, flag=False) is False


def test_rendered_location_difference_is_ignored_with_flag():
    old = make_source(ExternalTable(location="@dev/data/", unrendered_location=UNRENDERED))
    new = make_source(ExternalTable(location="@prod/data/", unrendered_location=UNRENDERED))
    assert compare(old, new, flag=True) is True


def test_changed_unrendered_location_is_a_change_with_flag():
    old = make_source(ExternalTable(location="@dev/data/", unrendered_location=UNRENDERED))
    new = make_source(
        ExternalTable(location="@dev/data/", unrendered_location="@{{ env_var('OTHER') }}/data/")
    )
    assert compare(old, new, flag=True) is False


def test_other_external_properties_still_compared_with_flag():
    old = make_source(
        ExternalTable(location="@dev/data/", unrendered_location=UNRENDERED, file_format="csv")
    )
    new = make_source(
        ExternalTable(location="@dev/data/", unrendered_location=UNRENDERED, file_format="json")
    )
    assert compare(old, new, flag=True) is False


def test_falls_back_to_rendered_location_without_unrendered_value():
    old = make_source(ExternalTable(location="@dev/data/"))
    new = make_source(ExternalTable(location="@prod/data/"))
    assert compare(old, new, flag=True) is False
    assert compare(old, make_source(ExternalTable(location="@dev/data/")), flag=True) is True


def test_one_side_missing_unrendered_value_compares_rendered_locations():
    # e.g. state manifest written before unrendered_location existed
    old = make_source(ExternalTable(location="@dev/data/"))
    same = make_source(ExternalTable(location="@dev/data/", unrendered_location=UNRENDERED))
    different = make_source(ExternalTable(location="@prod/data/", unrendered_location=UNRENDERED))
    assert compare(old, same, flag=True) is True
    assert compare(old, different, flag=True) is False


def test_unrendered_location_does_not_affect_legacy_equality():
    old = make_source(ExternalTable(location="@dev/data/"))
    new = make_source(ExternalTable(location="@dev/data/", unrendered_location=UNRENDERED))
    assert compare(old, new, flag=False) is True


def test_external_added_or_removed_is_a_change_with_flag():
    with_external = make_source(ExternalTable(location="@dev/data/", unrendered_location="x"))
    without_external = make_source(None)
    assert compare(with_external, without_external, flag=True) is False
    assert compare(without_external, with_external, flag=True) is False
    assert compare(without_external, make_source(None), flag=True) is True


def test_schema_source_file_unrendered_external_location_roundtrip():
    file = SchemaSourceFile.__new__(SchemaSourceFile)
    file.unrendered_external_locations = {}
    assert file.get_unrendered_external_location("sources", "src", "tbl") is None
    file.add_unrendered_external_location("sources", "src", "tbl", UNRENDERED)
    assert file.get_unrendered_external_location("sources", "src", "tbl") == UNRENDERED
    assert file.get_unrendered_external_location("sources", "src", "other") is None
