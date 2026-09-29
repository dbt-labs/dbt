"""Unit tests for the dbtRunner wrapper: pure Python-binding logic, no engine run.

Commands run against fixtures live in tests/integration.
"""

import pytest
from dbt.runner import _kwargs_to_cli, dbtRunner


def test_manifest_injection_not_implemented():
    with pytest.raises(NotImplementedError, match="manifest="):
        dbtRunner(manifest=object())


def test_callbacks_not_implemented():
    with pytest.raises(NotImplementedError, match="callbacks="):
        dbtRunner(callbacks=[lambda event: None])


@pytest.mark.parametrize(
    "kwargs, expected",
    [
        ({"fail_fast": True}, ["--fail-fast"]),
        ({"fail_fast": False}, ["--no-fail-fast"]),
        ({"select": "my_model"}, ["--select", "my_model"]),
        ({"select": ["a", "b"]}, ["--select", "a", "--select", "b"]),
        ({"threads": 4}, ["--threads", "4"]),
    ],
)
def test_kwargs_to_cli_mapping(kwargs, expected):
    assert _kwargs_to_cli(kwargs) == expected


def test_unknown_command_captured_as_exception():
    # A bad command is reported on the result, not raised.
    res = dbtRunner().invoke(["this-is-not-a-command"])
    assert res.success is False
    assert res.exception is not None
    assert isinstance(res.exception, BaseException)
    # A binding-level failure never reaches the engine, so there is no catalog.
    assert res.catalog is None
