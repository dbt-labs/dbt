"""Binding mechanics: argument forms, repeated invocations, GIL, adapter failures."""

import os
import threading


def test_kwargs_invocation_equivalent_to_flags(built_project, invoke, unique_ids):
    """invoke(["build"], select=...) must select the same nodes as the flag form."""
    via_kwargs = invoke(built_project(), "build", select="mart_people")
    via_flags = invoke(built_project(), "build", "--select", "mart_people")

    assert via_kwargs.success, via_kwargs.exception
    assert via_flags.success, via_flags.exception
    assert unique_ids(via_kwargs.result) == unique_ids(via_flags.result)


def test_kwargs_list_becomes_repeated_flags(built_project, invoke, unique_ids):
    via_kwargs = invoke(built_project(), "build", select=["stg_people", "mart_people"])
    via_flags = invoke(
        built_project(),
        "build",
        "--select",
        "stg_people",
        "--select",
        "mart_people",
    )

    assert via_kwargs.success, via_kwargs.exception
    assert via_flags.success, via_flags.exception
    assert unique_ids(via_kwargs.result) == unique_ids(via_flags.result)


def test_unknown_flag_is_reported_not_raised(tmp_project, invoke):
    res = invoke(tmp_project("layered"), "parse", definitely_not_a_flag="x")

    assert res.success is False
    assert res.exception is not None


def test_manifest_from_first_invoke_survives_the_second(tmp_project, invoke):
    """The returned manifest is owned by the caller, not invalidated by a re-run."""
    proj = tmp_project("layered")
    first = invoke(proj, "parse")
    assert first.success, first.exception
    nodes_before = set(first.result.nodes)

    second = invoke(proj, "parse")
    assert second.success, second.exception

    assert set(first.result.nodes) == nodes_before
    assert first.result.to_dict()["nodes"]


def test_selection_does_not_leak_between_invocations(built_project, invoke, unique_ids):
    proj = built_project()
    narrow = invoke(proj, "build", "--select", "mart_people")
    wide = invoke(proj, "build")

    assert narrow.success, narrow.exception
    assert wide.success, wide.exception
    assert unique_ids(narrow.result) < unique_ids(wide.result)


def test_no_cross_project_bleed(tmp_project, invoke):
    """Two different projects in one process report their own nodes."""
    layered = invoke(tmp_project("layered"), "list")
    hello = invoke(tmp_project("hello_world"), "list")

    assert layered.success and hello.success
    assert any(item.startswith("layered.") for item in layered.result)
    assert not any(item.startswith("layered.") for item in hello.result)


def test_multiple_invocations_in_one_process(tmp_project, invoke):
    # Second invoke must reuse the once-per-process tracing init, not re-run it.
    proj = tmp_project("hello_world")
    assert invoke(proj, "parse").success
    assert invoke(proj, "parse").success


def test_unimplemented_adapter_is_not_fatal(tmp_project, invoke):
    # datafusion is unimplemented; either way the interpreter must survive and
    # the runner stay usable.
    proj = tmp_project("hello_world")
    os.environ["target_env_var"] = "datafusion"
    try:
        res = invoke(proj, "parse")
    finally:
        os.environ.pop("target_env_var", None)
    assert res.success is False
    # Still alive: a subsequent call works.
    assert invoke(proj, "parse").success


def test_gil_released_during_invoke(tmp_project, invoke):
    # invoke() drops the GIL, so a CPU-bound Python thread makes progress during
    # the run; if the GIL were held, the worker would be starved.
    proj = tmp_project("hello_world")
    counter = [0]
    stop = threading.Event()

    def worker():
        while not stop.is_set():
            counter[0] += 1

    t = threading.Thread(target=worker)
    t.start()
    try:
        before = counter[0]
        invoke(proj, "parse")
        delta = counter[0] - before
    finally:
        stop.set()
        t.join()
    assert delta > 100, f"worker barely progressed ({delta}); GIL may not be released"
