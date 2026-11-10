from dbt._core import run_cli as _run_cli
from dbt.runner import (  # noqa: F401
    DbtRunnerError,
    dbtRunner,
    dbtRunnerResult,
)


def cli() -> None:
    """Console-script entrypoint; hands argv to the engine and exits — never returns."""
    import sys

    _run_cli(sys.argv)
