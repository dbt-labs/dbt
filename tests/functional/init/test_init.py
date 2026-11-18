import os
from pathlib import Path
from unittest import mock
from unittest.mock import Mock, call

import click
import pytest
import yaml

from dbt.exceptions import DbtRuntimeError
from dbt.task.init import InitTask
from dbt.tests.util import run_dbt


class TestInitProjectWithExistingProfilesYml:
    @mock.patch("dbt.task.init.InitTask._run_debug")
    @mock.patch("dbt.task.init._get_adapter_plugin_names")
    @mock.patch("click.confirm")
    @mock.patch("click.prompt")
    def test_init_task_in_project_with_existing_profiles_yml(
        self, mock_prompt, mock_confirm, mock_get_adapter, mock_run_debug, project
    ):
        manager = Mock()
        manager.attach_mock(mock_prompt, "prompt")
        manager.attach_mock(mock_confirm, "confirm")
        manager.confirm.side_effect = ["y"]
        manager.prompt.side_effect = [
            1,
            "localhost",
            5432,
            "test_user",
            "test_password",
            "test_db",
            "test_schema",
            4,
        ]
        mock_get_adapter.return_value = [project.adapter.type()]

        run_dbt(["init"])

        manager.assert_has_calls(
            [
                call.confirm(
                    f"The profile test already exists in {os.path.join(project.profiles_dir, 'profiles.yml')}. Continue and overwrite it?"
                ),
                call.prompt(
                    "Which database would you like to use?\n[1] postgres\n\n(Don't see the one you want? https://docs.getdbt.com/docs/available-adapters)\n\nEnter a number",
                    type=click.INT,
                ),
                call.prompt(
                    "host (hostname for the instance)", default=None, hide_input=False, type=None
                ),
                call.prompt("port", default=5432, hide_input=False, type=click.INT),
                call.prompt("user (dev username)", default=None, hide_input=False, type=None),
                call.prompt("pass (dev password)", default=None, hide_input=True, type=None),
                call.prompt(
                    "dbname (default database that dbt will build objects in)",
                    default=None,
                    hide_input=False,
                    type=None,
                ),
                call.prompt(
                    "schema (default schema that dbt will build objects in)",
                    default=None,
                    hide_input=False,
                    type=None,
                ),
                call.prompt("threads (1 or more)", default=1, hide_input=False, type=click.INT),
            ]
        )

        with open(os.path.join(project.profiles_dir, "profiles.yml"), "r") as f:
            assert (
                f.read()
                == """test:
  outputs:
    dev:
      dbname: test_db
      host: localhost
      pass: test_password
      port: 5432
      schema: test_schema
      threads: 4
      type: postgres
      user: test_user
  target: dev
"""
            )

        mock_run_debug.assert_called_once()

    def test_init_task_in_project_specifying_profile_errors(self, project):
        with pytest.raises(DbtRuntimeError) as error:
            run_dbt(["init", "--profile", "test"], expect_pass=False)
            assert "Can not init existing project with specified profile" in str(error)


class TestInitProjectWithoutExistingProfilesYml:
    @mock.patch("dbt.task.init.InitTask._run_debug")
    @mock.patch("dbt.task.init._get_adapter_plugin_names")
    @mock.patch("click.prompt")
    @mock.patch.object(Path, "exists", autospec=True)
    def test_init_task_in_project_without_existing_profiles_yml(
        self, exists, mock_prompt, mock_get_adapter, mock_run_debug, project
    ):
        def exists_side_effect(path):
            # Override responses on specific files, default to 'real world' if not overriden
            return {"profiles.yml": False}.get(path.name, os.path.exists(path))

        exists.side_effect = exists_side_effect
        manager = Mock()
        manager.attach_mock(mock_prompt, "prompt")
        manager.prompt.side_effect = [
            1,
            "localhost",
            5432,
            "test_user",
            "test_password",
            "test_db",
            "test_schema",
            4,
        ]
        mock_get_adapter.return_value = [project.adapter.type()]

        run_dbt(["init"])

        manager.assert_has_calls(
            [
                call.prompt(
                    "Which database would you like to use?\n[1] postgres\n\n(Don't see the one you want? https://docs.getdbt.com/docs/available-adapters)\n\nEnter a number",
                    type=click.INT,
                ),
                call.prompt(
                    "host (hostname for the instance)", default=None, hide_input=False, type=None
                ),
                call.prompt("port", default=5432, hide_input=False, type=click.INT),
                call.prompt("user (dev username)", default=None, hide_input=False, type=None),
                call.prompt("pass (dev password)", default=None, hide_input=True, type=None),
                call.prompt(
                    "dbname (default database that dbt will build objects in)",
                    default=None,
                    hide_input=False,
                    type=None,
                ),
                call.prompt(
                    "schema (default schema that dbt will build objects in)",
                    default=None,
                    hide_input=False,
                    type=None,
                ),
                call.prompt("threads (1 or more)", default=1, hide_input=False, type=click.INT),
            ]
        )

        with open(os.path.join(project.profiles_dir, "profiles.yml"), "r") as f:
            assert (
                f.read()
                == """test:
  outputs:
    dev:
      dbname: test_db
      host: localhost
      pass: test_password
      port: 5432
      schema: test_schema
      threads: 4
      type: postgres
      user: test_user
  target: dev
"""
            )

            mock_run_debug.assert_called_once()

    def test_init_task_in_project_without_profile_yml_specifying_profile_errors(self, project):
        # Even without profiles.yml, init inside a project with --profile should error
        with pytest.raises(DbtRuntimeError) as error:
            run_dbt(["init", "--profile", "test"], expect_pass=False)
            assert "Can not init existing project with specified profile" in str(error)


class TestInitProjectWithoutExistingProfilesYmlOrTemplate:
    @mock.patch("dbt.task.init.InitTask._run_debug")
    @mock.patch("dbt.task.init._get_adapter_plugin_names")
    @mock.patch("click.confirm")
    @mock.patch("click.prompt")
    @mock.patch.object(Path, "exists", autospec=True)
    def test_init_task_in_project_without_existing_profiles_yml_or_profile_template(
        self, exists, mock_prompt, mock_confirm, mock_get_adapter, mock_run_debug, project
    ):
        def exists_side_effect(path):
            # Override responses on specific files, default to 'real world' if not overriden
            return {
                "profiles.yml": False,
                "profile_template.yml": False,
            }.get(path.name, os.path.exists(path))

        exists.side_effect = exists_side_effect
        manager = Mock()
        manager.attach_mock(mock_prompt, "prompt")
        manager.attach_mock(mock_confirm, "confirm")
        manager.prompt.side_effect = [
            1,
        ]
        mock_get_adapter.return_value = [project.adapter.type()]
        run_dbt(["init"])
        manager.assert_has_calls(
            [
                call.prompt(
                    "Which database would you like to use?\n[1] postgres\n\n(Don't see the one you want? https://docs.getdbt.com/docs/available-adapters)\n\nEnter a number",
                    type=click.INT,
                ),
            ]
        )

        with open(os.path.join(project.profiles_dir, "profiles.yml"), "r") as f:
            assert (
                f.read()
                == """test:
  outputs:

    dev:
      type: postgres
      threads: [1 or more]
      host: [host]
      port: [port]
      user: [dev_username]
      pass: [dev_password]
      dbname: [dbname]
      schema: [dev_schema]

    prod:
      type: postgres
      threads: [1 or more]
      host: [host]
      port: [port]
      user: [prod_username]
      pass: [prod_password]
      dbname: [dbname]
      schema: [prod_schema]

  target: dev
"""
            )
            mock_run_debug.assert_called_once()


class TestInitProjectWithProfileTemplateWithoutExistingProfilesYml:
    @mock.patch("dbt.task.init.InitTask._run_debug")
    @mock.patch("dbt.task.init._get_adapter_plugin_names")
    @mock.patch("click.confirm")
    @mock.patch("click.prompt")
    @mock.patch.object(Path, "exists", autospec=True)
    def test_init_task_in_project_with_profile_template_without_existing_profiles_yml(
        self, exists, mock_prompt, mock_confirm, mock_get_adapter, mock_run_debug, project
    ):
        def exists_side_effect(path):
            # Override responses on specific files, default to 'real world' if not overriden
            return {
                "profiles.yml": False,
            }.get(path.name, os.path.exists(path))

        exists.side_effect = exists_side_effect

        with open("profile_template.yml", "w") as f:
            f.write(
                """fixed:
  type: postgres
  threads: 4
  host: localhost
  dbname: my_db
  schema: my_schema
  target: my_target
prompts:
  target:
    hint: 'The target name'
    type: string
  port:
    hint: 'The port (for integer test purposes)'
    type: int
    default: 5432
  user:
    hint: 'Your username'
  pass:
    hint: 'Your password'
    hide_input: true"""
            )

        manager = Mock()
        manager.attach_mock(mock_prompt, "prompt")
        manager.attach_mock(mock_confirm, "confirm")
        manager.prompt.side_effect = ["my_target", 5432, "test_username", "test_password"]
        mock_get_adapter.return_value = [project.adapter.type()]
        run_dbt(["init"])
        manager.assert_has_calls(
            [
                call.prompt(
                    "target (The target name)", default=None, hide_input=False, type=click.STRING
                ),
                call.prompt(
                    "port (The port (for integer test purposes))",
                    default=5432,
                    hide_input=False,
                    type=click.INT,
                ),
                call.prompt("user (Your username)", default=None, hide_input=False, type=None),
                call.prompt("pass (Your password)", default=None, hide_input=True, type=None),
            ]
        )

        with open(os.path.join(project.profiles_dir, "profiles.yml"), "r") as f:
            assert (
                f.read()
                == """test:
  outputs:
    my_target:
      dbname: my_db
      host: localhost
      pass: test_password
      port: 5432
      schema: my_schema
      threads: 4
      type: postgres
      user: test_username
  target: my_target
"""
            )
            mock_run_debug.assert_called_once()


class TestInitInvalidProfileTemplate:
    @mock.patch("dbt.task.init.InitTask._run_debug")
    @mock.patch("dbt.task.init._get_adapter_plugin_names")
    @mock.patch("click.confirm")
    @mock.patch("click.prompt")
    def test_init_task_in_project_with_invalid_profile_template(
        self, mock_prompt, mock_confirm, mock_get_adapter, mock_run_debug, project
    ):
        """Test that when an invalid profile_template.yml is provided in the project,
        init command falls back to the target's profile_template.yml"""
        with open(os.path.join(project.project_root, "profile_template.yml"), "w") as f:
            f.write("""invalid template""")

        manager = Mock()
        manager.attach_mock(mock_prompt, "prompt")
        manager.attach_mock(mock_confirm, "confirm")
        manager.confirm.side_effect = ["y"]
        manager.prompt.side_effect = [
            1,
            "localhost",
            5432,
            "test_username",
            "test_password",
            "test_db",
            "test_schema",
            4,
        ]
        mock_get_adapter.return_value = [project.adapter.type()]

        run_dbt(["init"])

        manager.assert_has_calls(
            [
                call.confirm(
                    f"The profile test already exists in {os.path.join(project.profiles_dir, 'profiles.yml')}. Continue and overwrite it?"
                ),
                call.prompt(
                    "Which database would you like to use?\n[1] postgres\n\n(Don't see the one you want? https://docs.getdbt.com/docs/available-adapters)\n\nEnter a number",
                    type=click.INT,
                ),
                call.prompt(
                    "host (hostname for the instance)", default=None, hide_input=False, type=None
                ),
                call.prompt("port", default=5432, hide_input=False, type=click.INT),
                call.prompt("user (dev username)", default=None, hide_input=False, type=None),
                call.prompt("pass (dev password)", default=None, hide_input=True, type=None),
                call.prompt(
                    "dbname (default database that dbt will build objects in)",
                    default=None,
                    hide_input=False,
                    type=None,
                ),
                call.prompt(
                    "schema (default schema that dbt will build objects in)",
                    default=None,
                    hide_input=False,
                    type=None,
                ),
                call.prompt("threads (1 or more)", default=1, hide_input=False, type=click.INT),
            ]
        )

        with open(os.path.join(project.profiles_dir, "profiles.yml"), "r") as f:
            assert (
                f.read()
                == """test:
  outputs:
    dev:
      dbname: test_db
      host: localhost
      pass: test_password
      port: 5432
      schema: test_schema
      threads: 4
      type: postgres
      user: test_username
  target: dev
"""
            )
            mock_run_debug.assert_called_once()


class TestInitInsideOfProjectBase:
    @pytest.fixture(scope="class")
    def project_name(self, unique_schema):
        return f"my_project_{unique_schema}"


class TestInitOutsideOfProjectBase:
    @pytest.fixture(scope="class")
    def project_name(self, unique_schema):
        return f"my_project_{unique_schema}"

    @pytest.fixture(scope="class", autouse=True)
    def setup(self, project):
        # Start by removing the dbt_project.yml so that we're not in an existing project
        os.remove(os.path.join(project.project_root, "dbt_project.yml"))


class TestInitOutsideOfProject(TestInitOutsideOfProjectBase):
    @pytest.fixture(scope="class")
    def dbt_profile_data(self, unique_schema):
        return {
            "test": {
                "outputs": {
                    "default2": {
                        "type": "postgres",
                        "threads": 4,
                        "host": "localhost",
                        "port": int(os.getenv("POSTGRES_TEST_PORT", 5432)),
                        "user": os.getenv("POSTGRES_TEST_USER", "root"),
                        "pass": os.getenv("POSTGRES_TEST_PASS", "password"),
                        "dbname": os.getenv("POSTGRES_TEST_DATABASE", "dbt"),
                        "schema": unique_schema,
                    },
                    "noaccess": {
                        "type": "postgres",
                        "threads": 4,
                        "host": "localhost",
                        "port": int(os.getenv("POSTGRES_TEST_PORT", 5432)),
                        "user": "noaccess",
                        "pass": "password",
                        "dbname": os.getenv("POSTGRES_TEST_DATABASE", "dbt"),
                        "schema": unique_schema,
                    },
                },
                "target": "default2",
            },
        }

    @mock.patch("dbt.task.init.InitTask._run_debug")
    @mock.patch("dbt.task.init._get_adapter_plugin_names")
    @mock.patch("click.confirm")
    @mock.patch("click.prompt")
    def test_init_task_outside_of_project(
        self,
        mock_prompt,
        mock_confirm,
        mock_get_adapter,
        mock_run_debug,
        project,
        project_name,
        unique_schema,
        dbt_profile_data,
    ):
        manager = Mock()
        manager.attach_mock(mock_prompt, "prompt")
        manager.attach_mock(mock_confirm, "confirm")
        manager.prompt.side_effect = [
            project_name,
            1,
            "localhost",
            5432,
            "test_username",
            "test_password",
            "test_db",
            "test_schema",
            4,
        ]
        mock_get_adapter.return_value = [project.adapter.type()]
        run_dbt(["init"])
        default2 = dbt_profile_data["test"]["outputs"]["default2"]
        noaccess = dbt_profile_data["test"]["outputs"]["noaccess"]

        manager.assert_has_calls(
            [
                call.prompt("Enter a name for your project (letters, digits, underscore)"),
                call.prompt(
                    "Which database would you like to use?\n[1] postgres\n\n(Don't see the one you want? https://docs.getdbt.com/docs/available-adapters)\n\nEnter a number",
                    type=click.INT,
                ),
                call.prompt(
                    "host (hostname for the instance)", default=None, hide_input=False, type=None
                ),
                call.prompt("port", default=5432, hide_input=False, type=click.INT),
                call.prompt("user (dev username)", default=None, hide_input=False, type=None),
                call.prompt("pass (dev password)", default=None, hide_input=True, type=None),
                call.prompt(
                    "dbname (default database that dbt will build objects in)",
                    default=None,
                    hide_input=False,
                    type=None,
                ),
                call.prompt(
                    "schema (default schema that dbt will build objects in)",
                    default=None,
                    hide_input=False,
                    type=None,
                ),
                call.prompt("threads (1 or more)", default=1, hide_input=False, type=click.INT),
            ]
        )

        mock_run_debug.assert_called_once()

        with open(os.path.join(project.profiles_dir, "profiles.yml"), "r") as f:
            assert (
                f.read()
                == f"""{project_name}:
  outputs:
    dev:
      dbname: test_db
      host: localhost
      pass: test_password
      port: 5432
      schema: test_schema
      threads: 4
      type: postgres
      user: test_username
  target: dev
test:
  outputs:
    default2:
      dbname: {default2["dbname"]}
      host: localhost
      pass: {default2["pass"]}
      port: {default2["port"]}
      schema: {unique_schema}
      threads: 4
      type: postgres
      user: {default2["user"]}
    noaccess:
      dbname: {noaccess["dbname"]}
      host: localhost
      pass: password
      port: {noaccess["port"]}
      schema: {unique_schema}
      threads: 4
      type: postgres
      user: noaccess
  target: default2
"""
            )

        with open(os.path.join(project.project_root, project_name, "dbt_project.yml"), "r") as f:
            assert (
                f.read()
                == f"""
# Name your project! Project names should contain only lowercase characters
# and underscores. A good package name should reflect your organization's
# name or the intended use of these models
name: '{project_name}'
version: '1.0.0'

# This setting configures which "profile" dbt uses for this project.
profile: '{project_name}'

# These configurations specify where dbt should look for different types of files.
# The `model-paths` config, for example, states that models in this project can be
# found in the "models/" directory. You probably won't need to change these!
model-paths: ["models"]
analysis-paths: ["analyses"]
test-paths: ["tests"]
seed-paths: ["seeds"]
macro-paths: ["macros"]
snapshot-paths: ["snapshots"]

clean-targets:         # directories to be removed by `dbt clean`
  - "target"
  - "dbt_packages"


# Configuring models
# Full documentation: https://docs.getdbt.com/docs/configuring-models

# In this example config, we tell dbt to build all models in the example/
# directory as views. These settings can be overridden in the individual model
# files using the `{{{{ config(...) }}}}` macro.
models:
  {project_name}:
    # Config indicated by + and applies to all files under models/example/
    example:
      +materialized: view
"""
            )


class TestInitInvalidProjectNameCLI(TestInitOutsideOfProjectBase):
    @mock.patch("dbt.task.init._get_adapter_plugin_names")
    @mock.patch("click.confirm")
    @mock.patch("click.prompt")
    def test_init_invalid_project_name_cli(
        self, mock_prompt, mock_confirm, mock_get_adapter, project_name, project
    ):
        manager = Mock()
        manager.attach_mock(mock_prompt, "prompt")
        manager.attach_mock(mock_confirm, "confirm")

        invalid_name = "name-with-hyphen"
        valid_name = project_name
        manager.prompt.side_effect = [valid_name]
        mock_get_adapter.return_value = [project.adapter.type()]

        run_dbt(["init", invalid_name, "--skip-profile-setup"])
        manager.assert_has_calls(
            [
                call.prompt("Enter a name for your project (letters, digits, underscore)"),
            ]
        )


class TestInitInvalidProjectNamePrompt(TestInitOutsideOfProjectBase):
    @mock.patch("dbt.task.init._get_adapter_plugin_names")
    @mock.patch("click.confirm")
    @mock.patch("click.prompt")
    def test_init_invalid_project_name_prompt(
        self, mock_prompt, mock_confirm, mock_get_adapter, project_name, project
    ):
        manager = Mock()
        manager.attach_mock(mock_prompt, "prompt")
        manager.attach_mock(mock_confirm, "confirm")

        invalid_name = "name-with-hyphen"
        valid_name = project_name
        manager.prompt.side_effect = [invalid_name, valid_name]
        mock_get_adapter.return_value = [project.adapter.type()]

        run_dbt(["init", "--skip-profile-setup"])
        manager.assert_has_calls(
            [
                call.prompt("Enter a name for your project (letters, digits, underscore)"),
                call.prompt("Enter a name for your project (letters, digits, underscore)"),
            ]
        )


class TestInitProvidedProjectNameAndSkipProfileSetup(TestInitOutsideOfProjectBase):
    @mock.patch("dbt.task.init._get_adapter_plugin_names")
    @mock.patch("click.confirm")
    @mock.patch("click.prompt")
    def test_init_provided_project_name_and_skip_profile_setup(
        self, mock_prompt, mock_confirm, mock_get, project, project_name
    ):
        manager = mock.Mock()
        manager.attach_mock(mock_prompt, "prompt")
        manager.attach_mock(mock_confirm, "confirm")
        manager.prompt.side_effect = [
            1,
            "localhost",
            5432,
            "test_username",
            "test_password",
            "test_db",
            "test_schema",
            4,
        ]
        mock_get.return_value = [project.adapter.type()]

        # provide project name through the init command
        run_dbt(["init", project_name, "--skip-profile-setup"])
        assert len(manager.mock_calls) == 0

        with open(os.path.join(project.project_root, project_name, "dbt_project.yml"), "r") as f:
            assert (
                f.read()
                == f"""
# Name your project! Project names should contain only lowercase characters
# and underscores. A good package name should reflect your organization's
# name or the intended use of these models
name: '{project_name}'
version: '1.0.0'

# This setting configures which "profile" dbt uses for this project.
profile: '{project_name}'

# These configurations specify where dbt should look for different types of files.
# The `model-paths` config, for example, states that models in this project can be
# found in the "models/" directory. You probably won't need to change these!
model-paths: ["models"]
analysis-paths: ["analyses"]
test-paths: ["tests"]
seed-paths: ["seeds"]
macro-paths: ["macros"]
snapshot-paths: ["snapshots"]

clean-targets:         # directories to be removed by `dbt clean`
  - "target"
  - "dbt_packages"


# Configuring models
# Full documentation: https://docs.getdbt.com/docs/configuring-models

# In this example config, we tell dbt to build all models in the example/
# directory as views. These settings can be overridden in the individual model
# files using the `{{{{ config(...) }}}}` macro.
models:
  {project_name}:
    # Config indicated by + and applies to all files under models/example/
    example:
      +materialized: view
"""
            )


class TestInitInsideProjectAndSkipProfileSetup(TestInitInsideOfProjectBase):
    @mock.patch("dbt.task.init.InitTask._run_debug")
    @mock.patch("dbt.task.init._get_adapter_plugin_names")
    @mock.patch("click.confirm")
    @mock.patch("click.prompt")
    def test_init_inside_project_and_skip_profile_setup(
        self, mock_prompt, mock_confirm, mock_get, mock_debug, project, project_name
    ):
        manager = mock.Mock()
        manager.attach_mock(mock_prompt, "prompt")
        manager.attach_mock(mock_confirm, "confirm")

        assert Path("dbt_project.yml").exists()

        # skip interactive profile setup
        run_dbt(["init", "--skip-profile-setup"])
        assert len(manager.mock_calls) == 0
        mock_debug.assert_not_called()


class TestInitOutsideOfProjectWithSpecifiedProfile(TestInitOutsideOfProjectBase):
    @mock.patch("dbt.task.init.InitTask._run_debug")
    @mock.patch("dbt.task.init._get_adapter_plugin_names")
    @mock.patch("click.prompt")
    def test_init_task_outside_of_project_with_specified_profile(
        self,
        mock_prompt,
        mock_get_adapter,
        mock_run_debug,
        project,
        project_name,
        unique_schema,
        dbt_profile_data,
    ):
        manager = Mock()
        manager.attach_mock(mock_prompt, "prompt")
        manager.prompt.side_effect = [
            project_name,
        ]
        mock_get_adapter.return_value = [project.adapter.type()]
        run_dbt(["init", "--profile", "test"])

        manager.assert_has_calls(
            [
                call.prompt("Enter a name for your project (letters, digits, underscore)"),
            ]
        )

        # profiles.yml is NOT overwritten, so assert that the text matches that of the
        # original fixture
        with open(os.path.join(project.profiles_dir, "profiles.yml"), "r") as f:
            assert f.read() == yaml.safe_dump(dbt_profile_data)

        with open(os.path.join(project.project_root, project_name, "dbt_project.yml"), "r") as f:
            assert (
                f.read()
                == f"""
# Name your project! Project names should contain only lowercase characters
# and underscores. A good package name should reflect your organization's
# name or the intended use of these models
name: '{project_name}'
version: '1.0.0'

# This setting configures which "profile" dbt uses for this project.
profile: 'test'

# These configurations specify where dbt should look for different types of files.
# The `model-paths` config, for example, states that models in this project can be
# found in the "models/" directory. You probably won't need to change these!
model-paths: ["models"]
analysis-paths: ["analyses"]
test-paths: ["tests"]
seed-paths: ["seeds"]
macro-paths: ["macros"]
snapshot-paths: ["snapshots"]

clean-targets:         # directories to be removed by `dbt clean`
  - "target"
  - "dbt_packages"


# Configuring models
# Full documentation: https://docs.getdbt.com/docs/configuring-models

# In this example config, we tell dbt to build all models in the example/
# directory as views. These settings can be overridden in the individual model
# files using the `{{{{ config(...) }}}}` macro.
models:
  {project_name}:
    # Config indicated by + and applies to all files under models/example/
    example:
      +materialized: view
"""
            )

            mock_run_debug.assert_called_once()


class TestInitOutsideOfProjectSpecifyingInvalidProfile(TestInitOutsideOfProjectBase):
    @mock.patch("dbt.task.init._get_adapter_plugin_names")
    @mock.patch("click.prompt")
    def test_init_task_outside_project_specifying_invalid_profile_errors(
        self, mock_prompt, mock_get_adapter, project, project_name
    ):
        manager = Mock()
        manager.attach_mock(mock_prompt, "prompt")
        manager.prompt.side_effect = [
            project_name,
        ]
        mock_get_adapter.return_value = [project.adapter.type()]

        with pytest.raises(DbtRuntimeError) as error:
            run_dbt(["init", "--profile", "invalid"], expect_pass=False)
            assert "Could not find profile named invalid" in str(error)

        manager.assert_has_calls(
            [
                call.prompt("Enter a name for your project (letters, digits, underscore)"),
            ]
        )


class TestInitOutsideOfProjectSpecifyingProfileNoProfilesYml(TestInitOutsideOfProjectBase):
    @mock.patch("dbt.task.init._get_adapter_plugin_names")
    @mock.patch("click.prompt")
    def test_init_task_outside_project_specifying_profile_no_profiles_yml_errors(
        self, mock_prompt, mock_get_adapter, project, project_name
    ):
        manager = Mock()
        manager.attach_mock(mock_prompt, "prompt")
        manager.prompt.side_effect = [
            project_name,
        ]
        mock_get_adapter.return_value = [project.adapter.type()]

        # Override responses on specific files, default to 'real world' if not overriden
        original_isfile = os.path.isfile
        with mock.patch(
            "os.path.isfile",
            new=lambda path: {"profiles.yml": False}.get(
                os.path.basename(path), original_isfile(path)
            ),
        ):
            with pytest.raises(DbtRuntimeError) as error:
                run_dbt(["init", "--profile", "test"], expect_pass=False)
                assert "Could not find profile named invalid" in str(error)

        manager.assert_has_calls(
            [
                call.prompt("Enter a name for your project (letters, digits, underscore)"),
            ]
        )


class TestInitRunsDebugAfterInit(TestInitInsideOfProjectBase):
    @mock.patch("dbt.task.init._get_adapter_plugin_names")
    @mock.patch("click.confirm")
    @mock.patch("click.prompt")
    def test_debug_runs_after_init(self, mock_prompt, mock_confirm, mock_get_adapter, project):
        """Verify DebugTask is instantiated and run() is called after init with profile setup."""
        manager = Mock()
        manager.attach_mock(mock_prompt, "prompt")
        manager.attach_mock(mock_confirm, "confirm")
        manager.confirm.side_effect = ["y"]
        manager.prompt.side_effect = [
            1,
            "localhost",
            5432,
            "test_user",
            "test_password",
            "test_db",
            "test_schema",
            4,
        ]
        mock_get_adapter.return_value = [project.adapter.type()]

        mock_debug_task_cls = Mock()

        with mock.patch("dbt.task.debug.DebugTask", mock_debug_task_cls):
            run_dbt(["init"])

        mock_debug_task_cls.return_value.run.assert_called_once()


class TestInitDebugSkippedWithSkipProfileSetup(TestInitInsideOfProjectBase):
    @mock.patch("dbt.task.init.InitTask._run_debug")
    @mock.patch("dbt.task.init._get_adapter_plugin_names")
    @mock.patch("click.confirm")
    @mock.patch("click.prompt")
    def test_debug_skipped_with_skip_profile_setup(
        self, mock_prompt, mock_confirm, mock_get_adapter, mock_run_debug, project
    ):
        """Verify _run_debug is NOT called when --skip-profile-setup is used."""
        run_dbt(["init", "--skip-profile-setup"])
        mock_run_debug.assert_not_called()


class TestInitSkipDebugFlag(TestInitInsideOfProjectBase):
    @mock.patch("dbt.task.init._get_adapter_plugin_names")
    @mock.patch("click.confirm")
    @mock.patch("click.prompt")
    def test_debug_skipped_with_skip_debug_flag(
        self, mock_prompt, mock_confirm, mock_get_adapter, project
    ):
        """Verify DebugTask is NOT instantiated when --skip-debug is used."""
        manager = Mock()
        manager.attach_mock(mock_prompt, "prompt")
        manager.attach_mock(mock_confirm, "confirm")
        manager.confirm.side_effect = ["y"]
        manager.prompt.side_effect = [
            1,
            "localhost",
            5432,
            "test_user",
            "test_password",
            "test_db",
            "test_schema",
            4,
        ]
        mock_get_adapter.return_value = [project.adapter.type()]

        mock_debug_task_cls = Mock()

        with mock.patch("dbt.task.debug.DebugTask", mock_debug_task_cls):
            run_dbt(["init", "--skip-debug"])

        mock_debug_task_cls.assert_not_called()

    def test_run_debug_returns_none_when_skip_debug(self):
        """Verify _run_debug returns None when skip_debug is True."""
        task = Mock(spec=InitTask)
        task.args = Mock(skip_debug=True)
        assert InitTask._run_debug(task) is None


class TestInitDebugFailureDoesNotFailInit(TestInitInsideOfProjectBase):
    @mock.patch("dbt.task.init._get_adapter_plugin_names")
    @mock.patch("click.confirm")
    @mock.patch("click.prompt")
    def test_debug_failure_does_not_fail_init(
        self, mock_prompt, mock_confirm, mock_get_adapter, project
    ):
        """Verify that if DebugTask.run() raises, init still succeeds."""
        manager = Mock()
        manager.attach_mock(mock_prompt, "prompt")
        manager.attach_mock(mock_confirm, "confirm")
        manager.confirm.side_effect = ["y"]
        manager.prompt.side_effect = [
            1,
            "localhost",
            5432,
            "test_user",
            "test_password",
            "test_db",
            "test_schema",
            4,
        ]
        mock_get_adapter.return_value = [project.adapter.type()]

        mock_debug_task_cls = Mock()
        mock_debug_task_cls.return_value.run.side_effect = Exception("connection failed")

        with mock.patch("dbt.task.debug.DebugTask", mock_debug_task_cls):
            # Should not raise — debug failure is informational only
            run_dbt(["init"])
