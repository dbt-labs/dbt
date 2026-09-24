from dbt.parser.common import normalize_tags


def test_normalize_tags_none():
    assert normalize_tags(None) == []


def test_normalize_tags_empty_list():
    assert normalize_tags([]) == []


def test_normalize_tags_string():
    assert normalize_tags("single") == ["single"]


def test_normalize_tags_preserves_order():
    assert normalize_tags(["b", "a", "c"]) == ["b", "a", "c"]


def test_normalize_tags_deduplication():
    assert normalize_tags(["a", "a", "b"]) == ["a", "b"]


def test_normalize_tags_dedup_preserves_first_occurrence():
    assert normalize_tags(["z", "a", "z", "b"]) == ["z", "a", "b"]


def test_normalize_tags_single_item_list():
    assert normalize_tags(["only"]) == ["only"]


def test_normalize_tags_empty_string():
    assert normalize_tags("") == [""]
    assert normalize_tags([""]) == [""]


def test_normalize_tags_no_sources():
    assert normalize_tags() == []


def test_normalize_tags_multiple_sources():
    assert normalize_tags(["a"], ["b", "c"]) == ["a", "b", "c"]


def test_normalize_tags_multiple_sources_with_string_and_none():
    assert normalize_tags("project_tag", None, ["yaml_tag"]) == ["project_tag", "yaml_tag"]


def test_normalize_tags_multiple_sources_dedup_across():
    assert normalize_tags(["shared"], "shared") == ["shared"]
