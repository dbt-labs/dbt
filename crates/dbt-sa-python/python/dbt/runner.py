from typing import Any, Callable, Dict, List, Optional

import msgpack

from dbt._core import DbtRunner as _DbtRunner
from dbt.artifacts.schemas.catalog import CatalogArtifact
from dbt.artifacts.schemas.manifest import Manifest
from dbt.artifacts.schemas.run import RunResultsArtifact
from dbt.artifacts.schemas.sources import FreshnessResultsArtifact

# Keyed on the engine's `result_kind` tag. `list` is plain strings, so it skips
# the dataclass layer.
_DECODERS: Dict[str, Callable[[bytes], Any]] = {
    "manifest": Manifest.from_msgpack,
    "run_results": RunResultsArtifact.from_msgpack,
    "sources": FreshnessResultsArtifact.from_msgpack,
    "list": lambda blob: msgpack.unpackb(blob, raw=False),
}


def _decode(kind: Optional[str], blob: Optional[bytes]) -> Any:
    if kind is None or blob is None:
        return None
    try:
        return _DECODERS[kind](blob)
    except KeyError:
        raise DbtRunnerError(f"engine returned an unknown artifact kind: {kind!r}") from None


class DbtRunnerError(Exception):
    """Wraps the engine's error message, carried on ``dbtRunnerResult.exception``."""


class dbtRunnerResult:
    """Result of a dbt invocation.

    success: exited 0.
    result: command artifact — Manifest for parse, list[str] for list,
        FreshnessResultsArtifact for source freshness, RunResultsArtifact
        otherwise. Present even on a failure when the engine got far enough to
        build it (e.g. a partial manifest for a parse error); None only when
        nothing was captured.
    catalog: CatalogArtifact when --write-catalog produced one, else None. Kept
        off `result` so that stays dbt-core-compatible.
    exception: set on an engine error or a caught panic; a handled failure that its
        result artifact accounts for (a failing test, a stale source) has
        success=False and no exception.
    exit_code: engine exit code — 0 ok, 1 failure, 2 completed with elevated
        warnings. Not reconstructible from (success, exception); read it directly.
    """

    def __init__(
        self,
        success: bool,
        result: Any = None,
        exception: Optional[BaseException] = None,
        exit_code: Optional[int] = None,
        catalog: Any = None,
    ):
        self.success = success
        self.result = result
        self.exception = exception
        self.exit_code = exit_code
        self.catalog = catalog

    def __repr__(self) -> str:
        return (
            f"dbtRunnerResult(success={self.success}, "
            f"result={self.result!r}, exception={self.exception!r})"
        )


def _kwargs_to_cli(kwargs: dict) -> List[str]:
    """Turn keyword args into CLI flags.

        fail_fast=True     -> --fail-fast
        fail_fast=False    -> --no-fail-fast
        select="my_model" -> --select my_model
        select=["a", "b"] -> --select a --select b
        threads=4          -> --threads 4

    An unknown flag becomes a parse error on the result, not a crash.
    """
    argv: List[str] = []
    for key, value in kwargs.items():
        flag = "--" + key.replace("_", "-")
        if isinstance(value, bool):
            argv.append(flag if value else "--no-" + key.replace("_", "-"))
        elif isinstance(value, (list, tuple)):
            for item in value:
                argv += [flag, str(item)]
        else:
            argv += [flag, str(value)]
    return argv


class dbtRunner:
    """In-process dbt runner. Reuse one instance across calls."""

    # Each invoke() gets its own log file, verbosity and warn-error options. Concurrent
    # invokes are serialized, since a run's log layers are installed process-wide.
    # `--log-level trace` acts as `debug`: the subscriber's cap is fixed per process.

    def __init__(self, manifest: Any = None, callbacks: Any = None):
        if manifest is not None:
            raise NotImplementedError(
                "manifest= injection is not yet supported. Reuse the runner "
                "instance across invocations to avoid re-parsing."
            )
        if callbacks is not None:
            raise NotImplementedError("callbacks= (EventManager hooks) are not yet supported.")
        self._runner = _DbtRunner()

    def invoke(self, args: List[str], **kwargs) -> dbtRunnerResult:
        argv = list(args) + _kwargs_to_cli(kwargs)
        try:
            core = self._runner.invoke(argv)
        except (KeyboardInterrupt, SystemExit):
            raise
        except BaseException as exc:
            # Parse errors and caught panics: hand back on the result, don't
            # kill the interpreter.
            return dbtRunnerResult(success=False, result=None, exception=exc)
        # Engine reports errors on the result, not by raising; surface the message.
        exception = DbtRunnerError(core.exception) if core.exception else None
        catalog = (
            CatalogArtifact.from_msgpack(core.catalog_msgpack)
            if core.catalog_msgpack is not None
            else None
        )
        return dbtRunnerResult(
            success=core.success,
            result=_decode(core.result_kind, core.result_msgpack),
            exception=exception,
            exit_code=core.exit_code,
            catalog=catalog,
        )
