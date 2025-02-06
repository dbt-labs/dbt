import os
from datetime import datetime

import pytest

import dbt
from dbt.tests.fixtures.project import write_project_files
from dbt.tests.util import check_datetime_between, get_artifact, rm_file, run_dbt
from tests.functional.adapter.basic.expected_catalog import (
    base_expected_catalog,
    expected_references_catalog,
    no_stats,
)

models__schema_yml = """
version: 2

models:
  - name: model
    description: "The test model"
    docs:
      show: false
    columns:
      - name: id
        description: The user ID number
        data_tests:
          - unique
          - not_null
      - name: first_name
        description: The user's first name
      - name: email
        description: The user's email
      - name: ip_address
        description: The user's IP address
      - name: updated_at
        description: The last time this user's email was updated
    data_tests:
      - test.nothing

  - name: second_model
    description: "The second test model"
    docs:
      show: false
    columns:
      - name: id
        description: The user ID number
      - name: first_name
        description: The user's first name
      - name: email
        description: The user's email
      - name: ip_address
        description: The user's IP address
      - name: updated_at
        description: The last time this user's email was updated


sources:
  - name: my_source
    description: "My source"
    loader: a_loader
    schema: "{{ var('test_schema') }}"
    tables:
      - name: my_table
        description: "My table"
        identifier: seed
        columns:
          - name: id
            description: "An ID field"


exposures:
  - name: simple_exposure
    type: dashboard
    depends_on:
      - ref('model')
      - source('my_source', 'my_table')
    owner:
      email: something@example.com
  - name: notebook_exposure
    type: notebook
    depends_on:
      - ref('model')
      - ref('second_model')
    owner:
      email: something@example.com
      name: Some name
    description: >
      A description of the complex exposure
    maturity: medium
    meta:
      tool: 'my_tool'
      languages:
        - python
    tags: ['my_department']
    url: http://example.com/notebook/1
"""

models__second_model_sql = """
{{
    config(
        materialized='view',
        schema='test',
    )
}}

select * from {{ ref('seed') }}
"""

models__readme_md = """
This is a readme.md file with {{ invalid-ish jinja }} in it
"""

models__model_sql = """
{{
    config(
        materialized='view',
    )
}}

select * from {{ ref('seed') }}
"""

seed__schema_yml = """
version: 2
seeds:
  - name: seed
    description: "The test seed"
    columns:
      - name: id
        description: The user ID number
      - name: first_name
        description: The user's first name
      - name: email
        description: The user's email
      - name: ip_address
        description: The user's IP address
      - name: updated_at
        description: The last time this user's email was updated
"""

seed__seed_csv = """id,first_name,email,ip_address,updated_at
1,Larry,lking0@miitbeian.gov.cn,69.135.206.194,2008-09-12 19:08:31
"""

macros__schema_yml = """
version: 2
macros:
  - name: test_nothing
    description: "{{ doc('macro_info') }}"
    meta:
      some_key: 100
    arguments:
      - name: model
        type: Relation
        description: "{{ doc('macro_arg_info') }}"
"""

macros__macro_md = """
{% docs macro_info %}
My custom test that I wrote that does nothing
{% enddocs %}

{% docs macro_arg_info %}
The model for my custom test
{% enddocs %}
"""

macros__dummy_test_sql = """
{% test nothing(model) %}

-- a silly test to make sure that table-level tests show up in the manifest
-- without a column_name field
select 0

{% endtest %}
"""

snapshot__snapshot_seed_sql = """
{% snapshot snapshot_seed %}
{{
    config(
      unique_key='id',
      strategy='check',
      check_cols='all',
      target_schema=var('alternate_schema')
    )
}}
select * from {{ ref('seed') }}
{% endsnapshot %}
"""

ref_models__schema_yml = """
version: 2

models:
  - name: ephemeral_summary
    description: "{{ doc('ephemeral_summary') }}"
    columns: &summary_columns
      - name: first_name
        description: "{{ doc('summary_first_name') }}"
      - name: ct
        description: "{{ doc('summary_count') }}"
  - name: view_summary
    description: "{{ doc('view_summary') }}"
    columns: *summary_columns

exposures:
  - name: notebook_exposure
    type: notebook
    depends_on:
      - ref('view_summary')
    owner:
      email: something@example.com
      name: Some name
    description: "{{ doc('notebook_info') }}"
    maturity: medium
    url: http://example.com/notebook/1
    meta:
      tool: 'my_tool'
      languages:
        - python
    tags: ['my_department']

"""

ref_sources__schema_yml = """
version: 2
sources:
  - name: my_source
    description: "{{ doc('source_info') }}"
    loader: a_loader
    schema: "{{ var('test_schema') }}"
    tables:
      - name: my_table
        description: "{{ doc('table_info') }}"
        identifier: seed
        columns:
          - name: id
            description: "{{ doc('column_info') }}"
"""

ref_models__view_summary_sql = """
{{
  config(
    materialized = "view"
  )
}}

select first_name, ct from {{ref('ephemeral_summary')}}
order by ct asc

"""

ref_models__ephemeral_summary_sql = """
{{
  config(
    materialized = "table"
  )
}}

select first_name, count(*) as ct from {{ref('ephemeral_copy')}}
group by first_name
order by first_name asc

"""

ref_models__ephemeral_copy_sql = """
{{
  config(
    materialized = "ephemeral"
  )
}}

select * from {{ source("my_source", "my_table") }}

"""

ref_models__docs_md = """
{% docs ephemeral_summary %}
A summmary table of the ephemeral copy of the seed data
{% enddocs %}

{% docs summary_first_name %}
The first name being summarized
{% enddocs %}

{% docs summary_count %}
The number of instances of the first name
{% enddocs %}

{% docs view_summary %}
A view of the summary of the ephemeral copy of the seed data
{% enddocs %}

{% docs source_info %}
My source
{% enddocs %}

{% docs table_info %}
My table
{% enddocs %}

{% docs column_info %}
An ID field
{% enddocs %}

{% docs notebook_info %}
A description of the complex exposure
{% enddocs %}

"""


def verify_catalog(project, expected_catalog, start_time):
    # get the catalog.json
    catalog_path = os.path.join(project.project_root, "target", "catalog.json")
    assert os.path.exists(catalog_path)
    catalog = get_artifact(catalog_path)

    # verify the catalog
    assert set(catalog) == {"errors", "metadata", "nodes", "sources"}
    verify_metadata(
        catalog["metadata"],
        "https://schemas.getdbt.com/dbt/catalog/v1.json",
        start_time,
    )
    assert not catalog["errors"]
    for key in "nodes", "sources":
        for unique_id, expected_node in expected_catalog[key].items():
            found_node = catalog[key][unique_id]
            for node_key in expected_node:
                assert node_key in found_node
                assert (
                    found_node[node_key] == expected_node[node_key]
                ), f"Key '{node_key}' in '{unique_id}' did not match"


def verify_metadata(metadata, dbt_schema_version, start_time):
    assert "generated_at" in metadata
    check_datetime_between(metadata["generated_at"], start=start_time)
    assert "dbt_version" in metadata
    assert metadata["dbt_version"] == dbt.version.__version__
    assert "dbt_schema_version" in metadata
    assert metadata["dbt_schema_version"] == dbt_schema_version
    key = "env_key"
    if os.name == "nt":
        key = key.upper()
    assert metadata["env"] == {key: "env_value"}


def run_and_generate(project, args=None):
    results = run_dbt(["run"])
    assert len(results) == 2
    rm_file(project.project_root, "target", "manifest.json")
    rm_file(project.project_root, "target", "run_results.json")

    start_time = datetime.utcnow()
    run_args = ["docs", "generate"]
    if args:
        run_args.extend(args)
    catalog = run_dbt(run_args)
    assert catalog
    return start_time


class BaseGenerateProject:
    @pytest.fixture(scope="class", autouse=True)
    def setup(self, project):
        alternate_schema_name = project.test_schema + "_test"
        project.create_test_schema(schema_name=alternate_schema_name)
        os.environ["DBT_ENV_CUSTOM_ENV_env_key"] = "env_value"
        assets = {"lorem-ipsum.txt": "Lorem ipsum dolor sit amet"}
        write_project_files(project.project_root, "assets", assets)
        run_dbt(["seed"])
        yield
        del os.environ["DBT_ENV_CUSTOM_ENV_env_key"]

    @pytest.fixture(scope="class")
    def seeds(self):
        return {"schema.yml": seed__schema_yml, "seed.csv": seed__seed_csv}

    @pytest.fixture(scope="class")
    def macros(self):
        return {
            "schema.yml": macros__schema_yml,
            "macro.md": macros__macro_md,
            "dummy_test.sql": macros__dummy_test_sql,
        }

    @pytest.fixture(scope="class")
    def snapshots(self):
        return {"snapshot_seed.sql": snapshot__snapshot_seed_sql}

    @pytest.fixture(scope="class")
    def project_config_update(self, unique_schema):
        alternate_schema = unique_schema + "_test"
        return {
            "asset-paths": ["assets", "invalid-asset-paths"],
            "vars": {
                "test_schema": unique_schema,
                "alternate_schema": alternate_schema,
            },
            "seeds": {
                "quote_columns": True,
            },
        }


class BaseDocsGenerate(BaseGenerateProject):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": models__schema_yml,
            "second_model.sql": models__second_model_sql,
            "readme.md": models__readme_md,
            "model.sql": models__model_sql,
        }

    @pytest.fixture(scope="class")
    def expected_catalog(self, project, profile_user):
        return base_expected_catalog(
            project,
            role=profile_user,
            id_type="integer",
            text_type="text",
            time_type="timestamp without time zone",
            view_type="VIEW",
            table_type="BASE TABLE",
            model_stats=no_stats(),
        )

    @pytest.fixture(autouse=True)
    def clean_up(self, project):
        yield
        with project.adapter.connection_named("__test"):
            alternate_schema = f"{project.test_schema}_test"
            relation = project.adapter.Relation.create(
                database=project.database, schema=alternate_schema
            )
            project.adapter.drop_schema(relation)

    pass

    # Test "--no-compile" flag works and produces no manifest.json
    def test_run_and_generate_no_compile(self, project, expected_catalog):
        start_time = run_and_generate(project, ["--no-compile"])
        assert not os.path.exists(os.path.join(project.project_root, "target", "manifest.json"))
        verify_catalog(project, expected_catalog, start_time)

    # Test generic "docs generate" command
    def test_run_and_generate(self, project, expected_catalog):
        start_time = run_and_generate(project)
        verify_catalog(project, expected_catalog, start_time)

        # Check that assets have been copied to the target directory for use in the docs html page
        assert os.path.exists(os.path.join(project.project_root, "target", "assets"))
        assert os.path.exists(
            os.path.join(project.project_root, "target", "assets", "lorem-ipsum.txt")
        )
        assert not os.path.exists(
            os.path.join(project.project_root, "target", "non-existent-assets")
        )

    # Test generic "docs generate" command
    def test_locally_run_and_generate(self, project, expected_catalog):
        os.chdir(
            project.profiles_dir
        )  # Change to random directory to test that assets doc generation works with project-dir
        start_time = run_and_generate(project)
        verify_catalog(project, expected_catalog, start_time)

        # Check that assets have been copied to the target directory for use in the docs html page
        assert os.path.exists(os.path.join(project.project_root, "target", "assets"))
        assert os.path.exists(
            os.path.join(project.project_root, "target", "assets", "lorem-ipsum.txt")
        )
        assert not os.path.exists(
            os.path.join(project.project_root, "target", "non-existent-assets")
        )


class TestDocsGenerate(BaseDocsGenerate):
    pass


class BaseDocsGenReferences(BaseGenerateProject):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": ref_models__schema_yml,
            "sources.yml": ref_sources__schema_yml,
            "view_summary.sql": ref_models__view_summary_sql,
            "ephemeral_summary.sql": ref_models__ephemeral_summary_sql,
            "ephemeral_copy.sql": ref_models__ephemeral_copy_sql,
            "docs.md": ref_models__docs_md,
        }

    @pytest.fixture(scope="class")
    def expected_catalog(self, project, profile_user):
        return expected_references_catalog(
            project,
            role=profile_user,
            id_type="integer",
            text_type="text",
            time_type="timestamp without time zone",
            bigint_type="bigint",
            view_type="VIEW",
            table_type="BASE TABLE",
            model_stats=no_stats(),
        )

    def test_references(self, project, expected_catalog):
        start_time = run_and_generate(project)
        verify_catalog(project, expected_catalog, start_time)


class TestDocsGenReferences(BaseDocsGenReferences):
    pass
