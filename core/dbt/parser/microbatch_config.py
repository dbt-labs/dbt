"""Validation (and coercion) of microbatch-specific node configs.

Extracted into its own module -- rather than living inline in
`dbt.parser.manifest` -- so it can be shared by both dbt-core's native parsing
pipeline (`ManifestLoader.check_valid_microbatch_config`) and the
`--use-v2-parser` path (`dbt.parser.v2.parse_with_v2`), which bypasses
`ManifestLoader` entirely and must call `validate_and_coerce_microbatch_configs`
directly.
"""

from datetime import datetime
from typing import Any

import dbt.exceptions
from dbt.artifacts.resources.types import BatchSize
from dbt.contracts.graph.manifest import Manifest
from dbt.contracts.graph.nodes import ManifestNode


def _is_microbatch_node(node: ManifestNode) -> bool:
    return (
        node.config.materialized == "incremental"
        and node.config.incremental_strategy == "microbatch"
    )


def _validate_event_time(node: ManifestNode) -> None:
    event_time = node.config.event_time
    if event_time is None:
        raise dbt.exceptions.ParsingError(
            f"Microbatch model '{node.name}' must provide an 'event_time' (string) config that indicates the name of the event time column."
        )
    if not isinstance(event_time, str):
        raise dbt.exceptions.ParsingError(
            f"Microbatch model '{node.name}' must provide an 'event_time' config of type string, but got: {type(event_time)}."
        )


def _coerce_and_validate_begin(node: ManifestNode) -> None:
    """Validate the `begin` config, coercing a string value to `datetime` in place.

    A `begin` config is only guaranteed to already be a native `datetime` when it
    is provided as an unquoted YAML timestamp (e.g. in a `.yml` properties file or
    `dbt_project.yml`), since PyYAML resolves those to `datetime.date`/`datetime.datetime`
    automatically. A `begin` provided as a quoted string -- including via a Jinja
    `{{ config(begin='2024-01-01') }}` call, which is never passed through YAML's
    timestamp resolver -- arrives here as a plain `str`, because `NodeConfig.begin`
    is typed `Any` and gets no automatic coercion from mashumaro.
    """
    begin: Any = node.config.begin
    if begin is None:
        raise dbt.exceptions.ParsingError(
            f"Microbatch model '{node.name}' must provide a 'begin' (datetime) config that indicates the earliest timestamp the microbatch model should be built from."
        )

    # Try to cast begin to a datetime using same format as mashumaro for consistency with other yaml-provided datetimes
    # Mashumaro default: https://github.com/Fatal1ty/mashumaro/blob/4ac16fd060a6c651053475597b58b48f958e8c5c/README.md?plain=1#L1186
    if isinstance(begin, str):
        try:
            begin = datetime.fromisoformat(begin)
            node.config.begin = begin
        except Exception:
            raise dbt.exceptions.ParsingError(
                f"Microbatch model '{node.name}' must provide a 'begin' config of valid datetime (ISO format), but got: {begin}."
            )

    if not isinstance(begin, datetime):
        raise dbt.exceptions.ParsingError(
            f"Microbatch model '{node.name}' must provide a 'begin' config of type datetime, but got: {type(begin)}."
        )


def _validate_batch_size(node: ManifestNode) -> None:
    batch_size = node.config.batch_size
    valid_batch_sizes = [size.value for size in BatchSize]
    if batch_size not in valid_batch_sizes:
        raise dbt.exceptions.ParsingError(
            f"Microbatch model '{node.name}' must provide a 'batch_size' config that is one of {valid_batch_sizes}, but got: {batch_size}."
        )


def _validate_lookback(node: ManifestNode) -> None:
    lookback = node.config.lookback
    if not isinstance(lookback, int) and lookback is not None:
        raise dbt.exceptions.ParsingError(
            f"Microbatch model '{node.name}' must provide the optional 'lookback' config as type int, but got: {type(lookback)})."
        )


def _validate_concurrent_batches(node: ManifestNode) -> None:
    concurrent_batches = node.config.concurrent_batches
    if not isinstance(concurrent_batches, bool) and concurrent_batches is not None:
        raise dbt.exceptions.ParsingError(
            f"Microbatch model '{node.name}' optional 'concurrent_batches' config must be of type `bool` if specified, but got: {type(concurrent_batches)})."
        )


def validate_and_coerce_microbatch_configs(manifest: Manifest, project_name: str) -> None:
    """Validate microbatch-specific configs on every incremental/microbatch node in
    `manifest`, coercing a string `begin` config to a `datetime` in place.

    This must run for every manifest before any microbatch model executes, regardless
    of how the manifest was produced: both dbt-core's own parser (via
    `ManifestLoader.check_valid_microbatch_config`, which delegates to this function)
    and the `--use-v2-parser` path (`dbt.parser.v2.parse_with_v2`, which bypasses
    `ManifestLoader` entirely and must call this function directly) load manifests
    whose `begin` may be a plain string.
    """
    if not manifest.use_microbatch_batches(project_name=project_name):
        return

    for node in manifest.nodes.values():
        if not _is_microbatch_node(node):
            continue

        _validate_event_time(node)
        _coerce_and_validate_begin(node)
        _validate_batch_size(node)
        _validate_lookback(node)
        _validate_concurrent_batches(node)
