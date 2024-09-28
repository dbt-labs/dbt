import pytest

from dbt.exceptions import DbtReferenceError, InvalidAccessTypeError
from dbt.node_types import AccessType
from dbt.tests.fixtures.project import write_project_files
from dbt.tests.util import get_manifest, rm_file, run_dbt, write_file
from tests.fixtures.dbt_integration_project import dbt_integration_project  # noqa: F401

my_model_sql = "select 1 as fun"

another_model_sql = "select 1234 as notfun"

yet_another_model_sql = "select 999 as weird"

stg_model_sql = """
-- depends_on: {{ source('my_source', 'my_table') }}

select 999 as weird
"""

stg_model_yml = """
models:
  - name: stg_model
    description: "my model"
    group: analytics
"""

invalid_access_source_yml = """
sources:
  - name: my_source
    access: invalid
    tables:
      - name: my_table
"""

non_grouped_source_yml = """
sources:
  - name: my_source
    tables:
      - name: my_table
"""

protected_analytics_source_yml = """
sources:
  - name: my_source
    group: analytics
    tables:
      - name: my_table
"""

private_marts_source_yml = """
sources:
  - name: my_source
    group: marts
    access: private
    tables:
      - name: my_table
"""

schema_yml = """
models:
  - name: my_model
    description: "my model"
    access: public
  - name: another_model
    description: "yet another model"
"""

ephemeral_schema_yml = """
models:
  - name: my_model
    description: "my model"
    access: public
    config:
      materialized: ephemeral
  - name: another_model
    description: "yet another model"
"""

v2_schema_yml = """
models:
  - name: my_model
    description: "my model"
    access: public
  - name: another_model
    description: "another model"
  - name: yet_another_model
    description: "yet another model"
    access: unsupported
"""

ref_my_model_sql = """
   select fun from {{ ref('my_model') }}
"""

groups_yml = """
groups:
  - name: analytics
    owner:
      name: analytics_owner
  - name: marts
    owner:
      name: marts_owner
"""


v3_schema_yml = """
models:
  - name: my_model
    description: "my model"
    access: private
    group: analytics
  - name: another_model
    description: "yet another model"
  - name: ref_my_model
    description: "a model that refs my_model"
    group: analytics
"""

v4_schema_yml = """
models:
  - name: my_model
    description: "my model"
    access: private
    group: analytics
  - name: another_model
    description: "yet another model"
  - name: ref_my_model
    description: "a model that refs my_model"
    group: marts
"""

simple_exposure_yml = """
exposures:
  - name: simple_exposure
    label: simple exposure label
    type: dashboard
    depends_on:
      - ref('my_model')
    owner:
      email: something@example.com
"""

v5_schema_yml = """
models:
  - name: my_model
    description: "my model"
    access: private
    group: analytics
  - name: another_model
    description: "yet another model"
  - name: ref_my_model
    description: "a model that refs my_model"
    group: analytics
  - name: people_model
    description: "some people"
    access: public
    group: analytics
"""

v6_schema_yml = """
models:
  - name: my_model
    description: "my model"
    config:
      access: private
      group: analytics
  - name: another_model
    description: "yet another model"
  - name: ref_my_model
    description: "a model that refs my_model"
    config:
      group: analytics
  - name: people_model
    description: "some people"
    config:
      access: public
      group: analytics
"""

people_model_sql = """
select 1 as id, 'Drew' as first_name, 'Banin' as last_name, 'yellow' as favorite_color, true as loves_dbt, 5 as tenure, current_timestamp as created_at
union all
select 1 as id, 'Jeremy' as first_name, 'Cohen' as last_name, 'indigo' as favorite_color, true as loves_dbt, 4 as tenure, current_timestamp as created_at
union all
select 1 as id, 'Callum' as first_name, 'McCann' as last_name, 'emerald' as favorite_color, true as loves_dbt, 0 as tenure, current_timestamp as created_at
"""

people_semantic_model_yml = """
semantic_models:
  - name: semantic_people
    model: ref('people_model')
    dimensions:
      - name: favorite_color
        type: categorical
      - name: created_at
        type: TIME
        type_params:
          time_granularity: day
    measures:
      - name: years_tenure
        agg: SUM
        expr: tenure
      - name: people
        agg: count
        expr: id
    entities:
      - name: id
        type: primary
    defaults:
      agg_time_dimension: created_at
"""

people_metric_yml = """
metrics:

  - name: number_of_people
    label: "Number of people"
    description: Total count of people
    type: simple
    type_params:
      measure: "people"
    meta:
        my_meta: 'testing'
    config:
      group: analytics
"""

v2_people_metric_yml = """
metrics:

  - name: number_of_people
    label: "Number of people"
    description: Total count of people
    type: simple
    type_params:
      measure: "people"
    meta:
        my_meta: 'testing'
    config:
      group: marts
"""


dbt_integration_project__dbt_project_yml_restrited_access = """
name: dbt_integration_project
version: '1.0'
config-version: 2

model-paths: ["models"]    # paths to models
analysis-paths: ["analyses"] # path with analysis files which are compiled, but not run
target-path: "target"      # path for compiled code
clean-targets: ["target"]  # directories removed by the clean task
test-paths: ["tests"]       # where to store test results
seed-paths: ["seeds"]       # load CSVs from this directory with `dbt seed`
macro-paths: ["macros"]    # where to find macros

profile: user

models:
    dbt_integration_project:

restrict-access: True
"""


dbt_integration_project__schema_yml_protected_model = """
version: 2
models:
- name: table_model
  access: protected
"""

dbt_integration_project__schema_yml_private_model = """
version: 2
models:
- name: table_model
  access: private
  group: package
"""

ref_package_model_sql = """
   select * from {{ ref('dbt_integration_project', 'table_model') }}
"""

schema_yml_ref_package_model = """
version: 2
models:
- name: ref_package_model
  group: package
"""

metricflow_time_spine_sql = """
SELECT to_date('02/20/2023', 'mm/dd/yyyy') as date_day
"""


class TestAccess:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "my_model.sql": my_model_sql,
            "another_model.sql": yet_another_model_sql,
            "schema.yml": schema_yml,
        }

    def test_access_attribute(self, project):
        manifest = run_dbt(["parse"])
        assert len(manifest.nodes) == 2

        my_model_id = "model.test.my_model"
        another_model_id = "model.test.another_model"
        assert my_model_id in manifest.nodes
        assert another_model_id in manifest.nodes

        assert manifest.nodes[my_model_id].access == AccessType.Public
        assert manifest.nodes[another_model_id].access == AccessType.Protected

        # write a file with invalid materialization for public access value
        write_file(ephemeral_schema_yml, project.project_root, "models", "schema.yml")
        with pytest.raises(InvalidAccessTypeError):
            run_dbt(["parse"])

        # write a file with an invalid access value
        write_file(yet_another_model_sql, project.project_root, "models", "yet_another_model.sql")
        write_file(v2_schema_yml, project.project_root, "models", "schema.yml")

        with pytest.raises(InvalidAccessTypeError):
            run_dbt(["parse"])

        write_file(v2_schema_yml, project.project_root, "models", "schema.yml")
        with pytest.raises(InvalidAccessTypeError):
            run_dbt(["parse"])

        # Remove invalid access files and write out model that refs my_model
        rm_file(project.project_root, "models", "yet_another_model.sql")
        write_file(schema_yml, project.project_root, "models", "schema.yml")
        write_file(ref_my_model_sql, project.project_root, "models", "ref_my_model.sql")
        manifest = run_dbt(["parse"])
        assert len(manifest.nodes) == 3

        # make my_model private, set same group on my_model and ref_my_model
        write_file(groups_yml, project.project_root, "models", "groups.yml")
        write_file(v3_schema_yml, project.project_root, "models", "schema.yml")
        manifest = run_dbt(["parse"])
        assert len(manifest.nodes) == 3
        manifest = get_manifest(project.project_root)
        ref_my_model_id = "model.test.ref_my_model"
        assert manifest.nodes[my_model_id].group == "analytics"
        assert manifest.nodes[ref_my_model_id].group == "analytics"

        # Change group on ref_my_model and it should raise
        write_file(v4_schema_yml, project.project_root, "models", "schema.yml")
        with pytest.raises(DbtReferenceError):
            run_dbt(["parse"])

        # put back group on ref_my_model, add exposure with ref to private model
        write_file(v3_schema_yml, project.project_root, "models", "schema.yml")
        # verify it works again
        manifest = run_dbt(["parse"])
        assert len(manifest.nodes) == 3

        # add source with invalid access
        write_file(invalid_access_source_yml, project.project_root, "models", "my_source.yml")
        with pytest.raises(InvalidAccessTypeError):
            run_dbt(["parse"])

        # add source without group, new model in group analytics that depends on it
        write_file(non_grouped_source_yml, project.project_root, "models", "my_source.yml")
        write_file(stg_model_yml, project.project_root, "models", "stg_model.yml")
        write_file(stg_model_sql, project.project_root, "models", "stg_model.sql")
        manifest = run_dbt(["parse"])
        assert len(manifest.nodes) == 4
        assert len(manifest.sources) == 1

        # add source to group analytics, implicitly set it to protected
        write_file(protected_analytics_source_yml, project.project_root, "models", "my_source.yml")
        manifest = run_dbt(["parse"])
        assert len(manifest.nodes) == 4
        assert len(manifest.sources) == 1

        # add source to group mart, explicitly set it to private
        write_file(private_marts_source_yml, project.project_root, "models", "my_source.yml")
        with pytest.raises(DbtReferenceError):
            run_dbt(["parse"])

        # revert source to no group, protected access
        write_file(non_grouped_source_yml, project.project_root, "models", "my_source.yml")

        # Write out exposure refing private my_model
        write_file(simple_exposure_yml, project.project_root, "models", "simple_exposure.yml")
        # Fails with reference error
        with pytest.raises(DbtReferenceError):
            run_dbt(["parse"])

        # Remove exposure and add people model and metric file
        write_file(v5_schema_yml, project.project_root, "models", "schema.yml")
        rm_file(project.project_root, "models", "simple_exposure.yml")
        write_file(people_model_sql, "models", "people_model.sql")
        write_file(people_semantic_model_yml, "models", "people_semantic_model.yml")
        write_file(people_metric_yml, "models", "people_metric.yml")
        write_file(metricflow_time_spine_sql, "models", "metricflow_time_spine.sql")
        # Should succeed
        manifest = run_dbt(["parse"])
        assert len(manifest.nodes) == 6
        metric_id = "metric.test.number_of_people"
        assert manifest.metrics[metric_id].group == "analytics"

        # Use access and group in config
        write_file(v5_schema_yml, project.project_root, "models", "schema.yml")
        manifest = run_dbt(["parse"])
        assert len(manifest.nodes) == 6
        assert manifest.nodes["model.test.my_model"].access == AccessType.Private
        assert manifest.nodes["model.test.my_model"].group == "analytics"
        assert manifest.nodes["model.test.ref_my_model"].access == AccessType.Protected
        assert manifest.nodes["model.test.ref_my_model"].group == "analytics"
        assert manifest.nodes["model.test.people_model"].access == AccessType.Public
        assert manifest.nodes["model.test.people_model"].group == "analytics"
        assert manifest.nodes["model.test.another_model"].access == AccessType.Protected
        assert manifest.nodes["model.test.another_model"].group is None


class TestUnrestrictedPackageAccess:
    @pytest.fixture(scope="class", autouse=True)
    def setUp(self, project_root, dbt_integration_project):  # noqa: F811
        write_project_files(project_root, "dbt_integration_project", dbt_integration_project)

    @pytest.fixture(scope="class")
    def packages(self):
        return {"packages": [{"local": "dbt_integration_project"}]}

    @pytest.fixture(scope="class")
    def models(self):
        return {"ref_protected_package_model.sql": ref_package_model_sql}

    def test_unrestricted_protected_ref(self, project):
        write_file(
            dbt_integration_project__schema_yml_protected_model,
            project.project_root,
            "dbt_integration_project",
            "models",
            "schema.yml",
        )
        run_dbt(["deps"])

        # Runs without issue because restrict-access defaults to False
        manifest = run_dbt(["parse"])
        assert len(manifest.nodes) == 4
        root_project_model = manifest.nodes["model.test.ref_protected_package_model"]
        assert root_project_model.depends_on_nodes == ["model.dbt_integration_project.table_model"]


class TestRestrictedPackageAccess:
    @pytest.fixture(scope="class", autouse=True)
    def setUp(self, project_root, dbt_integration_project):  # noqa: F811
        write_project_files(project_root, "dbt_integration_project", dbt_integration_project)
        # Set table_model.access to protected
        write_file(
            dbt_integration_project__schema_yml_protected_model,
            project_root,
            "dbt_integration_project",
            "models",
            "schema.yml",
        )
        # Set dbt_integration_project.restrict-access to True
        write_file(
            dbt_integration_project__dbt_project_yml_restrited_access,
            project_root,
            "dbt_integration_project",
            "dbt_project.yml",
        )

    @pytest.fixture(scope="class")
    def packages(self):
        return {"packages": [{"local": "dbt_integration_project"}]}

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "ref_package_model.sql": ref_package_model_sql,
            "schema.yml": schema_yml_ref_package_model,
        }

    def test_restricted_protected_ref(self, project):
        run_dbt(["deps"])
        with pytest.raises(DbtReferenceError):
            run_dbt(["parse"])

    def test_restricted_private_ref(self, project):
        run_dbt(["deps"])

        # Set table_model.access to private
        write_file(
            dbt_integration_project__schema_yml_private_model,
            project.project_root,
            "dbt_integration_project",
            "models",
            "schema.yml",
        )

        with pytest.raises(DbtReferenceError):
            run_dbt(["parse"])


dbt_project_yml = """
models:
  test:
    subdir_one:
      +group: analytics
      +access: private
    subdir_two:
      +group: marts
      +access: public
"""


class TestAccessDbtProjectConfig:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "model_one.sql": my_model_sql,
            "subdir_one": {
                "model_two.sql": my_model_sql,
            },
            "subdir_two": {
                "model_three.sql": my_model_sql,
            },
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return dbt_project_yml

    def test_dbt_project_access_config(self, project):
        write_file(groups_yml, project.project_root, "models", "groups.yml")
        manifest = run_dbt(["parse"])
        model_one = manifest.nodes["model.test.model_one"]
        model_two = manifest.nodes["model.test.model_two"]
        model_three = manifest.nodes["model.test.model_three"]
        assert model_one.group is None
        assert model_one.access == AccessType.Protected
        assert model_two.group == "analytics"
        assert model_two.access == AccessType.Private
        assert model_three.group == "marts"
        assert model_three.access == AccessType.Public
