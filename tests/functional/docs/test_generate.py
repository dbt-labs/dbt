from unittest import mock

import pytest

from dbt.plugins.manifest import ModelNodeArgs, PluginNodes
from dbt.tests.util import get_manifest, run_dbt

sample_seed = """sample_num,sample_bool
1,true
2,false
3,true
"""

second_seed = """sample_num,sample_bool
4,true
5,false
6,true
"""

sample_config = """
sources:
  - name: my_source_schema
    schema: "{{ target.schema }}"
    tables:
      - name: sample_source
      - name: second_source
      - name: non_existent_source
      - name: source_from_seed
"""


class TestBaseGenerate:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "my_model.sql": "select 1 as fun",
            "alt_model.sql": "select 1 as notfun",
            "sample_config.yml": sample_config,
        }

    @pytest.fixture(scope="class")
    def seeds(self):
        return {
            "sample_seed.csv": sample_seed,
            "second_seed.csv": sample_seed,
        }


class TestGenerateManifestNotCompiled(TestBaseGenerate):
    def test_manifest_not_compiled(self, project):
        run_dbt(["docs", "generate", "--no-compile"])
        # manifest.json is written out in parsing now, but it
        # shouldn't be compiled because of the --no-compile flag
        manifest = get_manifest(project.project_root)
        model_id = "model.test.my_model"
        assert model_id in manifest.nodes
        assert manifest.nodes[model_id].compiled is False


class TestGenerateEmptyCatalog(TestBaseGenerate):
    def test_generate_empty_catalog(self, project):
        catalog = run_dbt(["docs", "generate", "--empty-catalog"])
        assert catalog.nodes == {}, "nodes should be empty"
        assert catalog.sources == {}, "sources should be empty"
        assert catalog.errors is None, "errors should be null"


class TestGenerateSelectLimitsCatalog(TestBaseGenerate):
    def test_select_limits_catalog(self, project):
        run_dbt(["run"])
        catalog = run_dbt(["docs", "generate", "--select", "my_model"])
        assert len(catalog.nodes) == 1
        assert "model.test.my_model" in catalog.nodes


class TestGenerateSelectLimitsNoMatch(TestBaseGenerate):
    def test_select_limits_no_match(self, project):
        run_dbt(["run"])
        catalog = run_dbt(["docs", "generate", "--select", "my_missing_model"])
        assert len(catalog.nodes) == 0
        assert len(catalog.sources) == 0


class TestGenerateSelectLimitsCatalogNoCompile(TestBaseGenerate):
    def test_select_limits_catalog_no_compile(self, project):
        run_dbt(["run"])
        catalog = run_dbt(["docs", "generate", "--select", "my_model", "--no-compile"])
        assert list(catalog.nodes) == ["model.test.my_model"]
        assert len(catalog.sources) == 0


class TestGenerateSelectSourceNoCompile(TestBaseGenerate):
    def test_select_source_no_compile(self, project):
        project.run_sql("create table {}.sample_source (id int)".format(project.test_schema))
        project.run_sql("create table {}.second_source (id int)".format(project.test_schema))
        run_dbt(["run"])
        catalog = run_dbt(
            [
                "docs",
                "generate",
                "--select",
                "source:test.my_source_schema.sample_source",
                "--no-compile",
            ]
        )
        assert len(catalog.nodes) == 0
        assert list(catalog.sources) == ["source.test.my_source_schema.sample_source"]


class TestGenerateSelectLimitsNoMatchNoCompile(TestBaseGenerate):
    def test_select_limits_no_match_no_compile(self, project):
        run_dbt(["run"])
        catalog = run_dbt(["docs", "generate", "--select", "my_missing_model", "--no-compile"])
        assert len(catalog.nodes) == 0
        assert len(catalog.sources) == 0


class TestGenerateNoSelectNoCompile(TestBaseGenerate):
    def test_no_select_no_compile(self, project):
        project.run_sql("create table {}.sample_source (id int)".format(project.test_schema))
        project.run_sql("create table {}.second_source (id int)".format(project.test_schema))
        run_dbt(["run"])
        catalog = run_dbt(["docs", "generate", "--no-compile"])
        assert set(catalog.nodes) == {"model.test.my_model", "model.test.alt_model"}
        assert set(catalog.sources) == {
            "source.test.my_source_schema.sample_source",
            "source.test.my_source_schema.second_source",
        }


class TestGenerateCatalogWithSources(TestBaseGenerate):
    def test_catalog_with_sources(self, project):
        # populate sources other than non_existent_source
        project.run_sql("create table {}.sample_source (id int)".format(project.test_schema))
        project.run_sql("create table {}.second_source (id int)".format(project.test_schema))

        # build nodes
        run_dbt(["build"])

        catalog = run_dbt(["docs", "generate"])

        # 2 seeds + 2 models
        assert len(catalog.nodes) == 4
        # 2 sources (only ones that exist)
        assert len(catalog.sources) == 2


class TestGenerateCatalogWithExternalNodes(TestBaseGenerate):
    @mock.patch("dbt.plugins.get_plugin_manager")
    def test_catalog_with_external_node(self, get_plugin_manager, project):
        project.run_sql("create table {}.external_model (id int)".format(project.test_schema))

        run_dbt(["build"])

        external_nodes = PluginNodes()
        external_model_node = ModelNodeArgs(
            name="external_model",
            package_name="external_package",
            identifier="external_model",
            schema=project.test_schema,
            database="dbt",
        )
        external_nodes.add_model(external_model_node)
        get_plugin_manager.return_value.get_nodes.return_value = external_nodes
        catalog = run_dbt(["docs", "generate"])

        assert "model.external_package.external_model" in catalog.nodes


class TestGenerateSelectSource(TestBaseGenerate):
    @pytest.fixture(scope="class")
    def seeds(self):
        return {
            "sample_seed.csv": sample_seed,
            "second_seed.csv": sample_seed,
            "source_from_seed.csv": sample_seed,
        }

    def test_select_source(self, project):
        run_dbt(["build"])

        project.run_sql("create table {}.sample_source (id int)".format(project.test_schema))
        project.run_sql("create table {}.second_source (id int)".format(project.test_schema))

        # 2 existing sources, 1 selected
        catalog = run_dbt(
            ["docs", "generate", "--select", "source:test.my_source_schema.sample_source"]
        )
        assert len(catalog.sources) == 1
        assert "source.test.my_source_schema.sample_source" in catalog.sources
        # no nodes selected
        assert len(catalog.nodes) == 0

        # 2 existing sources sources, 1 selected that has relation as a seed
        catalog = run_dbt(
            ["docs", "generate", "--select", "source:test.my_source_schema.source_from_seed"]
        )
        assert len(catalog.sources) == 1
        assert "source.test.my_source_schema.source_from_seed" in catalog.sources
        # seed with same relation that was not selected not in catalog
        assert len(catalog.nodes) == 0


class TestGenerateSelectOverMaxSchemaMetadataRelations(TestBaseGenerate):
    @pytest.fixture(scope="class")
    def seeds(self):
        return {
            "sample_seed.csv": sample_seed,
            "second_seed.csv": sample_seed,
            "source_from_seed.csv": sample_seed,
        }

    def test_select_source(self, project):
        run_dbt(["build"])

        project.run_sql("create table {}.sample_source (id int)".format(project.test_schema))
        project.run_sql("create table {}.second_source (id int)".format(project.test_schema))

        with mock.patch.object(type(project.adapter), "MAX_SCHEMA_METADATA_RELATIONS", 1):
            # more relations than MAX_SCHEMA_METADATA_RELATIONS -> all sources and nodes correctly returned
            catalog = run_dbt(["docs", "generate"])
            assert len(catalog.sources) == 3
            assert len(catalog.nodes) == 5

            # full source selection respected
            catalog = run_dbt(["docs", "generate", "--select", "source:*"])
            assert len(catalog.sources) == 3
            assert len(catalog.nodes) == 0

            # full node selection respected
            catalog = run_dbt(["docs", "generate", "--exclude", "source:*"])
            assert len(catalog.sources) == 0
            assert len(catalog.nodes) == 5

            # granular source selection respected (> MAX_SCHEMA_METADATA_RELATIONS selected sources)
            catalog = run_dbt(
                [
                    "docs",
                    "generate",
                    "--select",
                    "source:test.my_source_schema.sample_source",
                    "source:test.my_source_schema.second_source",
                ]
            )
            assert len(catalog.sources) == 2
            assert len(catalog.nodes) == 0

            # granular node selection respected (> MAX_SCHEMA_METADATA_RELATIONS selected nodes)
            catalog = run_dbt(["docs", "generate", "--select", "my_model", "alt_model"])
            assert len(catalog.sources) == 0
            assert len(catalog.nodes) == 2


class TestGenerateSelectSeed(TestBaseGenerate):
    @pytest.fixture(scope="class")
    def seeds(self):
        return {
            "sample_seed.csv": sample_seed,
            "second_seed.csv": sample_seed,
            "source_from_seed.csv": sample_seed,
        }

    def test_select_seed(self, project):
        run_dbt(["build"])

        # 3 seeds, 1 selected
        catalog = run_dbt(["docs", "generate", "--select", "sample_seed"])
        assert len(catalog.nodes) == 1
        assert "seed.test.sample_seed" in catalog.nodes
        # no sources selected
        assert len(catalog.sources) == 0

        # 3 seeds, 1 selected that has same relation as a source
        catalog = run_dbt(["docs", "generate", "--select", "source_from_seed"])
        assert len(catalog.nodes) == 1
        assert "seed.test.source_from_seed" in catalog.nodes
        # source with same relation that was not selected not in catalog
        assert len(catalog.sources) == 0
