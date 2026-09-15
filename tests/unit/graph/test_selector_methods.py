import copy
import os
from argparse import Namespace
from dataclasses import replace
from pathlib import Path
from unittest import mock

import pytest

import dbt.compilation
import dbt_common.exceptions
from dbt.artifacts.resources import ColumnInfo, FileHash
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.state import PreviousState
from dbt.graph import NodeSelector
from dbt.graph.selector_methods import (
    AccessSelectorMethod,
    ConfigSelectorMethod,
    ExposureSelectorMethod,
    FileSelectorMethod,
    GroupSelectorMethod,
    MethodManager,
    MetricSelectorMethod,
    PackageSelectorMethod,
    PathSelectorMethod,
    QualifiedNameSelectorMethod,
    SavedQuerySelectorMethod,
    SelectorSelectorMethod,
    SemanticModelSelectorMethod,
    SourceSelectorMethod,
    StateSelectorMethod,
    TagSelectorMethod,
    TestNameSelectorMethod,
    TestTypeSelectorMethod,
    UnitTestSelectorMethod,
    VersionSelectorMethod,
)
from dbt.graph.selector_spec import (
    SelectionCriteria,
    SelectionIntersection,
    SelectionUnion,
)
from dbt_common.ui import warning_tag
from tests.unit.utils import replace_config
from tests.unit.utils.manifest import (
    make_exposure,
    make_group,
    make_macro,
    make_metric,
    make_model,
    make_saved_query,
    make_seed,
    make_semantic_model,
    make_unit_test,
)


def search_manifest_using_method(manifest, method, selection):
    selected = method.search(
        set(manifest.nodes)
        | set(manifest.sources)
        | set(manifest.exposures)
        | set(manifest.metrics)
        | set(manifest.semantic_models)
        | set(manifest.saved_queries)
        | set(manifest.unit_tests),
        selection,
    )
    results = {manifest.expect(uid).search_name for uid in selected}
    return results


def test_select_fqn(manifest):
    methods = MethodManager(manifest, None)
    method = methods.get_method("fqn", [])
    assert isinstance(method, QualifiedNameSelectorMethod)
    assert method.arguments == []

    assert search_manifest_using_method(manifest, method, "pkg.unions") == {
        "union_model",
        "mynamespace.union_model",
    }
    assert not search_manifest_using_method(manifest, method, "ext.unions")
    # sources don't show up, because selection pretends they have no FQN. Should it?
    assert search_manifest_using_method(manifest, method, "pkg") == {
        "union_model",
        "versioned_model.v1",
        "versioned_model.v2",
        "versioned_model.v3",
        "versioned_model.v4",
        "versioned_model.v12",
        "table_model",
        "table_model_py",
        "table_model_csv",
        "view_model",
        "ephemeral_model",
        "seed",
        "mynamespace.union_model",
        "mynamespace.ephemeral_model",
        "mynamespace.seed",
        "unit_test_table_model",
    }
    assert search_manifest_using_method(manifest, method, "ext") == {"ext_model"}
    # versions
    assert search_manifest_using_method(manifest, method, "versioned_model") == {
        "versioned_model.v1",
        "versioned_model.v2",
        "versioned_model.v3",
        "versioned_model.v4",
        "versioned_model.v12",
    }
    assert search_manifest_using_method(manifest, method, "versioned_model.v1") == {
        "versioned_model.v1"
    }
    # version selection with _ instead of '.'
    assert search_manifest_using_method(manifest, method, "versioned_model_v1") == {
        "versioned_model.v1"
    }
    # version selection with _ instead of '.' - latest version
    assert search_manifest_using_method(manifest, method, "versioned_model_v2") == {
        "versioned_model.v2"
    }
    # wildcards
    assert search_manifest_using_method(manifest, method, "*.*.*_model") == {
        "mynamespace.union_model",
        "mynamespace.ephemeral_model",
        "test_semantic_model",
        "union_model",
        "unit_test_table_model",
    }
    # multiple wildcards
    assert search_manifest_using_method(manifest, method, "*unions*") == {
        "union_model",
        "mynamespace.union_model",
    }
    # negation
    assert not search_manifest_using_method(manifest, method, "!pkg*")
    # single wildcard
    assert search_manifest_using_method(manifest, method, "pkg.t*") == {
        "table_model",
        "table_model_py",
        "table_model_csv",
        "unit_test_table_model",
    }
    # wildcard and ? (matches exactly one character)
    assert search_manifest_using_method(manifest, method, "*ext_m?del") == {"ext_model"}
    # multiple ?
    assert search_manifest_using_method(manifest, method, "*.?????_model") == {
        "union_model",
        "table_model",
        "mynamespace.union_model",
    }
    # multiple ranges
    assert search_manifest_using_method(manifest, method, "*.[t-u][a-n][b-i][l-o][e-n]_model") == {
        "union_model",
        "table_model",
        "mynamespace.union_model",
    }


def test_select_tag(manifest):
    methods = MethodManager(manifest, None)
    method = methods.get_method("tag", [])
    assert isinstance(method, TagSelectorMethod)
    assert method.arguments == []

    assert search_manifest_using_method(manifest, method, "uses_ephemeral") == {
        "view_model",
        "table_model",
    }
    assert not search_manifest_using_method(manifest, method, "missing")
    assert search_manifest_using_method(manifest, method, "uses_eph*") == {
        "view_model",
        "table_model",
    }


def test_select_selector(manifest, runtime_config):
    compiler = dbt.compilation.Compiler(runtime_config)
    graph = compiler.compile(manifest)
    selectors = {
        "union_selector": {
            "definition": SelectionUnion(
                components=[
                    SelectionCriteria.from_single_spec("fqn:table_model"),
                    SelectionCriteria.from_single_spec("fqn:view_model"),
                ]
            ),
        },
        "intersection_selector": {
            "definition": SelectionIntersection(
                components=[
                    SelectionCriteria.from_single_spec("selector:union_selector"),
                    SelectionCriteria.from_single_spec("fqn:table_model"),
                ]
            ),
        },
    }
    selector = NodeSelector(graph, manifest, selectors=selectors)
    methods = MethodManager(manifest, None)
    method = methods.get_method(
        "selector", [], selectors=selectors, get_selected_callback=selector.get_selected
    )
    assert isinstance(method, SelectorSelectorMethod)

    assert search_manifest_using_method(manifest, method, "union_selector").issuperset(
        {
            "table_model",
            "view_model",
        }
    )
    assert search_manifest_using_method(manifest, method, "intersection_selector").issuperset(
        {
            "table_model",
        }
    )


def test_select_group(manifest, view_model):
    group_name = "my_group"
    group = make_group("test", group_name)
    manifest.groups[group.unique_id] = group
    change_node(
        manifest,
        replace(view_model, config={"materialized": "view", "group": group_name}),
    )
    methods = MethodManager(manifest, None)
    method = methods.get_method("group", [])
    assert isinstance(method, GroupSelectorMethod)
    assert method.arguments == []

    assert search_manifest_using_method(manifest, method, group_name) == {"view_model"}
    assert search_manifest_using_method(manifest, method, "my?group") == {"view_model"}
    assert not search_manifest_using_method(manifest, method, "not_my_group")


def test_select_access(manifest, view_model):
    change_node(
        manifest,
        replace(view_model, access="public"),
    )
    methods = MethodManager(manifest, None)
    method = methods.get_method("access", [])
    assert isinstance(method, AccessSelectorMethod)
    assert method.arguments == []

    assert search_manifest_using_method(manifest, method, "public") == {"view_model"}
    assert not search_manifest_using_method(manifest, method, "private")


def test_select_source(manifest):
    methods = MethodManager(manifest, None)
    method = methods.get_method("source", [])
    assert isinstance(method, SourceSelectorMethod)
    assert method.arguments == []

    # the lookup is based on how many components you provide: source, source.table, package.source.table
    assert search_manifest_using_method(manifest, method, "raw") == {
        "raw.seed",
        "raw.ext_source",
        "raw.ext_source_2",
    }
    assert search_manifest_using_method(manifest, method, "raw.seed") == {"raw.seed"}
    assert search_manifest_using_method(manifest, method, "pkg.raw.seed") == {"raw.seed"}
    assert search_manifest_using_method(manifest, method, "pkg.*.*") == {"raw.seed"}
    assert search_manifest_using_method(manifest, method, "raw.*") == {
        "raw.seed",
        "raw.ext_source",
        "raw.ext_source_2",
    }
    assert search_manifest_using_method(manifest, method, "ext.raw.*") == {
        "raw.ext_source",
        "raw.ext_source_2",
    }
    assert not search_manifest_using_method(manifest, method, "missing")
    assert not search_manifest_using_method(manifest, method, "raw.missing")
    assert not search_manifest_using_method(manifest, method, "missing.raw.seed")

    assert search_manifest_using_method(manifest, method, "ext.*.*") == {
        "ext_raw.ext_source",
        "ext_raw.ext_source_2",
        "raw.ext_source",
        "raw.ext_source_2",
    }
    assert search_manifest_using_method(manifest, method, "ext_raw") == {
        "ext_raw.ext_source",
        "ext_raw.ext_source_2",
    }
    assert search_manifest_using_method(manifest, method, "ext.ext_raw.*") == {
        "ext_raw.ext_source",
        "ext_raw.ext_source_2",
    }
    assert not search_manifest_using_method(manifest, method, "pkg.ext_raw.*")
    assert search_manifest_using_method(manifest, method, "*.ext_[s]ourc?") == {
        "ext_raw.ext_source",
        "raw.ext_source",
    }


# TODO: this requires writing out files
@pytest.mark.skip("TODO: write manifest files to disk")
def test_select_path(manifest):
    methods = MethodManager(manifest, None)
    method = methods.get_method("path", [])
    assert isinstance(method, PathSelectorMethod)
    assert method.arguments == []

    assert search_manifest_using_method(manifest, method, "subdirectory/*.sql") == {
        "union_model",
        "table_model",
    }
    assert search_manifest_using_method(manifest, method, "subdirectory/union_model.sql") == {
        "union_model"
    }
    assert search_manifest_using_method(manifest, method, "models/*.sql") == {
        "view_model",
        "ephemeral_model",
    }
    assert not search_manifest_using_method(manifest, method, "missing")
    assert not search_manifest_using_method(manifest, method, "models/missing.sql")
    assert not search_manifest_using_method(manifest, method, "models/missing*")


def test_select_file(manifest):
    methods = MethodManager(manifest, None)
    method = methods.get_method("file", [])
    assert isinstance(method, FileSelectorMethod)
    assert method.arguments == []

    assert search_manifest_using_method(manifest, method, "table_model.sql") == {"table_model"}
    assert search_manifest_using_method(manifest, method, "table_model.py") == {"table_model_py"}
    assert search_manifest_using_method(manifest, method, "table_model.csv") == {"table_model_csv"}
    assert search_manifest_using_method(manifest, method, "union_model.sql") == {
        "union_model",
        "mynamespace.union_model",
    }
    assert not search_manifest_using_method(manifest, method, "missing.sql")
    assert not search_manifest_using_method(manifest, method, "missing.py")
    assert search_manifest_using_method(manifest, method, "table_*.csv") == {"table_model_csv"}

    # stem selector match
    assert search_manifest_using_method(manifest, method, "union_model") == {
        "union_model",
        "mynamespace.union_model",
    }
    assert search_manifest_using_method(manifest, method, "versioned_model_v1") == {
        "versioned_model.v1"
    }


def test_select_package(manifest):
    methods = MethodManager(manifest, None)
    method = methods.get_method("package", [])
    assert isinstance(method, PackageSelectorMethod)
    assert method.arguments == []

    assert search_manifest_using_method(manifest, method, "pkg") == {
        "union_model",
        "versioned_model.v1",
        "versioned_model.v2",
        "versioned_model.v3",
        "versioned_model.v4",
        "versioned_model.v12",
        "table_model",
        "table_model_py",
        "table_model_csv",
        "view_model",
        "ephemeral_model",
        "seed",
        "raw.seed",
        "unique_table_model_id",
        "not_null_table_model_id",
        "unique_view_model_id",
        "view_test_nothing",
        "mynamespace.seed",
        "mynamespace.ephemeral_model",
        "mynamespace.union_model",
        "unit_test_table_model",
    }
    assert search_manifest_using_method(manifest, method, "ext") == {
        "ext_model",
        "ext_raw.ext_source",
        "ext_raw.ext_source_2",
        "raw.ext_source",
        "raw.ext_source_2",
        "unique_ext_raw_ext_source_id",
    }

    assert not search_manifest_using_method(manifest, method, "missing")

    assert search_manifest_using_method(manifest, method, "ex*") == {
        "ext_model",
        "ext_raw.ext_source",
        "ext_raw.ext_source_2",
        "raw.ext_source",
        "raw.ext_source_2",
        "unique_ext_raw_ext_source_id",
    }


def test_select_package_this(manifest):
    new_manifest = copy.deepcopy(manifest)

    # change the package name for all nodes except ones where the unique_id contains "table_model"
    for id, node in new_manifest.nodes.items():
        if "table_model" not in id:
            node.package_name = "foo"

    for source in new_manifest.sources.values():
        if "table_model" not in source.unique_id:
            source.package_name = "foo"

    methods = MethodManager(new_manifest, None)
    method = methods.get_method("package", [])
    assert isinstance(method, PackageSelectorMethod)
    assert method.arguments == []

    assert search_manifest_using_method(new_manifest, method, "this") == {
        "not_null_table_model_id",
        "table_model",
        "table_model_csv",
        "table_model_py",
        "unique_table_model_id",
        "unit_test_table_model",
    }


def test_select_config_materialized(manifest):
    methods = MethodManager(manifest, None)
    method = methods.get_method("config", ["materialized"])
    assert isinstance(method, ConfigSelectorMethod)
    assert method.arguments == ["materialized"]

    assert search_manifest_using_method(manifest, method, "view") == {"view_model", "ext_model"}
    assert search_manifest_using_method(manifest, method, "table") == {
        "table_model",
        "table_model_py",
        "table_model_csv",
        "union_model",
        "versioned_model.v1",
        "versioned_model.v2",
        "versioned_model.v3",
        "versioned_model.v4",
        "versioned_model.v12",
        "mynamespace.union_model",
    }


def test_select_config_meta(manifest):
    methods = MethodManager(manifest, None)

    string_method = methods.get_method("config", ["meta", "string_property"])
    assert search_manifest_using_method(manifest, string_method, "some_string") == {"table_model"}
    assert not search_manifest_using_method(manifest, string_method, "other_string") == {
        "table_model"
    }

    truthy_bool_method = methods.get_method("config", ["meta", "truthy_bool_property"])
    assert search_manifest_using_method(manifest, truthy_bool_method, "true") == {"table_model"}
    assert not search_manifest_using_method(manifest, truthy_bool_method, "false") == {
        "table_model"
    }
    assert not search_manifest_using_method(manifest, truthy_bool_method, "other") == {
        "table_model"
    }

    falsy_bool_method = methods.get_method("config", ["meta", "falsy_bool_property"])
    assert search_manifest_using_method(manifest, falsy_bool_method, "false") == {"table_model"}
    assert not search_manifest_using_method(manifest, falsy_bool_method, "true") == {"table_model"}
    assert not search_manifest_using_method(manifest, falsy_bool_method, "other") == {
        "table_model"
    }

    list_method = methods.get_method("config", ["meta", "list_property"])
    assert search_manifest_using_method(manifest, list_method, "some_value") == {"table_model"}
    assert search_manifest_using_method(manifest, list_method, "true") == {"table_model"}
    assert search_manifest_using_method(manifest, list_method, "false") == {"table_model"}
    assert not search_manifest_using_method(manifest, list_method, "other") == {"table_model"}


def test_select_test_name(manifest):
    methods = MethodManager(manifest, None)
    method = methods.get_method("test_name", [])
    assert isinstance(method, TestNameSelectorMethod)
    assert method.arguments == []

    assert search_manifest_using_method(manifest, method, "unique") == {
        "unique_table_model_id",
        "unique_view_model_id",
        "unique_ext_raw_ext_source_id",
    }
    assert search_manifest_using_method(manifest, method, "not_null") == {
        "not_null_table_model_id"
    }
    assert not search_manifest_using_method(manifest, method, "notatest")
    assert search_manifest_using_method(manifest, method, "not_*") == {"not_null_table_model_id"}


def test_select_test_type(manifest):
    methods = MethodManager(manifest, None)
    method = methods.get_method("test_type", [])
    assert isinstance(method, TestTypeSelectorMethod)
    assert method.arguments == []
    assert search_manifest_using_method(manifest, method, "generic") == {
        "unique_table_model_id",
        "not_null_table_model_id",
        "unique_view_model_id",
        "unique_ext_raw_ext_source_id",
    }
    assert search_manifest_using_method(manifest, method, "singular") == {"view_test_nothing"}
    # test backwards compatibility
    assert search_manifest_using_method(manifest, method, "schema") == {
        "unique_table_model_id",
        "not_null_table_model_id",
        "unique_view_model_id",
        "unique_ext_raw_ext_source_id",
    }
    assert search_manifest_using_method(manifest, method, "data") == {
        "view_test_nothing",
        "unique_table_model_id",
        "not_null_table_model_id",
        "unique_view_model_id",
        "unique_ext_raw_ext_source_id",
    }
    assert search_manifest_using_method(manifest, method, "unit") == {
        "unit_test_table_model",
    }


def test_select_version(manifest):
    methods = MethodManager(manifest, None)
    method = methods.get_method("version", [])
    assert isinstance(method, VersionSelectorMethod)
    assert method.arguments == []
    assert search_manifest_using_method(manifest, method, "latest") == {"versioned_model.v2"}
    assert search_manifest_using_method(manifest, method, "old") == {"versioned_model.v1"}
    assert search_manifest_using_method(manifest, method, "prerelease") == {
        "versioned_model.v3",
        "versioned_model.v4",
        "versioned_model.v12",
    }
    assert search_manifest_using_method(manifest, method, "none") == {
        "table_model_py",
        "union_model",
        "view_model",
        "mynamespace.ephemeral_model",
        "table_model_csv",
        "ephemeral_model",
        "mynamespace.union_model",
        "table_model",
        "ext_model",
    }


def test_select_exposure(manifest):
    exposure = make_exposure("test", "my_exposure")
    manifest.exposures[exposure.unique_id] = exposure
    methods = MethodManager(manifest, None)
    method = methods.get_method("exposure", [])
    assert isinstance(method, ExposureSelectorMethod)
    assert search_manifest_using_method(manifest, method, "my_exposure") == {"my_exposure"}
    assert not search_manifest_using_method(manifest, method, "not_my_exposure")
    assert search_manifest_using_method(manifest, method, "my_e*e") == {"my_exposure"}


def test_select_metric(manifest):
    metric = make_metric("test", "my_metric")
    manifest.metrics[metric.unique_id] = metric
    methods = MethodManager(manifest, None)
    method = methods.get_method("metric", [])
    assert isinstance(method, MetricSelectorMethod)
    assert search_manifest_using_method(manifest, method, "my_metric") == {"my_metric"}
    assert not search_manifest_using_method(manifest, method, "not_my_metric")
    assert search_manifest_using_method(manifest, method, "*_metric") == {"my_metric"}


def test_select_semantic_model(manifest, table_model):
    semantic_model = make_semantic_model(
        "pkg",
        "customer",
        model=table_model,
        path="_semantic_models.yml",
    )
    manifest.semantic_models[semantic_model.unique_id] = semantic_model
    methods = MethodManager(manifest, None)
    method = methods.get_method("semantic_model", [])
    assert isinstance(method, SemanticModelSelectorMethod)
    assert search_manifest_using_method(manifest, method, "customer") == {"customer"}
    assert not search_manifest_using_method(manifest, method, "not_customer")
    assert search_manifest_using_method(manifest, method, "*omer") == {"customer"}


def test_select_semantic_model_by_tag(manifest, table_model):
    semantic_model = make_semantic_model(
        "pkg",
        "customer",
        model=table_model,
        path="_semantic_models.yml",
    )
    manifest.semantic_models[semantic_model.unique_id] = semantic_model
    methods = MethodManager(manifest, None)
    method = methods.get_method("tag", [])
    assert isinstance(method, TagSelectorMethod)
    assert method.arguments == []
    search_manifest_using_method(manifest, method, "any_tag")


def test_select_saved_query(manifest: Manifest) -> None:
    metric = make_metric("test", "my_metric")
    saved_query = make_saved_query(
        "pkg",
        "test_saved_query",
        "my_metric",
    )
    manifest.metrics[metric.unique_id] = metric
    manifest.saved_queries[saved_query.unique_id] = saved_query
    methods = MethodManager(manifest, None)
    method = methods.get_method("saved_query", [])
    assert isinstance(method, SavedQuerySelectorMethod)
    assert search_manifest_using_method(manifest, method, "test_saved_query") == {
        "test_saved_query"
    }
    assert not search_manifest_using_method(manifest, method, "not_test_saved_query")
    assert search_manifest_using_method(manifest, method, "*uery") == {"test_saved_query"}


def test_select_saved_query_by_tag(manifest: Manifest) -> None:
    metric = make_metric("test", "my_metric")
    saved_query = make_saved_query(
        "pkg",
        "test_saved_query",
        "my_metric",
    )
    manifest.metrics[metric.unique_id] = metric
    manifest.saved_queries[saved_query.unique_id] = saved_query
    methods = MethodManager(manifest, None)
    method = methods.get_method("tag", [])
    assert isinstance(method, TagSelectorMethod)
    assert method.arguments == []
    search_manifest_using_method(manifest, method, "any_tag")


def test_modified_saved_query(manifest: Manifest) -> None:
    metric = make_metric("test", "my_metric")
    saved_query = make_saved_query(
        "pkg",
        "test_saved_query",
        "my_metric",
    )
    manifest.metrics[metric.unique_id] = metric
    manifest.saved_queries[saved_query.unique_id] = saved_query
    # Create PreviousState with a saved query, this deepcopies manifest
    previous_state = create_previous_state(manifest)
    method = statemethod(manifest, previous_state)

    # create another metric and add to saved query
    alt_metric = make_metric("test", "alt_metric")
    manifest.metrics[alt_metric.unique_id] = alt_metric
    saved_query.query_params.metrics.append("alt_metric")

    assert search_manifest_using_method(manifest, method, "modified") == {"test_saved_query"}


def test_select_unit_test(manifest: Manifest) -> None:
    test_model = make_model("test", "my_model", "select 1 as id")
    unit_test = make_unit_test("test", "my_unit_test", test_model)
    manifest.unit_tests[unit_test.unique_id] = unit_test
    methods = MethodManager(manifest, None)
    method = methods.get_method("unit_test", [])

    assert isinstance(method, UnitTestSelectorMethod)
    assert not search_manifest_using_method(manifest, method, "not_test_unit_test")
    assert search_manifest_using_method(manifest, method, "*nit_test") == {unit_test.search_name}
    assert search_manifest_using_method(manifest, method, "test.my_unit_test") == {
        unit_test.search_name
    }
    assert search_manifest_using_method(manifest, method, "my_unit_test") == {
        unit_test.search_name
    }


def create_previous_state(manifest):
    writable = copy.deepcopy(manifest).writable_manifest()
    state = PreviousState(
        state_path=Path("/path/does/not/exist"),
        target_path=Path("/path/does/not/exist"),
        project_root=Path("/path/does/not/exist"),
    )
    state.manifest = Manifest.from_writable_manifest(writable)
    return state


@pytest.fixture
def previous_state(manifest):
    return create_previous_state(manifest)


@pytest.fixture
def args_for_flags():
    return Namespace(
        state_modified_compare_more_unrendered_values=False, state_modified_compare_vars=False
    )


def add_node(manifest, node):
    manifest.nodes[node.unique_id] = node


def add_macro(manifest, macro):
    manifest.macros[macro.unique_id] = macro


def change_node(manifest, node, change=None):
    if change is not None:
        node = change(node)
    manifest.nodes[node.unique_id] = node


def statemethod(manifest, previous_state):
    methods = MethodManager(manifest, previous_state)
    method = methods.get_method("state", [])
    assert isinstance(method, StateSelectorMethod)
    assert method.arguments == []
    return method


def test_select_state_no_change(manifest, previous_state):
    method = statemethod(manifest, previous_state)
    assert not search_manifest_using_method(manifest, method, "modified")
    assert not search_manifest_using_method(manifest, method, "new")
    assert not search_manifest_using_method(manifest, method, "modified.configs")
    assert not search_manifest_using_method(manifest, method, "modified.persisted_descriptions")
    assert not search_manifest_using_method(manifest, method, "modified.relation")
    assert not search_manifest_using_method(manifest, method, "modified.macros")


def test_select_state_nothing(manifest, previous_state):
    previous_state.manifest = None
    method = statemethod(manifest, previous_state)
    with pytest.raises(dbt_common.exceptions.DbtRuntimeError) as exc:
        search_manifest_using_method(manifest, method, "modified")
    assert "no comparison manifest" in str(exc.value)

    with pytest.raises(dbt_common.exceptions.DbtRuntimeError) as exc:
        search_manifest_using_method(manifest, method, "new")
    assert "no comparison manifest" in str(exc.value)

    with pytest.raises(dbt_common.exceptions.DbtRuntimeError) as exc:
        search_manifest_using_method(manifest, method, "unmodified")
    assert "no comparison manifest" in str(exc.value)

    with pytest.raises(dbt_common.exceptions.DbtRuntimeError) as exc:
        search_manifest_using_method(manifest, method, "old")
    assert "no comparison manifest" in str(exc.value)


def test_select_state_added_model(manifest, previous_state):
    add_node(manifest, make_model("pkg", "another_model", "select 1 as id"))
    method = statemethod(manifest, previous_state)
    assert search_manifest_using_method(manifest, method, "modified") == {"another_model"}
    assert search_manifest_using_method(manifest, method, "new") == {"another_model"}
    assert search_manifest_using_method(manifest, method, "modified.body") == {"another_model"}

    # none of these
    assert not {"another_model"} in search_manifest_using_method(manifest, method, "old")
    assert not {"another_model"} in search_manifest_using_method(manifest, method, "unmodified")


def test_select_state_changed_model_sql(manifest, previous_state, view_model):
    change_node(manifest, replace(view_model, raw_code="select 1 as id"))
    method = statemethod(manifest, previous_state)

    # both of these
    assert search_manifest_using_method(manifest, method, "modified") == {"view_model"}
    assert search_manifest_using_method(manifest, method, "modified.body") == {"view_model"}

    # none of these
    assert not search_manifest_using_method(manifest, method, "new")
    assert not {"view_model"} in search_manifest_using_method(manifest, method, "old")
    assert not {"view_model"} in search_manifest_using_method(manifest, method, "unmodified")
    assert not search_manifest_using_method(manifest, method, "modified.configs")
    assert not search_manifest_using_method(manifest, method, "modified.persisted_descriptions")
    assert not search_manifest_using_method(manifest, method, "modified.relation")
    assert not search_manifest_using_method(manifest, method, "modified.macros")


def test_select_state_changed_model_fqn(manifest, previous_state, view_model):
    change_node(
        manifest, replace(view_model, fqn=view_model.fqn[:-1] + ["nested"] + view_model.fqn[-1:])
    )
    method = statemethod(manifest, previous_state)
    assert search_manifest_using_method(manifest, method, "modified") == {"view_model"}
    assert not search_manifest_using_method(manifest, method, "new")

    # none of these
    assert not {"view_model"} in search_manifest_using_method(manifest, method, "old")
    assert not {"view_model"} in search_manifest_using_method(manifest, method, "unmodified")


def test_select_state_added_seed(manifest, previous_state):
    add_node(manifest, make_seed("pkg", "another_seed"))
    method = statemethod(manifest, previous_state)
    assert search_manifest_using_method(manifest, method, "modified") == {"another_seed"}
    assert search_manifest_using_method(manifest, method, "new") == {"another_seed"}

    # none of these
    assert not {"another_seed"} in search_manifest_using_method(manifest, method, "old")
    assert not {"another_seed"} in search_manifest_using_method(manifest, method, "unmodified")


def test_select_state_changed_seed_checksum_sha_to_sha(manifest, previous_state, seed):
    change_node(manifest, replace(seed, checksum=FileHash.from_contents("changed")))
    method = statemethod(manifest, previous_state)
    assert search_manifest_using_method(manifest, method, "modified") == {"seed"}
    assert not search_manifest_using_method(manifest, method, "new")
    assert "seed" not in search_manifest_using_method(manifest, method, "unmodified")


def test_select_state_changed_seed_checksum_path_to_path(manifest, previous_state, seed):
    change_node(
        previous_state.manifest,
        replace(seed, checksum=FileHash(name="path", checksum=seed.original_file_path)),
    )
    change_node(
        manifest, replace(seed, checksum=FileHash(name="path", checksum=seed.original_file_path))
    )
    method = statemethod(manifest, previous_state)
    with mock.patch("dbt.contracts.graph.nodes.fire_or_defer_event") as fire_or_defer_event_patch:
        assert not search_manifest_using_method(manifest, method, "modified")
        fire_or_defer_event_patch.assert_called_once()
        event = fire_or_defer_event_patch.call_args[0][0]
        assert type(event).__name__ == "SeedExceedsLimitSamePath"
        msg = event.message()
        assert msg.startswith(warning_tag("Found a seed (pkg.seed) >1MiB in size"))
    with mock.patch("dbt.contracts.graph.nodes.fire_or_defer_event") as fire_or_defer_event_patch:
        assert not search_manifest_using_method(manifest, method, "new")
        fire_or_defer_event_patch.assert_not_called()
    with mock.patch("dbt.contracts.graph.nodes.fire_or_defer_event") as fire_or_defer_event_patch:
        assert search_manifest_using_method(manifest, method, "old")
        fire_or_defer_event_patch.assert_not_called()
    with mock.patch("dbt.contracts.graph.nodes.fire_or_defer_event") as fire_or_defer_event_patch:
        assert search_manifest_using_method(manifest, method, "unmodified")
        fire_or_defer_event_patch.assert_called_once()
        event = fire_or_defer_event_patch.call_args[0][0]
        assert type(event).__name__ == "SeedExceedsLimitSamePath"
        msg = event.message()
        assert msg.startswith(warning_tag("Found a seed (pkg.seed) >1MiB in size"))


def test_select_state_changed_seed_checksum_sha_to_path(manifest, previous_state, seed):
    change_node(
        manifest, replace(seed, checksum=FileHash(name="path", checksum=seed.original_file_path))
    )
    method = statemethod(manifest, previous_state)
    with mock.patch("dbt.contracts.graph.nodes.fire_or_defer_event") as fire_or_defer_event_patch:
        assert search_manifest_using_method(manifest, method, "modified") == {"seed"}
        fire_or_defer_event_patch.assert_called_once()
        event = fire_or_defer_event_patch.call_args[0][0]
        assert type(event).__name__ == "SeedIncreased"
        msg = event.message()
        assert msg.startswith(warning_tag("Found a seed (pkg.seed) >1MiB in size"))
    with mock.patch("dbt.contracts.graph.nodes.fire_or_defer_event") as fire_or_defer_event_patch:
        assert not search_manifest_using_method(manifest, method, "new")
        fire_or_defer_event_patch.assert_not_called()
    with mock.patch("dbt.contracts.graph.nodes.fire_or_defer_event") as fire_or_defer_event_patch:
        assert search_manifest_using_method(manifest, method, "old")
        fire_or_defer_event_patch.assert_not_called()
    with mock.patch("dbt.contracts.graph.nodes.fire_or_defer_event") as fire_or_defer_event_patch:
        assert search_manifest_using_method(manifest, method, "unmodified")
        fire_or_defer_event_patch.assert_called_once()
        event = fire_or_defer_event_patch.call_args[0][0]
        assert type(event).__name__ == "SeedIncreased"
        msg = event.message()
        assert msg.startswith(warning_tag("Found a seed (pkg.seed) >1MiB in size"))


def test_select_state_changed_seed_checksum_path_to_sha(manifest, previous_state, seed):
    change_node(
        previous_state.manifest,
        replace(seed, checksum=FileHash(name="path", checksum=seed.original_file_path)),
    )
    method = statemethod(manifest, previous_state)
    with mock.patch("dbt.contracts.graph.nodes.fire_or_defer_event") as fire_or_defer_event_patch:
        assert search_manifest_using_method(manifest, method, "modified") == {"seed"}
        fire_or_defer_event_patch.assert_not_called()
    with mock.patch("dbt.contracts.graph.nodes.fire_or_defer_event") as fire_or_defer_event_patch:
        assert not search_manifest_using_method(manifest, method, "new")
        fire_or_defer_event_patch.assert_not_called()
    with mock.patch("dbt.contracts.graph.nodes.fire_or_defer_event") as fire_or_defer_event_patch:
        assert "seed" not in search_manifest_using_method(manifest, method, "unmodified")
        fire_or_defer_event_patch.assert_not_called()
    with mock.patch("dbt.contracts.graph.nodes.fire_or_defer_event") as fire_or_defer_event_patch:
        assert "seed" in search_manifest_using_method(manifest, method, "old")
        fire_or_defer_event_patch.assert_not_called()


def test_select_state_changed_seed_fqn(manifest, previous_state, seed):
    change_node(manifest, replace(seed, fqn=seed.fqn[:-1] + ["nested"] + seed.fqn[-1:]))
    method = statemethod(manifest, previous_state)
    assert search_manifest_using_method(manifest, method, "modified") == {"seed"}
    assert not search_manifest_using_method(manifest, method, "new")
    assert "seed" not in search_manifest_using_method(manifest, method, "unmodified")
    assert "seed" in search_manifest_using_method(manifest, method, "old")


def test_select_state_changed_seed_relation_documented(manifest, previous_state, seed):
    seed_doc_relation = replace_config(seed, persist_docs={"relation": True})
    change_node(manifest, seed_doc_relation)
    method = statemethod(manifest, previous_state)
    assert search_manifest_using_method(manifest, method, "modified") == {"seed"}
    assert search_manifest_using_method(manifest, method, "modified.configs") == {"seed"}
    assert "seed" in search_manifest_using_method(manifest, method, "old")
    assert not search_manifest_using_method(manifest, method, "new")
    assert "seed" not in search_manifest_using_method(manifest, method, "unmodified")
    assert not search_manifest_using_method(manifest, method, "modified.body")
    assert not search_manifest_using_method(manifest, method, "modified.persisted_descriptions")


def test_select_state_changed_seed_relation_documented_nodocs(manifest, previous_state, seed):
    seed_doc_relation = replace_config(seed, persist_docs={"relation": True})
    seed_doc_relation_documented = replace(seed_doc_relation, description="a description")
    change_node(previous_state.manifest, seed_doc_relation)
    change_node(manifest, seed_doc_relation_documented)
    method = statemethod(manifest, previous_state)
    assert search_manifest_using_method(manifest, method, "modified") == {"seed"}
    assert search_manifest_using_method(manifest, method, "modified.persisted_descriptions") == {
        "seed"
    }
    assert "seed" in search_manifest_using_method(manifest, method, "old")
    assert not search_manifest_using_method(manifest, method, "new")
    assert "seed" not in search_manifest_using_method(manifest, method, "unmodified")
    assert not search_manifest_using_method(manifest, method, "modified.configs")


def test_select_state_changed_seed_relation_documented_withdocs(manifest, previous_state, seed):
    seed_doc_relation = replace_config(seed, persist_docs={"relation": True})
    seed_doc_relation_documented = replace(seed_doc_relation, description="a description")
    change_node(previous_state.manifest, seed_doc_relation_documented)
    change_node(manifest, seed_doc_relation)
    method = statemethod(manifest, previous_state)
    assert search_manifest_using_method(manifest, method, "modified") == {"seed"}
    assert search_manifest_using_method(manifest, method, "modified.persisted_descriptions") == {
        "seed"
    }
    assert "seed" in search_manifest_using_method(manifest, method, "old")
    assert not search_manifest_using_method(manifest, method, "new")
    assert "seed" not in search_manifest_using_method(manifest, method, "unmodified")


def test_select_state_changed_seed_columns_documented(manifest, previous_state, seed):
    # changing persist_docs, even without changing the description -> changed
    seed_doc_columns = replace_config(seed, persist_docs={"columns": True})
    change_node(manifest, seed_doc_columns)
    method = statemethod(manifest, previous_state)
    assert search_manifest_using_method(manifest, method, "modified") == {"seed"}
    assert search_manifest_using_method(manifest, method, "modified.configs") == {"seed"}
    assert "seed" in search_manifest_using_method(manifest, method, "old")
    assert not search_manifest_using_method(manifest, method, "new")
    assert "seed" not in search_manifest_using_method(manifest, method, "unmodified")
    assert not search_manifest_using_method(manifest, method, "modified.persisted_descriptions")


def test_select_state_changed_seed_columns_documented_nodocs(manifest, previous_state, seed):
    seed_doc_columns = replace_config(seed, persist_docs={"columns": True})
    seed_doc_columns_documented_columns = replace(
        seed_doc_columns,
        columns={"a": ColumnInfo(name="a", description="a description")},
    )

    change_node(previous_state.manifest, seed_doc_columns)
    change_node(manifest, seed_doc_columns_documented_columns)

    method = statemethod(manifest, previous_state)
    assert search_manifest_using_method(manifest, method, "modified") == {"seed"}
    assert search_manifest_using_method(manifest, method, "modified.persisted_descriptions") == {
        "seed"
    }
    assert "seed" in search_manifest_using_method(manifest, method, "old")
    assert not search_manifest_using_method(manifest, method, "new")
    assert "seed" not in search_manifest_using_method(manifest, method, "unmodified")
    assert not search_manifest_using_method(manifest, method, "modified.configs")


def test_select_state_changed_seed_columns_documented_withdocs(manifest, previous_state, seed):
    seed_doc_columns = replace_config(seed, persist_docs={"columns": True})
    seed_doc_columns_documented_columns = replace(
        seed_doc_columns,
        columns={"a": ColumnInfo(name="a", description="a description")},
    )

    change_node(manifest, seed_doc_columns)
    change_node(previous_state.manifest, seed_doc_columns_documented_columns)

    method = statemethod(manifest, previous_state)
    assert search_manifest_using_method(manifest, method, "modified") == {"seed"}
    assert search_manifest_using_method(manifest, method, "modified.persisted_descriptions") == {
        "seed"
    }
    assert "seed" in search_manifest_using_method(manifest, method, "old")
    assert not search_manifest_using_method(manifest, method, "new")
    assert "seed" not in search_manifest_using_method(manifest, method, "unmodified")
    assert not search_manifest_using_method(manifest, method, "modified.configs")


def test_select_state_changed_test_macro_sql(
    manifest, previous_state, macro_default_test_not_null
):
    manifest.macros[macro_default_test_not_null.unique_id] = replace(
        macro_default_test_not_null, macro_sql="lalala"
    )
    method = statemethod(manifest, previous_state)
    assert search_manifest_using_method(manifest, method, "modified") == {
        "not_null_table_model_id"
    }
    assert search_manifest_using_method(manifest, method, "modified.macros") == {
        "not_null_table_model_id"
    }
    assert "not_null_table_model_id" in search_manifest_using_method(manifest, method, "old")
    assert not search_manifest_using_method(manifest, method, "new")
    assert "not_null_table_model_id" not in search_manifest_using_method(
        manifest, method, "unmodified"
    )


def test_select_state_changed_test_macros(manifest, previous_state):
    changed_macro = make_macro("dbt", "changed_macro", "blablabla")
    add_macro(manifest, changed_macro)
    add_macro(previous_state.manifest, replace(changed_macro, macro_sql="something different"))

    unchanged_macro = make_macro("dbt", "unchanged_macro", "blablabla")
    add_macro(manifest, unchanged_macro)
    add_macro(previous_state.manifest, unchanged_macro)

    model1 = make_model(
        "dbt",
        "model1",
        "blablabla",
        depends_on_macros=[changed_macro.unique_id, unchanged_macro.unique_id],
    )
    add_node(manifest, model1)
    add_node(previous_state.manifest, model1)

    model2 = make_model(
        "dbt",
        "model2",
        "blablabla",
        depends_on_macros=[unchanged_macro.unique_id, changed_macro.unique_id],
    )
    add_node(manifest, model2)
    add_node(previous_state.manifest, model2)

    method = statemethod(manifest, previous_state)

    assert search_manifest_using_method(manifest, method, "modified") == {"model1", "model2"}
    assert search_manifest_using_method(manifest, method, "modified.macros") == {
        "model1",
        "model2",
    }
    assert "model1" and "model2" in search_manifest_using_method(manifest, method, "old")
    assert not search_manifest_using_method(manifest, method, "new")
    assert "model1" and "model2" not in search_manifest_using_method(
        manifest, method, "unmodified"
    )


def test_select_state_changed_test_macros_with_upstream_change(manifest, previous_state):
    changed_macro = make_macro("dbt", "changed_macro", "blablabla")
    add_macro(manifest, changed_macro)
    add_macro(previous_state.manifest, replace(changed_macro, macro_sql="something different"))

    unchanged_macro1 = make_macro("dbt", "unchanged_macro", "blablabla")
    add_macro(manifest, unchanged_macro1)
    add_macro(previous_state.manifest, unchanged_macro1)

    unchanged_macro2 = make_macro(
        "dbt",
        "unchanged_macro",
        "blablabla",
        depends_on_macros=[unchanged_macro1.unique_id, changed_macro.unique_id],
    )
    add_macro(manifest, unchanged_macro2)
    add_macro(previous_state.manifest, unchanged_macro2)

    unchanged_macro3 = make_macro(
        "dbt",
        "unchanged_macro",
        "blablabla",
        depends_on_macros=[changed_macro.unique_id, unchanged_macro1.unique_id],
    )
    add_macro(manifest, unchanged_macro3)
    add_macro(previous_state.manifest, unchanged_macro3)

    model1 = make_model(
        "dbt", "model1", "blablabla", depends_on_macros=[unchanged_macro1.unique_id]
    )
    add_node(manifest, model1)
    add_node(previous_state.manifest, model1)

    model2 = make_model(
        "dbt", "model2", "blablabla", depends_on_macros=[unchanged_macro3.unique_id]
    )
    add_node(manifest, model2)
    add_node(previous_state.manifest, model2)

    method = statemethod(manifest, previous_state)

    assert search_manifest_using_method(manifest, method, "modified") == {"model1", "model2"}
    assert search_manifest_using_method(manifest, method, "modified.macros") == {
        "model1",
        "model2",
    }
    assert "model1" and "model2" in search_manifest_using_method(manifest, method, "old")
    assert not search_manifest_using_method(manifest, method, "new")
    assert "model1" and "model2" not in search_manifest_using_method(
        manifest, method, "unmodified"
    )


def test_select_state_changed_test_macros_missing_macro(manifest, previous_state):
    # Some unrelated macro must change so that `modified_macros` is non-empty and
    # `recursively_check_macros_modified` actually walks `model1`'s macro deps below.
    changed_macro = make_macro("dbt", "changed_macro", "blablabla")
    add_macro(manifest, changed_macro)
    add_macro(previous_state.manifest, replace(changed_macro, macro_sql="something different"))

    # model1 depends on a macro uid that isn't in `manifest.macros` on either side (e.g. a
    # v2/fusion-parser manifest that omits an adapter-dispatched macro). This must not raise
    # KeyError, and since the missing macro isn't in `modified_macros`, model1 is unmodified.
    missing_macro_uid = "macro.dbt_snowflake.snowflake__date_spine"

    model1 = make_model(
        "dbt",
        "model1",
        "blablabla",
        depends_on_macros=[missing_macro_uid],
    )
    add_node(manifest, model1)
    add_node(previous_state.manifest, model1)

    method = statemethod(manifest, previous_state)

    assert search_manifest_using_method(manifest, method, "modified") == set()
    assert search_manifest_using_method(manifest, method, "modified.macros") == set()
    assert "model1" in search_manifest_using_method(manifest, method, "unmodified")


class TestSeedNodeSameSeedsFallback:
    """Tests for the legacy-hash fallback in SeedNode.same_seeds()."""

    def _make_seed_node(self, checksum, root_path="", original_file_path="seeds/seed.csv"):
        seed = make_seed("pkg", "test_seed")
        seed = replace(seed, checksum=checksum, root_path=root_path)
        seed = replace(seed, original_file_path=original_file_path)
        return seed

    def test_matching_checksums(self):
        """When checksums match, same_seeds returns True (no fallback needed)."""
        h = FileHash.from_contents("id,name\n1,Alice")
        current = self._make_seed_node(h)
        previous = self._make_seed_node(h)
        assert current.same_seeds(previous) is True

    def test_different_checksums_no_root_path(self):
        """When checksums differ and root_path is empty, no fallback — returns False."""
        h1 = FileHash.from_contents("id,name\n1,Alice")
        h2 = FileHash.from_contents("id,name\n1,Bob")
        current = self._make_seed_node(h1, root_path="")
        previous = self._make_seed_node(h2)
        assert current.same_seeds(previous) is False

    @pytest.mark.skipif(
        os.name != "nt",
        reason="CRLF/LF hash divergence only occurs on Windows where text mode normalises \\r\\n",
    )
    def test_legacy_fallback_matches_windows(self, tmp_path):
        """On Windows: new hash (text-mode) differs from old (binary-mode legacy) hash for a
        CRLF file, but the fallback re-hashes via the legacy method and recognises the file
        is unchanged."""
        seed_file = tmp_path / "seeds" / "seed.csv"
        seed_file.parent.mkdir(parents=True)
        seed_file.write_bytes(b"id,name\r\n1,Alice\r\n")

        # On Windows, from_path (text mode) normalises \r\n → \n; from_path_legacy preserves it
        new_hash = FileHash.from_path(str(seed_file))
        old_hash = FileHash.from_path_legacy(str(seed_file))

        assert new_hash.checksum != old_hash.checksum, "Hashes should differ on Windows"

        current = self._make_seed_node(
            new_hash, root_path=str(tmp_path), original_file_path="seeds/seed.csv"
        )
        previous = self._make_seed_node(old_hash)

        # Fallback fires (os.name == "nt") and recognises the legacy hash → not modified
        assert current.same_seeds(previous) is True

    def test_legacy_fallback_no_match(self, tmp_path):
        """When new hash AND legacy hash both differ from old, seed IS modified."""
        seed_file = tmp_path / "seeds" / "seed.csv"
        seed_file.parent.mkdir(parents=True)
        seed_file.write_bytes(b"id,name\r\n1,Alice\r\n")

        new_hash = FileHash.from_path(str(seed_file))
        # Old hash from completely different content
        old_hash = FileHash.from_contents("id,name\r\n1,Bob\r\n")

        current = self._make_seed_node(
            new_hash, root_path=str(tmp_path), original_file_path="seeds/seed.csv"
        )
        previous = self._make_seed_node(old_hash)

        assert current.same_seeds(previous) is False

    def test_no_fallback_when_hash_types_differ(self, tmp_path):
        """When hash types differ (e.g., sha256 vs path), no fallback is attempted."""
        seed_file = tmp_path / "seeds" / "seed.csv"
        seed_file.parent.mkdir(parents=True)
        seed_file.write_text("id,name\n1,Alice\n")

        current = self._make_seed_node(
            FileHash.from_path(str(seed_file)),
            root_path=str(tmp_path),
            original_file_path="seeds/seed.csv",
        )
        previous = self._make_seed_node(FileHash.path("seeds/seed.csv"))

        # Different hash types → FileHash.__eq__ returns False, no fallback
        assert current.same_seeds(previous) is False

    def test_no_fallback_for_path_hash_type(self, tmp_path):
        """When both checksums are 'path' type, fallback should not trigger."""
        current = self._make_seed_node(
            FileHash.path("seeds/seed.csv"),
            root_path=str(tmp_path),
        )
        previous = self._make_seed_node(FileHash.path("seeds/other.csv"))
        # path type is excluded from fallback
        assert current.same_seeds(previous) is False
