import json
import os
from unittest.mock import MagicMock, patch

import pytest

from dbt.contracts.files import FileHash, FilePath, OsiSourceFile, ParseFileType
from dbt.events.types import MFConverterIssue
from dbt.exceptions import ParsingError
from dbt.parser.osi import (
    _build_model_lookup,
    _clear_osi_attributed_nodes,
    _scan_osi_directories,
    load_osi_into_manifest,
)
from dbt_common.events.event_catcher import EventCatcher
from dbt_common.events.event_manager_client import add_callback_to_manager
from tests.unit.utils.manifest import make_manifest, make_model

PKG = "test_pkg"


# Minimal OSI JSON; source uses three-part "db.schema.table" to match make_model defaults
# (database="dbt", schema="dbt_schema").
def _osi_json(
    sm_name: str = "orders",
    source: str = "dbt.dbt_schema.orders",
    version: str = "0.1.1",
) -> str:
    return json.dumps(
        {
            "version": version,
            "semantic_model": [
                {"name": sm_name, "datasets": [{"name": sm_name, "source": source}]}
            ],
        }
    )


def _orders_model():
    return make_model(PKG, "orders", "select 1 as id")


class TestScanOsiDirectories:
    def test_no_directory_returns_empty(self, tmp_path):
        assert _scan_osi_directories(str(tmp_path), ["OSI"]) == []

    def test_empty_directory_returns_empty(self, tmp_path):
        (tmp_path / "OSI").mkdir()
        assert _scan_osi_directories(str(tmp_path), ["OSI"]) == []

    def test_returns_only_json_files(self, tmp_path):
        osi_dir = tmp_path / "OSI"
        osi_dir.mkdir()
        (osi_dir / "a.json").write_text("{}")
        (osi_dir / "b.yaml").write_text("{}")
        (osi_dir / "c.json").write_text("{}")
        result = _scan_osi_directories(str(tmp_path), ["OSI"])
        assert [p.name for p in result] == ["a.json", "c.json"]

    def test_results_are_sorted(self, tmp_path):
        osi_dir = tmp_path / "OSI"
        osi_dir.mkdir()
        for name in ["z.json", "a.json", "m.json"]:
            (osi_dir / name).write_text("{}")
        result = _scan_osi_directories(str(tmp_path), ["OSI"])
        assert [p.name for p in result] == ["a.json", "m.json", "z.json"]

    def test_recurses_into_subdirectories(self, tmp_path):
        osi_dir = tmp_path / "OSI"
        (osi_dir / "subdir").mkdir(parents=True)
        (osi_dir / "top.json").write_text("{}")
        (osi_dir / "subdir" / "nested.json").write_text("{}")
        (osi_dir / "subdir" / "ignored.yaml").write_text("{}")
        result = _scan_osi_directories(str(tmp_path), ["OSI"])
        assert {p.name for p in result} == {"top.json", "nested.json"}

    def test_custom_path_is_used(self, tmp_path):
        custom_dir = tmp_path / "custom_osi"
        custom_dir.mkdir()
        (custom_dir / "orders.json").write_text("{}")
        result = _scan_osi_directories(str(tmp_path), ["custom_osi"])
        assert [p.name for p in result] == ["orders.json"]

    def test_multiple_paths_all_scanned(self, tmp_path):
        (tmp_path / "osi_a").mkdir()
        (tmp_path / "osi_b").mkdir()
        (tmp_path / "osi_a" / "a.json").write_text("{}")
        (tmp_path / "osi_b" / "b.json").write_text("{}")
        result = _scan_osi_directories(str(tmp_path), ["osi_a", "osi_b"])
        assert {p.name for p in result} == {"a.json", "b.json"}

    def test_missing_path_is_skipped(self, tmp_path):
        (tmp_path / "osi_a").mkdir()
        (tmp_path / "osi_a" / "a.json").write_text("{}")
        result = _scan_osi_directories(str(tmp_path), ["osi_a", "nonexistent"])
        assert [p.name for p in result] == ["a.json"]

    def test_lowercase_osi_directory_is_recognized(self, tmp_path):
        osi_dir = tmp_path / "osi"
        osi_dir.mkdir()
        (osi_dir / "orders.json").write_text("{}")
        result = _scan_osi_directories(str(tmp_path), ["osi"])
        assert [p.name for p in result] == ["orders.json"]

    def test_uppercase_osi_directory_still_recognized(self, tmp_path):
        osi_dir = tmp_path / "OSI"
        osi_dir.mkdir()
        (osi_dir / "orders.json").write_text("{}")
        result = _scan_osi_directories(str(tmp_path), ["OSI"])
        assert [p.name for p in result] == ["orders.json"]

    # This test requires a case-sensitive filesystem: it creates osi/ and OSI/
    # as two distinct directories, which is impossible on macOS where they map
    # to the same physical directory and the second mkdir() raises FileExistsError.
    # The case where one physical directory is reachable via both names is covered
    # by test_duplicate_resolved_path_is_scanned_once.
    @pytest.mark.skipif(
        __import__("sys").platform == "darwin",
        reason="macOS uses a case-insensitive filesystem by default",
    )
    def test_both_osi_directories_scanned_when_present(self, tmp_path):
        (tmp_path / "osi").mkdir()
        (tmp_path / "OSI").mkdir()
        (tmp_path / "osi" / "lower.json").write_text("{}")
        (tmp_path / "OSI" / "upper.json").write_text("{}")
        result = _scan_osi_directories(str(tmp_path), ["osi", "OSI"])
        assert {p.name for p in result} == {"lower.json", "upper.json"}

    def test_duplicate_resolved_path_is_scanned_once(self, tmp_path):
        # On case-insensitive filesystems "osi" and "OSI" resolve to the same
        # directory; deduplication must prevent files from appearing twice.
        osi_dir = tmp_path / "osi"
        osi_dir.mkdir(exist_ok=True)
        (osi_dir / "orders.json").write_text("{}")
        result = _scan_osi_directories(str(tmp_path), ["osi", "OSI"])
        assert [p.name for p in result] == ["orders.json"]


class TestBuildModelLookup:
    def test_indexes_model_by_alias_schema_database(self):
        model = _orders_model()
        manifest = make_manifest(nodes=[model])
        lookup = _build_model_lookup(manifest)
        key = (model.alias.lower(), model.schema.lower(), (model.database or "").lower())
        assert key in lookup
        assert lookup[key] is model

    def test_name_used_when_no_alias(self):
        model = make_model(PKG, "orders", "select 1", alias=None)
        manifest = make_manifest(nodes=[model])
        lookup = _build_model_lookup(manifest)
        # make_model sets alias=name by default when alias=None, so check name fallback path
        # by directly testing the key uses the model name
        assert ("orders", model.schema.lower(), (model.database or "").lower()) in lookup

    def test_excludes_non_model_nodes(self):
        from tests.unit.utils.manifest import make_seed

        seed = make_seed(PKG, "my_seed")
        manifest = make_manifest(nodes=[seed])
        assert _build_model_lookup(manifest) == {}

    def test_empty_manifest_returns_empty(self):
        manifest = make_manifest()
        assert _build_model_lookup(manifest) == {}

    def test_keys_are_lowercased(self):
        model = make_model(PKG, "Orders", "select 1", alias="Orders")
        manifest = make_manifest(nodes=[model])
        lookup = _build_model_lookup(manifest)
        assert ("orders", model.schema.lower(), (model.database or "").lower()) in lookup


OSI_PATHS = ["OSI"]


class TestLoadOsiIntoManifest:
    def test_no_osi_directory_is_noop(self, tmp_path):
        manifest = make_manifest(nodes=[_orders_model()])
        load_osi_into_manifest(str(tmp_path), PKG, manifest, OSI_PATHS)
        assert manifest.semantic_models == {}
        assert manifest.metrics == {}

    def test_empty_osi_directory_is_noop(self, tmp_path):
        (tmp_path / "OSI").mkdir()
        manifest = make_manifest(nodes=[_orders_model()])
        load_osi_into_manifest(str(tmp_path), PKG, manifest, OSI_PATHS)
        assert manifest.semantic_models == {}
        assert manifest.metrics == {}

    def test_injects_semantic_model(self, tmp_path):
        (tmp_path / "OSI").mkdir()
        (tmp_path / "OSI" / "orders.json").write_text(_osi_json())
        manifest = make_manifest(nodes=[_orders_model()])
        load_osi_into_manifest(str(tmp_path), PKG, manifest, OSI_PATHS)
        uid = f"semantic_model.{PKG}.orders"
        assert uid in manifest.semantic_models
        sm = manifest.semantic_models[uid]
        assert sm.name == "orders"
        assert sm.package_name == PKG
        assert sm.path == os.path.join("OSI", "orders.json")

    def test_injected_semantic_model_refs_matched_dbt_model(self, tmp_path):
        (tmp_path / "OSI").mkdir()
        (tmp_path / "OSI" / "orders.json").write_text(_osi_json())
        manifest = make_manifest(nodes=[_orders_model()])
        load_osi_into_manifest(str(tmp_path), PKG, manifest, OSI_PATHS)
        from dbt.artifacts.resources import RefArgs

        sm = manifest.semantic_models[f"semantic_model.{PKG}.orders"]
        assert sm.refs == [RefArgs(name="orders", package=None, version=None)]

    def test_multiple_files_all_ingested(self, tmp_path):
        osi_dir = tmp_path / "OSI"
        osi_dir.mkdir()
        customers_model = make_model(PKG, "customers", "select 1")
        (osi_dir / "orders.json").write_text(_osi_json("orders", "dbt.dbt_schema.orders"))
        (osi_dir / "customers.json").write_text(_osi_json("customers", "dbt.dbt_schema.customers"))
        manifest = make_manifest(nodes=[_orders_model(), customers_model])
        load_osi_into_manifest(str(tmp_path), PKG, manifest, OSI_PATHS)
        assert f"semantic_model.{PKG}.orders" in manifest.semantic_models
        assert f"semantic_model.{PKG}.customers" in manifest.semantic_models

    def test_invalid_json_raises_parsing_error(self, tmp_path):
        (tmp_path / "OSI").mkdir()
        (tmp_path / "OSI" / "bad.json").write_text("{not valid json")
        manifest = make_manifest(nodes=[_orders_model()])
        with pytest.raises(ParsingError, match="Failed to parse OSI file"):
            load_osi_into_manifest(str(tmp_path), PKG, manifest, OSI_PATHS)

    def test_unsupported_version_raises_parsing_error(self, tmp_path):
        (tmp_path / "OSI").mkdir()
        (tmp_path / "OSI" / "v99.json").write_text(_osi_json(version="9.9.9"))
        manifest = make_manifest(nodes=[_orders_model()])
        with pytest.raises(ParsingError, match="unsupported version"):
            load_osi_into_manifest(str(tmp_path), PKG, manifest, OSI_PATHS)

    def test_database_less_source_binds_by_schema_and_alias(self, tmp_path):
        (tmp_path / "OSI").mkdir()
        (tmp_path / "OSI" / "orders.json").write_text(_osi_json("orders", "dbt_schema.orders"))
        manifest = make_manifest(nodes=[_orders_model()])
        load_osi_into_manifest(str(tmp_path), PKG, manifest, OSI_PATHS)
        sm = manifest.semantic_models[f"semantic_model.{PKG}.orders"]
        assert sm.model == "ref('orders')"

    def test_database_less_source_ambiguous_across_databases_raises(self, tmp_path):
        (tmp_path / "OSI").mkdir()
        (tmp_path / "OSI" / "orders.json").write_text(_osi_json("orders", "dbt_schema.orders"))
        model_a = _orders_model()
        model_b = make_model(PKG, "orders_other_db", "select 1", alias=model_a.alias)
        model_b.schema = model_a.schema
        model_b.database = "another_db"
        manifest = make_manifest(nodes=[model_a, model_b])
        with pytest.raises(ParsingError, match="matches models in multiple databases"):
            load_osi_into_manifest(str(tmp_path), PKG, manifest, OSI_PATHS)

    def test_no_matching_model_raises_parsing_error(self, tmp_path):
        (tmp_path / "OSI").mkdir()
        # Source references a table that has no corresponding model node
        (tmp_path / "OSI" / "orphan.json").write_text(
            _osi_json("orphan", source="unknown.schema.orphan")
        )
        manifest = make_manifest(nodes=[_orders_model()])
        with pytest.raises(ParsingError, match="does not match any dbt model"):
            load_osi_into_manifest(str(tmp_path), PKG, manifest, OSI_PATHS)

    def test_duplicate_semantic_model_raises_parsing_error(self, tmp_path):
        from tests.unit.utils.manifest import make_semantic_model

        osi_dir = tmp_path / "OSI"
        osi_dir.mkdir()
        (osi_dir / "orders.json").write_text(_osi_json())
        model = _orders_model()
        existing_sm = make_semantic_model(PKG, "orders", model=model)
        existing_sm = existing_sm.__class__(
            **{**existing_sm.__dict__, "unique_id": f"semantic_model.{PKG}.orders"}
        )
        manifest = make_manifest(nodes=[model], semantic_models=[existing_sm])
        with pytest.raises(ParsingError, match="conflicts with an existing semantic model"):
            load_osi_into_manifest(str(tmp_path), PKG, manifest, OSI_PATHS)

    def test_duplicate_metric_raises_parsing_error(self, tmp_path):
        osi_dir = tmp_path / "OSI"
        osi_dir.mkdir()
        (osi_dir / "orders.json").write_text(_osi_json())

        model = _orders_model()
        manifest = make_manifest(nodes=[model])

        fake_metric = MagicMock()
        fake_metric.name = "revenue"
        fake_metric.description = ""
        fake_metric.label = "Revenue"
        fake_metric.dict.return_value = {
            "name": "revenue",
            "description": "",
            "label": "Revenue",
        }

        fake_output = MagicMock()
        fake_output.semantic_models = []
        fake_output.metrics = [fake_metric]

        fake_result = MagicMock()
        fake_result.output = fake_output
        fake_result.issues = []

        # Pre-populate manifest with the same metric unique_id
        from tests.unit.utils.manifest import make_metric

        existing_metric = make_metric(PKG, "revenue")
        manifest.metrics[f"metric.{PKG}.revenue"] = existing_metric

        with patch("dbt.parser.osi.OSIToMSIConverter") as MockConverter:
            MockConverter.return_value.convert.return_value = fake_result
            with pytest.raises(ParsingError, match="conflicts with an existing metric"):
                load_osi_into_manifest(str(tmp_path), PKG, manifest, OSI_PATHS)

    def test_converter_issues_fire_events(self, tmp_path):
        from metricflow.converters.converter_issues import (
            ConverterIssue,
            ConverterIssueType,
        )

        osi_dir = tmp_path / "OSI"
        osi_dir.mkdir()
        (osi_dir / "orders.json").write_text(_osi_json())

        manifest = make_manifest(nodes=[_orders_model()])

        fake_output = MagicMock()
        fake_output.semantic_models = []
        fake_output.metrics = []

        issue = ConverterIssue(
            issue_type=ConverterIssueType.CONVERSION_METRIC_DROPPED,
            element_name="revenue",
        )
        fake_result = MagicMock()
        fake_result.output = fake_output
        fake_result.issues = [issue]

        catcher = EventCatcher(MFConverterIssue)
        add_callback_to_manager(catcher.catch)
        with patch("dbt.parser.osi.OSIToMSIConverter") as MockConverter:
            MockConverter.return_value.convert.return_value = fake_result
            load_osi_into_manifest(str(tmp_path), PKG, manifest, OSI_PATHS)

        assert len(catcher.caught_events) == 1
        event_data = catcher.caught_events[0].data
        assert event_data.element_name == "revenue"
        assert event_data.converter_name == "OSIToMSIConverter"
        assert event_data.issue_type == ConverterIssueType.CONVERSION_METRIC_DROPPED.value

    def test_custom_osi_path_is_used(self, tmp_path):
        custom_dir = tmp_path / "custom_osi"
        custom_dir.mkdir()
        (custom_dir / "orders.json").write_text(_osi_json())
        manifest = make_manifest(nodes=[_orders_model()])
        load_osi_into_manifest(str(tmp_path), PKG, manifest, ["custom_osi"])
        uid = f"semantic_model.{PKG}.orders"
        assert uid in manifest.semantic_models
        sm = manifest.semantic_models[uid]
        assert sm.path == os.path.join("custom_osi", "orders.json")


def _make_osi_source_file(project_root: str, rel_path: str, pkg: str) -> OsiSourceFile:
    parts = rel_path.split("/", 1)
    fp = FilePath(
        searched_path=parts[0],
        relative_path=parts[1] if len(parts) > 1 else "",
        modification_time=0.0,
        project_root=project_root,
    )
    return OsiSourceFile(
        path=fp,
        checksum=FileHash.from_contents("{}"),
        parse_file_type=ParseFileType.OSI,
        project_name=pkg,
    )


class TestOsiAttribution:
    def test_attribution_populated_after_load(self, tmp_path):
        (tmp_path / "OSI").mkdir()
        (tmp_path / "OSI" / "orders.json").write_text(_osi_json())
        manifest = make_manifest(nodes=[_orders_model()])
        osi_sf = _make_osi_source_file(str(tmp_path), "OSI/orders.json", PKG)
        file_id = osi_sf.file_id
        manifest.files[file_id] = osi_sf

        load_osi_into_manifest(str(tmp_path), PKG, manifest, OSI_PATHS)

        sf = manifest.files[file_id]
        assert isinstance(sf, OsiSourceFile)
        assert f"semantic_model.{PKG}.orders" in sf.semantic_models

    def test_clear_osi_attributed_nodes_removes_semantic_models(self, tmp_path):
        manifest = make_manifest(nodes=[_orders_model()])
        osi_sf = _make_osi_source_file(str(tmp_path), "OSI/orders.json", PKG)
        uid = f"semantic_model.{PKG}.orders"
        osi_sf.semantic_models.append(uid)

        from tests.unit.utils.manifest import make_semantic_model

        sm = make_semantic_model(PKG, "orders", model=_orders_model())
        # OSI-sourced nodes always have original_file_path under the OSI directory
        sm = sm.__class__(**{**sm.__dict__, "original_file_path": "OSI/orders.json"})
        manifest.semantic_models[uid] = sm
        manifest.files[osi_sf.file_id] = osi_sf

        _clear_osi_attributed_nodes(manifest, OSI_PATHS)

        assert uid not in manifest.semantic_models
        assert osi_sf.semantic_models == []

    def test_clear_osi_attributed_nodes_resets_lists(self, tmp_path):
        manifest = make_manifest()
        osi_sf = _make_osi_source_file(str(tmp_path), "OSI/orders.json", PKG)
        osi_sf.semantic_models.append("semantic_model.pkg.x")
        osi_sf.metrics.append("metric.pkg.y")
        manifest.files[osi_sf.file_id] = osi_sf

        _clear_osi_attributed_nodes(manifest, OSI_PATHS)

        assert osi_sf.semantic_models == []
        assert osi_sf.metrics == []

    def test_clear_osi_is_noop_on_non_osi_files(self, tmp_path):
        from dbt.contracts.files import SchemaSourceFile

        manifest = make_manifest()
        uid = "semantic_model.pkg.x"
        from tests.unit.utils.manifest import make_semantic_model

        sm = make_semantic_model(PKG, "x", model=_orders_model())
        manifest.semantic_models[uid] = sm
        manifest.files["pkg://models/foo.yml"] = MagicMock(spec=SchemaSourceFile)

        _clear_osi_attributed_nodes(manifest, OSI_PATHS)

        assert uid in manifest.semantic_models

    def test_load_clears_before_reinject(self, tmp_path):
        (tmp_path / "OSI").mkdir()
        (tmp_path / "OSI" / "orders.json").write_text(_osi_json())
        manifest = make_manifest(nodes=[_orders_model()])

        osi_sf = _make_osi_source_file(str(tmp_path), "OSI/orders.json", PKG)
        uid = f"semantic_model.{PKG}.orders"

        from tests.unit.utils.manifest import make_semantic_model

        stale_sm = make_semantic_model(PKG, "orders", model=_orders_model())
        stale_sm = stale_sm.__class__(
            **{
                **stale_sm.__dict__,
                "unique_id": uid,
                "original_file_path": "OSI/orders.json",
            }
        )
        manifest.semantic_models[uid] = stale_sm
        osi_sf.semantic_models.append(uid)
        manifest.files[osi_sf.file_id] = osi_sf

        load_osi_into_manifest(str(tmp_path), PKG, manifest, OSI_PATHS)

        assert uid in manifest.semantic_models
        assert manifest.files[osi_sf.file_id].semantic_models == [uid]
