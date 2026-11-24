"""Functional tests for catalogs.yml v2 parsing and bridge flow."""

from unittest import mock

import pytest

from dbt.adapters.capability import CapabilityDict, CapabilitySupport, Support
from dbt.adapters.catalogs import CatalogIntegration, CatalogIntegrationConfig
from dbt.contracts.results import TestStatus
from dbt.tests.util import run_dbt, write_config_file, write_file
from dbt_common.exceptions import DbtValidationError

_SENTINEL_CATALOGS_V2 = object()


class _MockCapabilityWithV2:
    """Stub for dbt.cli.requires.Capability that always has CatalogsV2 defined."""

    CatalogsV2 = _SENTINEL_CATALOGS_V2


class _AllSupportedCapabilityDict(CapabilityDict):
    """Returns Full support for any capability — used in tests to bypass capability checks."""

    def __missing__(self, key):
        return CapabilitySupport(support=Support.Full)


class StubCatalogIntegration(CatalogIntegration):
    """Minimal integration that accepts any catalog_type and stores adapter_properties."""

    catalog_type = "stub"
    allows_writes = True

    def __init__(self, config: CatalogIntegrationConfig):
        super().__init__(config)
        for key, value in config.adapter_properties.items():
            setattr(self, key, value)


def _catalog_entry(name, type_="horizon", table_format="iceberg", config=None):
    return {"name": name, "type": type_, "table_format": table_format, "config": config or {}}


def _catalogs_file(*entries):
    return {"catalogs": list(entries)}


def _mock_bridge(catalog_type: str):
    """Return a bridge side_effect that produces a CatalogWriteIntegrationConfig
    with catalog_type='stub' so StubCatalogIntegration is registered."""
    from dbt.artifacts.resources import CatalogWriteIntegrationConfig

    def _bridge(self, catalog):
        platform_block = catalog.config.get("snowflake", {}) or {}
        external_volume = platform_block.get("external_volume")
        file_format = platform_block.get("file_format")
        props = {
            k: v for k, v in platform_block.items() if k not in {"external_volume", "file_format"}
        }
        return CatalogWriteIntegrationConfig(
            name=catalog.name,
            catalog_type="stub",
            catalog_name=catalog.name,
            table_format=catalog.table_format.value,
            external_volume=str(external_volume) if external_volume is not None else None,
            file_format=str(file_format) if file_format is not None else None,
            adapter_properties=props,
        )

    return _bridge


@pytest.fixture
def v2_catalog_adapter(project):
    adapter_type = type(project.adapter)
    with (
        mock.patch("dbt.cli.requires.Capability", _MockCapabilityWithV2),
        mock.patch.object(adapter_type, "CATALOG_INTEGRATIONS", [StubCatalogIntegration]),
        mock.patch.object(
            adapter_type, "capabilities", return_value=_AllSupportedCapabilityDict({})
        ),
        mock.patch.object(adapter_type, "bridge_v2_catalog", _mock_bridge("horizon"), create=True),
    ):
        yield


class TestV2HappyPath:
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"flags": {"use_catalogs_v2": True}}

    @pytest.fixture
    def catalogs(self):
        return _catalogs_file(
            _catalog_entry(
                "my_catalog",
                config={"snowflake": {"external_volume": "s3_vol", "change_tracking": True}},
            )
        )

    def test_catalog_registered(self, project, catalogs, v2_catalog_adapter):
        write_config_file(catalogs, project.project_root, "catalogs.yml")

        run_dbt(["run"])
        integration = project.adapter.get_catalog_integration("my_catalog")
        assert isinstance(integration, StubCatalogIntegration)
        assert integration.name == "my_catalog"
        assert integration.table_format == "iceberg"
        assert integration.external_volume == "s3_vol"
        assert integration.change_tracking is True


class TestV2Retry:
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"flags": {"use_catalogs_v2": True}}

    @pytest.fixture(scope="class")
    def tests(self):
        return {"fails.sql": "select 1"}

    def test_retry_registers_catalog_with_resolved_vars(self, project, v2_catalog_adapter):
        catalogs = _catalogs_file(
            _catalog_entry(
                "my_catalog",
                type_="glue",
                config={"snowflake": {"external_volume": "{{ var('volume', 'initial_vol') }}"}},
            )
        )
        write_config_file(catalogs, project.project_root, "catalogs.yml")

        build_results = run_dbt(["build"], expect_pass=False)
        assert [result.status for result in build_results] == [TestStatus.Fail]
        assert (
            project.adapter.get_catalog_integration("my_catalog").external_volume == "initial_vol"
        )

        write_file("select 1 where false", "tests", "fails.sql")
        retry_results = run_dbt(["retry", "--vars", "{volume: retry_vol}"])

        assert [result.status for result in retry_results] == [TestStatus.Pass]
        assert project.adapter.get_catalog_integration("my_catalog").external_volume == "retry_vol"


class TestV2MultipleCatalogs:
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"flags": {"use_catalogs_v2": True}}

    @pytest.fixture
    def catalogs(self):
        return _catalogs_file(
            _catalog_entry("cat_a", config={"snowflake": {"external_volume": "vol_a"}}),
            _catalog_entry(
                "cat_b", type_="glue", config={"snowflake": {"external_volume": "vol_b"}}
            ),
        )

    def test_multiple_catalogs_registered(self, project, catalogs, v2_catalog_adapter):
        write_config_file(catalogs, project.project_root, "catalogs.yml")

        run_dbt(["run"])
        assert project.adapter.get_catalog_integration("cat_a").external_volume == "vol_a"
        assert project.adapter.get_catalog_integration("cat_b").external_volume == "vol_b"


class TestV2NoCatalogsFile:
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"flags": {"use_catalogs_v2": True}}

    def test_no_catalogs_file_runs(self, project, adapter):
        with mock.patch.object(
            type(project.adapter), "capabilities", return_value=_AllSupportedCapabilityDict({})
        ):
            run_dbt(["run"])


class TestV2DuplicateCatalogNames:
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"flags": {"use_catalogs_v2": True}}

    @pytest.fixture
    def catalogs(self):
        return _catalogs_file(
            _catalog_entry("dup", config={"snowflake": {"external_volume": "vol"}}),
            _catalog_entry("dup", type_="glue", config={"snowflake": {"external_volume": "vol"}}),
        )

    def test_duplicate_names_rejected(self, project, catalogs, adapter):
        write_config_file(catalogs, project.project_root, "catalogs.yml")

        with pytest.raises(DbtValidationError, match="Duplicate catalog name"):
            run_dbt(["run"])


class TestV2InvalidTableFormat:
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"flags": {"use_catalogs_v2": True}}

    @pytest.fixture
    def catalogs(self):
        return _catalogs_file(_catalog_entry("cat", table_format="parquet"))

    def test_invalid_table_format_rejected(self, project, catalogs, adapter):
        write_config_file(catalogs, project.project_root, "catalogs.yml")

        with pytest.raises(DbtValidationError, match="Invalid table_format"):
            run_dbt(["run"])


class TestV2MissingRequiredKey:
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"flags": {"use_catalogs_v2": True}}

    @pytest.fixture
    def catalogs(self):
        return _catalogs_file({"name": "cat", "type": "horizon", "table_format": "iceberg"})

    def test_missing_config_rejected(self, project, catalogs, adapter):
        write_config_file(catalogs, project.project_root, "catalogs.yml")

        with pytest.raises(DbtValidationError, match="Missing required keys.*config"):
            run_dbt(["run"])
