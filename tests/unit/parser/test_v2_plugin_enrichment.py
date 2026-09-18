from types import SimpleNamespace
from unittest import mock

import pytest

from dbt.exceptions import dbtPluginError
from dbt.parser.manifest import (
    assert_no_get_nodes_plugins,
    enrich_manifest_with_plugin_artifacts,
)


def _hook_for(plugin_name: str):
    """Build a callable whose __self__.name matches what PluginManager exposes."""
    plugin_obj = SimpleNamespace(name=plugin_name)
    hook = mock.MagicMock()
    hook.__self__ = plugin_obj
    return hook


class TestEnrichManifestWithPluginArtifacts:
    def test_runs_get_manifest_artifacts_and_writes(self):
        manifest = mock.MagicMock()
        artifact = mock.MagicMock()
        artifact.__class__.__name__ = "FakeArtifact"
        pm = SimpleNamespace(
            hooks={},
            get_manifest_artifacts=mock.MagicMock(return_value={"some/path.json": artifact}),
        )
        with mock.patch("dbt.parser.manifest.plugins.get_plugin_manager", return_value=pm):
            enrich_manifest_with_plugin_artifacts(manifest, "proj")

        pm.get_manifest_artifacts.assert_called_once_with(manifest)
        artifact.write.assert_called_once_with("some/path.json")

    def test_assert_no_get_nodes_plugins_raises(self):
        pm = SimpleNamespace(
            hooks={"get_nodes": [_hook_for("plugin_a"), _hook_for("plugin_b")]},
            get_manifest_artifacts=mock.MagicMock(return_value={}),
        )
        with mock.patch("dbt.parser.manifest.plugins.get_plugin_manager", return_value=pm):
            with pytest.raises(dbtPluginError, match="get_nodes"):
                assert_no_get_nodes_plugins("proj")

        pm.get_manifest_artifacts.assert_not_called()

    def test_assert_no_get_nodes_plugins_allows_v2_parity_plugins(self):
        pm = SimpleNamespace(
            hooks={
                "get_nodes": [
                    _hook_for("dbtCloudAutoExposures"),
                    _hook_for("dbtCloudCrossProjectRef"),
                ]
            },
            get_manifest_artifacts=mock.MagicMock(return_value={}),
        )
        with mock.patch("dbt.parser.manifest.plugins.get_plugin_manager", return_value=pm):
            assert_no_get_nodes_plugins("proj")

    def test_assert_no_get_nodes_plugins_mixed_raises_only_for_offenders(self):
        pm = SimpleNamespace(
            hooks={
                "get_nodes": [
                    _hook_for("dbtCloudAutoExposures"),
                    _hook_for("custom_plugin"),
                ]
            },
            get_manifest_artifacts=mock.MagicMock(return_value={}),
        )
        with mock.patch("dbt.parser.manifest.plugins.get_plugin_manager", return_value=pm):
            with pytest.raises(dbtPluginError, match="custom_plugin") as exc:
                assert_no_get_nodes_plugins("proj")
            assert "dbtCloudAutoExposures" not in str(exc.value)

    def test_no_plugins_no_artifacts(self):
        manifest = mock.MagicMock()
        pm = SimpleNamespace(
            hooks={},
            get_manifest_artifacts=mock.MagicMock(return_value={}),
        )
        with mock.patch("dbt.parser.manifest.plugins.get_plugin_manager", return_value=pm):
            enrich_manifest_with_plugin_artifacts(manifest, "proj")

        pm.get_manifest_artifacts.assert_called_once()
