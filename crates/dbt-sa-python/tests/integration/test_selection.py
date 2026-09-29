SEED = "seed.layered.people"
STG = "model.layered.stg_people"
MART = "model.layered.mart_people"


def models_only(uids):
    return {u for u in uids if u.startswith(("model.", "seed."))}


def test_select_single_model(built_project, invoke, unique_ids):
    res = invoke(built_project(), "build", "--select", "mart_people")

    assert res.success, res.exception
    assert models_only(unique_ids(res.result)) == {MART}


def test_select_ancestors(tmp_project, invoke, unique_ids):
    # Dependency-closed, so it needs no prior state.
    res = invoke(tmp_project("layered"), "build", "--select", "+mart_people")

    assert res.success, res.exception
    assert models_only(unique_ids(res.result)) == {SEED, STG, MART}


def test_select_descendants(built_project, invoke, unique_ids):
    res = invoke(built_project(), "build", "--select", "stg_people+")

    assert res.success, res.exception
    assert models_only(unique_ids(res.result)) == {STG, MART}


def test_select_by_tag(built_project, invoke, unique_ids):
    res = invoke(built_project(), "build", "--select", "tag:mart")

    assert res.success, res.exception
    assert models_only(unique_ids(res.result)) == {MART}


def test_exclude(tmp_project, invoke, unique_ids):
    # Excluding a leaf leaves the remainder dependency-closed.
    res = invoke(tmp_project("layered"), "build", "--exclude", "mart_people")

    assert res.success, res.exception
    selected = models_only(unique_ids(res.result))
    assert MART not in selected
    assert {SEED, STG} <= selected


def test_list_respects_selection(tmp_project, invoke):
    # `list` never touches the warehouse.
    res = invoke(tmp_project("layered"), "list", "--select", "tag:mart")

    assert res.success, res.exception
    # The model's own tests come along with it, as dbt does for any selection.
    assert set(res.result) == {
        "layered.mart_people",
        "layered.not_null_mart_people_id",
        "layered.unique_mart_people_id",
    }, res.result
