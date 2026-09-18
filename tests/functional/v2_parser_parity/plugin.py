"""Pytest plugin: opt tests into v2-parser parity coverage via a marker.

Mark a test with @pytest.mark.v2_parser_parity and have it take a
`parser_mode` fixture argument. Default behavior (no flag): the test
runs once with parser_mode='core'. When pytest is invoked with
--v2-parser-parity, marked tests are parametrized across
['core', 'v2_self'] and the v2_self runs apply the in-process shim
from v2_self_parser.install_shim.

Registered via pytest_plugins in tests/conftest.py.
"""

from __future__ import annotations

import pytest

from tests.functional.v2_parser_parity.v2_self_parser import install_shim


def pytest_addoption(parser):
    parser.addoption(
        "--v2-parser-parity",
        action="store_true",
        default=False,
        help=(
            "Parametrize @pytest.mark.v2_parser_parity tests across "
            "parser_mode=['core', 'v2_self']. v2_self routes the v2 "
            "parser subprocess through an in-process dbt parse to surface "
            "hidden parse-phase state."
        ),
    )


def pytest_configure(config):
    config.addinivalue_line(
        "markers",
        "v2_parser_parity: opt this test into v2-parser parity coverage. "
        "With --v2-parser-parity, parametrize across parser_mode=['core', "
        "'v2_self']; otherwise the test runs once in 'core' mode.",
    )


def pytest_generate_tests(metafunc):
    if "parser_mode" not in metafunc.fixturenames:
        return
    marker = metafunc.definition.get_closest_marker("v2_parser_parity")
    if marker is None:
        # parser_mode requested without opting in — single core run.
        metafunc.parametrize("parser_mode", ["core"])
        return
    if metafunc.config.getoption("--v2-parser-parity"):
        metafunc.parametrize("parser_mode", ["core", "v2_self"])
    else:
        metafunc.parametrize("parser_mode", ["core"])


@pytest.fixture
def parser_mode(request, monkeypatch):
    mode = request.param
    if mode == "v2_self":
        install_shim(monkeypatch)
    return mode
