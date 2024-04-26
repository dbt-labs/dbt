import pytest
import os
from datetime import timezone, datetime
import dbt
import jsonschema

from dbt.tests.util import run_dbt, get_artifact, check_datetime_between, run_dbt_and_capture
from tests.functional.artifacts.expected_manifest import (
    expected_seeded_manifest,
    expected_references_manifest,
    expected_versions_manifest,
)
from tests.functional.artifacts.expected_run_results import (
    expected_run_results,
    expected_references_run_results,
    expected_versions_run_results,
)

from dbt.contracts.graph.manifest import WritableManifest
from dbt.artifacts.schemas.results import RunStatus
from dbt.artifacts.schemas.run import RunResultsArtifact

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
        quoting:
          identifier: True
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

models__model_with_pre_hook_sql = """
{{
    config(
        pre_hook={
            "sql": "{{ alter_timezone(timezone='Etc/UTC') }}"
        }
    )
}}
select current_setting('timezone') as timezone
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

macros__alter_timezone_sql = """
{% macro alter_timezone(timezone='America/Los_Angeles') %}
{% set sql %}
    SET TimeZone='{{ timezone }}';
{% endset %}

{% do run_query(sql) %}
{% do log("Timezone set to: " + timezone, info=True) %}
{% endmacro %}
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

groups:
  - name: test_group
    owner:
      email: test_group@test.com

models:
  - name: ephemeral_summary
    description: "{{ doc('ephemeral_summary') }}"
    config:
      group: test_group
    columns: &summary_columns
      - name: first_name
        description: "{{ doc('summary_first_name') }}"
      - name: ct
        description: "{{ doc('summary_count') }}"
  - name: view_summary
    description: "{{ doc('view_summary') }}"
    columns: *summary_columns

sources:
  - name: my_source
    description: "{{ doc('source_info') }}"
    loader: a_loader
    schema: "{{ var('test_schema') }}"
    quoting:
      database: False
      identifier: False
    tables:
      - name: my_table
        description: "{{ doc('table_info') }}"
        identifier: seed
        quoting:
          identifier: True
        columns:
          - name: id
            description: "{{ doc('column_info') }}"

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

versioned_models__schema_yml = """
version: 2

groups:
  - name: test_group
    owner:
      email: test_group@test.com

models:
  - name: versioned_model
    description: "A versioned model"
    latest_version: 2
    config:
      group: test_group
      materialized: table
      meta:
        color: blue
        size: large
    data_tests:
      - unique:
          column_name: count
    columns:
      - name: first_name
        description: "The first name being summarized"
        data_tests:
          - unique
      - name: ct
        description: "The number of instances of the first name"
    versions:
      - v: 1
        defined_in: arbitrary_file_name
        deprecation_date: 2022-07-11
      - v: 2
        config:
          materialized: view
          meta:
            color: red
        data_tests: []
        columns:
          - include: '*'
            exclude: ['ct']
          - name: extra
  - name: ref_versioned_model

exposures:
  - name: notebook_exposure
    type: notebook
    depends_on:
      - ref('versioned_model', v=2)
    owner:
      email: something@example.com
      name: Some name
    description: "notebook_info"
"""

versioned_models__v1_sql = """
select "test first name" as first_name, 1 as ct
"""

versioned_models__v2_sql = """
select "test first name" as first_name, 1 as extra
"""

versioned_models___ref_sql = """
select first_name from {{ ref("versioned_model", version=2) }}
UNION ALL
select first_name from {{ ref("versioned_model", version="2") }}
UNION ALL
select first_name from {{ ref("versioned_model", v=2) }}
UNION ALL
select first_name from {{ ref("versioned_model") }}
UNION ALL
select first_name from {{ ref("versioned_model", version=1) }}
"""


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


def verify_manifest(project, expected_manifest, start_time, manifest_schema_path):
    manifest_path = os.path.join(project.project_root, "target", "manifest.json")
    assert os.path.exists(manifest_path)
    manifest = get_artifact(manifest_path)
    # Verify that manifest jsonschema from WritableManifest works
    manifest_schema = WritableManifest.json_schema()
    validate(manifest_schema, manifest)

    # Verify that stored manifest jsonschema works.
    # If this fails, schemas need to be updated with:
    #   scripts/collect-artifact-schema.py --path schemas --artifact manifest
    stored_manifest_schema = get_artifact(manifest_schema_path)
    validate(stored_manifest_schema, manifest)

    manifest_keys = {
        "nodes",
        "sources",
        "macros",
        "parent_map",
        "child_map",
        "group_map",
        "metrics",
        "groups",
        "docs",
        "metadata",
        "docs",
        "disabled",
        "exposures",
        "selectors",
        "semantic_models",
        "unit_tests",
        "saved_queries",
    }

    assert set(manifest.keys()) == manifest_keys

    for key in manifest_keys:
        if key == "macros":
            verify_manifest_macros(manifest, expected_manifest.get("macros"))
        elif key == "metadata":
            metadata = manifest["metadata"]
            dbt_schema_version = str(WritableManifest.dbt_schema_version)
            verify_metadata(metadata, dbt_schema_version, start_time)
            assert (
                "project_id" in metadata
                and metadata["project_id"] == "098f6bcd4621d373cade4e832627b4f6"
            )
            assert "project_name" in metadata and metadata["project_name"] == "test"
            assert (
                "send_anonymous_usage_stats" in metadata
                and metadata["send_anonymous_usage_stats"] is False
            )
            assert "adapter_type" in metadata and metadata["adapter_type"] == project.adapter_type
        elif key in ["nodes", "sources", "exposures", "metrics", "disabled", "docs"]:
            for unique_id, node in expected_manifest[key].items():
                assert unique_id in manifest[key]
                assert manifest[key][unique_id] == node, f"{unique_id} did not match"
        else:  # ['docs', 'parent_map', 'child_map', 'group_map', 'selectors', 'semantic_models', 'saved_queries']
            assert manifest[key] == expected_manifest[key]


def verify_manifest_macros(manifest, expected=None):
    assert "macros" in manifest
    if expected:
        for unique_id, expected_macro in expected.items():
            assert unique_id in manifest["macros"]
            actual_macro = manifest["macros"][unique_id]
            assert expected_macro == actual_macro


def verify_run_results(project, expected_run_results, start_time, run_results_schema_path):
    run_results_path = os.path.join(project.project_root, "target", "run_results.json")
    run_results = get_artifact(run_results_path)
    assert "metadata" in run_results

    # Verify that jsonschema for RunResultsArtifact works
    run_results_schema = RunResultsArtifact.json_schema()
    validate(run_results_schema, run_results)

    # Verify that stored run_results jsonschema works.
    # If this fails, schemas need to be updated with:
    #   scripts/collect-artifact-schema.py --path schemas --artifact run-results
    stored_run_results_schema = get_artifact(run_results_schema_path)
    validate(stored_run_results_schema, run_results)

    dbt_schema_version = str(RunResultsArtifact.dbt_schema_version)
    verify_metadata(run_results["metadata"], dbt_schema_version, start_time)
    assert "elapsed_time" in run_results
    assert run_results["elapsed_time"] > 0
    assert isinstance(run_results["elapsed_time"], float)
    assert "args" in run_results
    # sort the results so we can make reasonable assertions
    run_results["results"].sort(key=lambda r: r["unique_id"])
    assert run_results["results"] == expected_run_results
    assert set(run_results) == {"elapsed_time", "results", "metadata", "args"}


class BaseVerifyProject:
    @pytest.fixture(scope="class", autouse=True)
    def setup(self, project):
        alternate_schema_name = project.test_schema + "_test"
        project.create_test_schema(schema_name=alternate_schema_name)
        os.environ["DBT_ENV_CUSTOM_ENV_env_key"] = "env_value"
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
            "vars": {
                "test_schema": unique_schema,
                "alternate_schema": alternate_schema,
            },
            "seeds": {
                "quote_columns": True,
            },
            "quoting": {"identifier": False},
        }

    @pytest.fixture(scope="class")
    def manifest_schema_path(self, request):
        schema_version_paths = WritableManifest.dbt_schema_version.path.split("/")
        manifest_schema_path = os.path.join(
            request.config.rootdir, "schemas", *schema_version_paths
        )
        return manifest_schema_path

    @pytest.fixture(scope="class")
    def run_results_schema_path(self, request):
        schema_version_paths = RunResultsArtifact.dbt_schema_version.path.split("/")
        run_results_schema_path = os.path.join(
            request.config.rootdir, "schemas", *schema_version_paths
        )
        return run_results_schema_path


def validate(artifact_schema, artifact_dict):
    validator = jsonschema.Draft7Validator(artifact_schema)
    error = next(iter(validator.iter_errors(artifact_dict)), None)
    assert error is None


class TestVerifyArtifacts(BaseVerifyProject):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": models__schema_yml,
            "second_model.sql": models__second_model_sql,
            "readme.md": models__readme_md,
            "model.sql": models__model_sql,
        }

    # Test generic "docs generate" command
    def test_run_and_generate(self, project, manifest_schema_path, run_results_schema_path):
        start_time = datetime.now(timezone.utc).replace(tzinfo=None)
        results = run_dbt(["compile"])
        assert len(results) == 7
        verify_manifest(
            project,
            expected_seeded_manifest(project, quote_model=False),
            start_time,
            manifest_schema_path,
        )
        verify_run_results(project, expected_run_results(), start_time, run_results_schema_path)


class TestVerifyArtifactsReferences(BaseVerifyProject):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": ref_models__schema_yml,
            "view_summary.sql": ref_models__view_summary_sql,
            "ephemeral_summary.sql": ref_models__ephemeral_summary_sql,
            "ephemeral_copy.sql": ref_models__ephemeral_copy_sql,
            "docs.md": ref_models__docs_md,
        }

    def test_references(self, project, manifest_schema_path, run_results_schema_path):
        start_time = datetime.now(timezone.utc).replace(tzinfo=None)
        results = run_dbt(["compile"])
        assert len(results) == 4
        verify_manifest(
            project, expected_references_manifest(project), start_time, manifest_schema_path
        )
        verify_run_results(
            project, expected_references_run_results(), start_time, run_results_schema_path
        )


class TestVerifyArtifactsVersions(BaseVerifyProject):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": versioned_models__schema_yml,
            "versioned_model_v2.sql": versioned_models__v2_sql,
            "arbitrary_file_name.sql": versioned_models__v1_sql,
            "ref_versioned_model.sql": versioned_models___ref_sql,
        }

    @pytest.fixture(scope="class")
    def seeds(self):
        return {}

    @pytest.fixture(scope="class")
    def snapshots(self):
        return {}

    def test_versions(self, project, manifest_schema_path, run_results_schema_path):
        start_time = datetime.now(timezone.utc).replace(tzinfo=None)
        results = run_dbt(["compile"])
        assert len(results) == 6
        verify_manifest(
            project, expected_versions_manifest(project), start_time, manifest_schema_path
        )
        verify_run_results(
            project, expected_versions_run_results(), start_time, run_results_schema_path
        )


class TestVerifyRunOperation(BaseVerifyProject):
    @pytest.fixture(scope="class")
    def macros(self):
        return {"alter_timezone.sql": macros__alter_timezone_sql}

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "model_with_pre_hook.sql": models__model_with_pre_hook_sql,
        }

    def test_run_operation(self, project):
        results, log_output = run_dbt_and_capture(["run-operation", "alter_timezone"])
        assert len(results) == 1
        assert results[0].status == RunStatus.Success
        assert results[0].unique_id == "macro.test.alter_timezone"
        assert "Timezone set to: America/Los_Angeles" in log_output

    def test_run_model_with_operation(self, project):
        # pre-hooks are not included in run_results since they are an attribute of the node and not a node in their
        # own right
        results, log_output = run_dbt_and_capture(["run", "--select", "model_with_pre_hook"])
        assert len(results) == 1
        assert results[0].status == RunStatus.Success
        assert "Timezone set to: Etc/UTC" in log_output
