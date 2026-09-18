from types import SimpleNamespace
from unittest import mock

from dbt.cli import requires


def _ctx(flags, manifest=None):
    """Minimal click-like Context with the keys setup_manifest reads."""
    runtime_config = SimpleNamespace(project_target_path="target")
    project = SimpleNamespace(project_name="test_project")
    profile = SimpleNamespace()
    obj = {
        "flags": flags,
        "profile": profile,
        "project": project,
        "runtime_config": runtime_config,
        "manifest": manifest,
    }
    return SimpleNamespace(obj=obj)


def _flags(use_v2: bool = False):
    return SimpleNamespace(
        USE_V2_PARSER=use_v2,
        V2_PARSER="dbt-core-experimental-parser parse",
        USE_CATALOGS_V2=False,
        PROJECT_DIR="/proj",
        VARS={},
        write_json=True,
    )


class TestSetupManifestV2Branch:
    def _common_patches(self):
        # Stub adapter wiring + catalog loading + deferred events.
        # Also stub the plugin enrichment hook so the test does not interact
        # with mantle's registered cloud plugins (which advertise get_nodes
        # and would trip the v2 fail-fast guard).
        return mock.patch.multiple(
            "dbt.cli.requires",
            load_catalogs=mock.DEFAULT,
            get_active_write_integration=mock.DEFAULT,
            _wire_adapter_for_external_manifest=mock.DEFAULT,
            get_adapter=mock.DEFAULT,
            fire_deferred_events=mock.DEFAULT,
        )

    def _enrich_patch(self):
        return mock.patch("dbt.parser.manifest.enrich_manifest_with_plugin_artifacts")

    def _assert_no_get_nodes_patch(self):
        return mock.patch("dbt.parser.manifest.assert_no_get_nodes_plugins")

    def test_use_v2_calls_parse_with_v2(self):
        ctx = _ctx(_flags(use_v2=True))
        with self._common_patches() as patches, self._enrich_patch(), self._assert_no_get_nodes_patch(), mock.patch(
            "dbt.parser.v2.parse_with_v2"
        ) as parse_with_v2, mock.patch(
            "dbt.cli.requires.parse_manifest"
        ) as parse_manifest:
            patches["load_catalogs"].return_value = []
            parse_with_v2.return_value = SimpleNamespace()

            requires.setup_manifest(ctx)

            parse_with_v2.assert_called_once()
            parse_manifest.assert_not_called()
            patches["_wire_adapter_for_external_manifest"].assert_called_once()

    def test_default_calls_parse_manifest(self):
        ctx = _ctx(_flags(use_v2=False))
        with self._common_patches() as patches, self._enrich_patch(), self._assert_no_get_nodes_patch(), mock.patch(
            "dbt.parser.v2.parse_with_v2"
        ) as parse_with_v2, mock.patch(
            "dbt.cli.requires.parse_manifest"
        ) as parse_manifest:
            patches["load_catalogs"].return_value = []
            parse_manifest.return_value = SimpleNamespace()

            requires.setup_manifest(ctx)

            parse_manifest.assert_called_once()
            parse_with_v2.assert_not_called()
            patches["_wire_adapter_for_external_manifest"].assert_not_called()

    def test_pre_set_manifest_skips_both_parsers(self):
        existing = SimpleNamespace()
        ctx = _ctx(_flags(use_v2=True), manifest=existing)
        with self._common_patches() as patches, self._enrich_patch(), self._assert_no_get_nodes_patch(), mock.patch(
            "dbt.parser.v2.parse_with_v2"
        ) as parse_with_v2, mock.patch(
            "dbt.cli.requires.parse_manifest"
        ) as parse_manifest:
            patches["load_catalogs"].return_value = []

            requires.setup_manifest(ctx)

            parse_with_v2.assert_not_called()
            parse_manifest.assert_not_called()
            patches["_wire_adapter_for_external_manifest"].assert_called_once()
            assert ctx.obj["manifest"] is existing
