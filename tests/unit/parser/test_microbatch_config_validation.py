"""Unit tests for `dbt.parser.microbatch_config.validate_and_coerce_microbatch_configs`,
covering both direct usage and its integration into `dbt.parser.v2.parse_with_v2`.

See `tests/functional/microbatch/test_microbatch_config_validation.py` for the
Postgres-backed functional tests covering the native (non-`--use-v2-parser`)
parsing path end-to-end.
"""

import json
from datetime import datetime
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

import pytest

from dbt.contracts.graph.manifest import Manifest
from dbt.exceptions import ParsingError
from dbt.parser.microbatch_config import validate_and_coerce_microbatch_configs
from dbt.parser.v2 import parse_with_v2
from tests.unit.fixtures import model_node
from tests.unit.parser.test_v2 import _fake_parser, _patch_v2_deps  # noqa: F401


def _microbatch_model(begin=None, batch_size="day", event_time="event_time"):
    """A ModelNode configured as a microbatch incremental model, with `begin`
    left as whatever type the caller passes -- e.g. a plain `str`, mirroring
    what a v2-parser-produced manifest.json decodes `begin` as (see
    `validate_and_coerce_microbatch_configs`'s docstring)."""
    model = model_node()
    model.config.materialized = "incremental"
    model.config.incremental_strategy = "microbatch"
    model.config.event_time = event_time
    model.config.batch_size = batch_size
    model.config.begin = begin
    return model


class TestValidateAndCoerceMicrobatchConfigs:
    """Unit tests for the validation+coercion logic extracted out of
    `ManifestLoader.check_valid_microbatch_config` so it can also run under
    `--use-v2-parser` (see TestParseWithV2MicrobatchConfigValidation below for
    the `parse_with_v2` integration)."""

    def _manifest(self, mocker, *nodes, use_microbatch_batches=True):
        manifest = Manifest(nodes={node.unique_id: node for node in nodes})
        mocker.patch.object(
            manifest, "use_microbatch_batches", return_value=use_microbatch_batches
        )
        return manifest

    def test_coerces_string_begin_to_datetime_in_place(self, mocker):
        """The exact regression this closes (dbt-labs/dbt#16582): a `begin`
        config provided as a quoted Jinja string -- e.g.
        `{{ config(..., begin='2024-01-01') }}` -- is never resolved to a
        native YAML timestamp, so it arrives here (and would otherwise reach
        `MicrobatchBuilder`) as a plain `str`."""
        model = _microbatch_model(begin="2024-01-01")
        manifest = self._manifest(mocker, model)

        validate_and_coerce_microbatch_configs(manifest, "test")

        assert model.config.begin == datetime(2024, 1, 1)

    def test_leaves_native_datetime_begin_untouched(self, mocker):
        begin = datetime(2024, 1, 1)
        model = _microbatch_model(begin=begin)
        manifest = self._manifest(mocker, model)

        validate_and_coerce_microbatch_configs(manifest, "test")

        assert model.config.begin is begin

    def test_raises_on_missing_begin(self, mocker):
        model = _microbatch_model(begin=None)
        manifest = self._manifest(mocker, model)

        with pytest.raises(ParsingError, match="must provide a 'begin'"):
            validate_and_coerce_microbatch_configs(manifest, "test")

    def test_raises_on_non_iso_begin_string(self, mocker):
        model = _microbatch_model(begin="not-a-date")
        manifest = self._manifest(mocker, model)

        with pytest.raises(ParsingError, match="valid datetime"):
            validate_and_coerce_microbatch_configs(manifest, "test")

    def test_raises_on_non_string_non_datetime_begin(self, mocker):
        model = _microbatch_model(begin=2)
        manifest = self._manifest(mocker, model)

        with pytest.raises(ParsingError, match="type datetime"):
            validate_and_coerce_microbatch_configs(manifest, "test")

    def test_raises_on_missing_event_time(self, mocker):
        model = _microbatch_model(begin="2024-01-01", event_time=None)
        manifest = self._manifest(mocker, model)

        with pytest.raises(ParsingError, match="must provide an 'event_time'"):
            validate_and_coerce_microbatch_configs(manifest, "test")

    def test_raises_on_invalid_batch_size(self, mocker):
        model = _microbatch_model(begin="2024-01-01", batch_size="fortnight")
        manifest = self._manifest(mocker, model)

        with pytest.raises(ParsingError, match="must provide a 'batch_size'"):
            validate_and_coerce_microbatch_configs(manifest, "test")

    def test_noop_when_use_microbatch_batches_is_false(self, mocker):
        """Not every project requires batched execution for a custom
        microbatch strategy -- `Manifest.use_microbatch_batches` gates that.
        When it's False, validation (and the `begin` coercion) must not run."""
        model = _microbatch_model(begin="not-a-date")
        manifest = self._manifest(mocker, model, use_microbatch_batches=False)

        validate_and_coerce_microbatch_configs(manifest, "test")  # must not raise

        assert model.config.begin == "not-a-date"

    def test_ignores_non_microbatch_nodes(self, mocker):
        model = model_node()
        model.config.materialized = "table"
        manifest = self._manifest(mocker, model)

        validate_and_coerce_microbatch_configs(manifest, "test")  # must not raise


class TestParseWithV2MicrobatchConfigValidation:
    """Integration coverage proving `parse_with_v2` -- which bypasses
    `ManifestLoader` and therefore never calls
    `ManifestLoader.check_valid_microbatch_config` -- now applies the same
    `begin` validation/coercion via `validate_and_coerce_microbatch_configs`.

    Before this fix, a microbatch model's `begin` loaded straight off the v2
    parser's manifest.json stayed a plain `str`, and the first run (or any
    `--full-refresh`) crashed with `'str' object has no attribute 'year'`
    (dbt-labs/dbt#16582)."""

    def _runtime_config(self, target_path: Path):
        return SimpleNamespace(project_target_path=str(target_path), project_name="test")

    def test_parse_with_v2_coerces_string_begin(
        self, tmp_path: Path, _patch_v2_deps, mocker  # noqa: F811
    ):
        model = _microbatch_model(begin="2024-01-01")
        manifest = Manifest(nodes={model.unique_id: model})
        mocker.patch.object(manifest, "use_microbatch_batches", return_value=True)

        with mock.patch(
            "dbt.parser.v2.subprocess.Popen",
            side_effect=_fake_parser(json.dumps({"metadata": {}})),
        ), mock.patch(
            "dbt.parser.v2._load_writable_manifest", return_value=mock.MagicMock()
        ), mock.patch(
            "dbt.parser.v2.Manifest.from_writable_manifest", return_value=manifest
        ):
            result = parse_with_v2(self._runtime_config(tmp_path), write=False, write_json=False)

        assert result is manifest
        assert model.config.begin == datetime(2024, 1, 1)

    def test_parse_with_v2_raises_parsing_error_for_invalid_begin(
        self, tmp_path: Path, _patch_v2_deps, mocker  # noqa: F811
    ):
        model = _microbatch_model(begin="not-a-date")
        manifest = Manifest(nodes={model.unique_id: model})
        mocker.patch.object(manifest, "use_microbatch_batches", return_value=True)

        with mock.patch(
            "dbt.parser.v2.subprocess.Popen",
            side_effect=_fake_parser(json.dumps({"metadata": {}})),
        ), mock.patch(
            "dbt.parser.v2._load_writable_manifest", return_value=mock.MagicMock()
        ), mock.patch(
            "dbt.parser.v2.Manifest.from_writable_manifest", return_value=manifest
        ):
            with pytest.raises(ParsingError, match="valid datetime"):
                parse_with_v2(self._runtime_config(tmp_path), write=False, write_json=False)
