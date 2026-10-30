import pytest

from dbt.exceptions import CompilationError, ParsingError
from dbt.tests.util import check_relations_equal, get_manifest, run_dbt, write_file

models_alt__schema_yml = """
version: 2
sources:
  - name: raw
    database: "{{ target.database }}"
    schema: "{{ target.schema }}"
    tables:
      - name: 'some_seed'
        columns:
          - name: id

models:
  - name: model
    description: "This is a model description"
    config:
        tags: ['tag_in_schema']
        meta:
            owner: 'Julie Smith'
            my_attr: "{{ var('my_var') }}"
        materialized: view

    columns:
      - name: id
        data_tests:
          - not_null:
              meta:
                  owner: 'Simple Simon'
          - unique:
              config:
                  meta:
                      owner: 'John Doe'
"""

models_alt__untagged_sql = """
{{
    config(materialized='table')
}}

select id, value from {{ source('raw', 'some_seed') }}
"""

models_alt__tagged__model_sql = """
{{
    config(
        materialized='view',
        tags=['tag_1_in_model'],
    )
}}

{{
    config(
        materialized='table',
        tags=['tag_2_in_model'],
    )
}}

select 4 as id, 2 as value
"""

models_no_materialized__model_sql = """
{{
    config(
        tags=['tag_1_in_model'],
    )
}}

{{
    config(
        tags=['tag_2_in_model'],
    )
}}

select 4 as id, 2 as value
"""

seeds_alt__some_seed_csv = """id,value
4,2
"""

extra_alt__untagged_yml = """
version: 2

models:
  - name: untagged
    description: "This is a model description"
    meta:
      owner: 'Somebody Else'
    config:
        meta:
            owner: 'Julie Smith'
"""

extra_alt__untagged2_yml = """
version: 2

models:
  - name: untagged
    description: "This is a model description"
    data_tests:
      - not_null:
          error_if: ">2"
          config:
            error_if: ">2"
"""


class TestSchemaFileConfigs:
    @pytest.fixture(scope="class")
    def expected_unrendered_config(self):
        # my_attr is unrendered when state_modified_compare_more_unrendered_values: True
        # inherited merge-behavior fields (meta, tags) accumulate across the
        # project hierarchy instead of keeping only the innermost scope
        return {
            "materialized": "view",
            "meta": {
                "company": "NuMade",
                "project": "test",
                "team": "Core Team",
                "my_attr": "{{ var('my_var') }}",
                "owner": "Julie Smith",
            },
            # tags accumulate across project scopes, the schema-file patch and
            # config() calls; the duplicate tag_in_schema comes from the patch
            # dict being deep-merged with its unrendered form (parser/base.py)
            "tags": [
                "tag_in_project",
                "tag_in_schema",
                "tag_in_schema",
                "tag_1_in_model",
                "tag_2_in_model",
            ],
        }

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": models_alt__schema_yml,
            "untagged.sql": models_alt__untagged_sql,
            "tagged": {"model.sql": models_alt__tagged__model_sql},
        }

    @pytest.fixture(scope="class")
    def seeds(self):
        return {"some_seed.csv": seeds_alt__some_seed_csv}

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "flags": {
                "state_modified_compare_more_unrendered_values": True,
            },
            "models": {
                "+meta": {
                    "company": "NuMade",
                },
                "test": {
                    "+meta": {
                        "project": "test",
                    },
                    "tagged": {
                        "+meta": {
                            "team": "Core Team",
                        },
                        "tags": ["tag_in_project"],
                        "model": {
                            "materialized": "table",
                            "+meta": {
                                "owner": "Julie Dent",
                            },
                        },
                    },
                },
            },
            "vars": {
                "test": {
                    "my_var": "TESTING",
                }
            },
            "seeds": {
                "quote_columns": False,
            },
        }

    def test_config_layering(
        self,
        project,
        expected_unrendered_config,
    ):

        # run seed
        assert len(run_dbt(["seed"])) == 1

        # test the project-level tag, and both config() call tags
        assert len(run_dbt(["run", "--model", "tag:tag_in_project"])) == 1
        assert len(run_dbt(["run", "--model", "tag:tag_1_in_model"])) == 1
        assert len(run_dbt(["run", "--model", "tag:tag_2_in_model"])) == 1
        assert len(run_dbt(["run", "--model", "tag:tag_in_schema"])) == 1

        # Verify that model nodes have expected tags and meta
        manifest = get_manifest(project.project_root)
        model_id = "model.test.model"
        model_node = manifest.nodes[model_id]
        meta_expected = {
            "company": "NuMade",
            "project": "test",
            "team": "Core Team",
            "owner": "Julie Smith",
            "my_attr": "TESTING",
        }
        assert model_node.meta == meta_expected
        assert model_node.config.meta == meta_expected
        model_tags = ["tag_1_in_model", "tag_2_in_model", "tag_in_project", "tag_in_schema"]
        model_node_tags = model_node.tags.copy()
        model_node_tags.sort()
        assert model_node_tags == model_tags
        model_node_config_tags = model_node.config.tags.copy()
        model_node_config_tags.sort()
        assert model_node_config_tags == model_tags
        model_meta = {
            "company": "NuMade",
            "project": "test",
            "team": "Core Team",
            "owner": "Julie Smith",
            "my_attr": "TESTING",
        }
        assert model_node.config.meta == model_meta

        # make sure we overwrote the materialization properly
        tables = project.get_tables_in_schema()
        assert tables["model"] == "table"
        check_relations_equal(project.adapter, ["some_seed", "model"])

        # Remove materialized config from model
        write_file(
            models_no_materialized__model_sql,
            project.project_root,
            "models",
            "tagged",
            "model.sql",
        )
        results = run_dbt(["run"])
        assert len(results) == 2
        manifest = get_manifest(project.project_root)
        model_node = manifest.nodes[model_id]

        assert model_node.config.materialized == "view"
        model_unrendered_config = expected_unrendered_config
        assert model_node.unrendered_config == model_unrendered_config

        # look for test meta
        schema_file_id = model_node.patch_path
        schema_file = manifest.files[schema_file_id]
        tests = schema_file.get_tests("models", "model")
        assert tests[0] in manifest.nodes
        test = manifest.nodes[tests[0]]
        expected_meta = {"owner": "Simple Simon"}
        assert test.config.meta == expected_meta
        test = manifest.nodes[tests[1]]
        expected_meta = {"owner": "John Doe"}
        assert test.config.meta == expected_meta

        # copy a schema file with multiple metas
        #       shutil.copyfile('extra-alt/untagged.yml', 'models-alt/untagged.yml')
        write_file(extra_alt__untagged_yml, project.project_root, "models", "untagged.yml")
        with pytest.raises(ParsingError):
            run_dbt(["run"])

        # copy a schema file with config key in top-level of test and in config dict
        #       shutil.copyfile('extra-alt/untagged2.yml', 'models-alt/untagged.yml')
        write_file(extra_alt__untagged2_yml, project.project_root, "models", "untagged.yml")
        with pytest.raises(CompilationError):
            run_dbt(["run"])


class TestLegacySchemaFileConfigs(TestSchemaFileConfigs):
    @pytest.fixture(scope="class")
    def expected_unrendered_config(self):
        # my_attr is rendered ("TESTING") when state_modified_compare_more_unrendered_values: False
        return {
            "materialized": "view",
            "meta": {"my_attr": "TESTING", "owner": "Julie Smith"},
            "tags": ["tag_1_in_model", "tag_2_in_model"],
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "flags": {
                "state_modified_compare_more_unrendered_values": False,
            },
            "models": {
                "+meta": {
                    "company": "NuMade",
                },
                "test": {
                    "+meta": {
                        "project": "test",
                    },
                    "tagged": {
                        "+meta": {
                            "team": "Core Team",
                        },
                        "tags": ["tag_in_project"],
                        "model": {
                            "materialized": "table",
                            "+meta": {
                                "owner": "Julie Dent",
                            },
                        },
                    },
                },
            },
            "vars": {
                "test": {
                    "my_var": "TESTING",
                }
            },
            "seeds": {
                "quote_columns": False,
            },
        }


list_schema_yml = """
- name: my_name
- name: alt_name
"""


class TestListSchemaFile:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "my_model.sql": "select 1 as id",
            "schema.yml": list_schema_yml,
        }

    def test_list_schema(self, project):
        with pytest.raises(ParsingError) as excinfo:
            run_dbt(["run"])
        assert "Dictionary expected" in str(excinfo.value)
