import os
import random
import shutil
import string

import pytest

from dbt.exceptions import CompilationError, ContractBreakingChangeError
from dbt.tests.util import (
    get_manifest,
    rm_file,
    run_dbt,
    run_dbt_and_capture,
    update_config_file,
    write_file,
)
from tests.functional.defer_state.fixtures import (
    constraint_schema_yml,
    contract_schema_yml,
    disabled_contract_schema_yml,
    disabled_unenforced_contract_schema_yml,
    disabled_versioned_contract_schema_yml,
    disabled_versioned_unenforced_contract_schema_yml,
    ephemeral_model_sql,
    exposures_yml,
    infinite_macros_sql,
    macros_sql,
    metricflow_time_spine_sql,
    modified_column_constraint_schema_yml,
    modified_contract_schema_yml,
    modified_model_constraint_schema_yml,
    modified_semantic_model_schema_yml,
    no_contract_schema_yml,
    schema_yml,
    seed_csv,
    semantic_model_schema_yml,
    table_model_now_incremental_sql,
    table_model_now_view_sql,
    table_model_sql,
    unenforced_contract_schema_yml,
    versioned_contract_schema_yml,
    versioned_modified_contract_schema_yml,
    versioned_no_contract_schema_yml,
    versioned_unenforced_contract_schema_yml,
    view_model_now_table_sql,
    view_model_sql,
)


class BaseModifiedState:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "table_model.sql": table_model_sql,
            "view_model.sql": view_model_sql,
            "ephemeral_model.sql": ephemeral_model_sql,
            "schema.yml": schema_yml,
            "exposures.yml": exposures_yml,
        }

    @pytest.fixture(scope="class")
    def macros(self):
        return {
            "macros.sql": macros_sql,
            "infinite_macros.sql": infinite_macros_sql,
        }

    @pytest.fixture(scope="class")
    def seeds(self):
        return {
            "seed.csv": seed_csv,
        }

    @property
    def project_config_update(self):
        return {
            "seeds": {
                "test": {
                    "quote_columns": False,
                }
            }
        }

    def copy_state(self):
        if not os.path.exists("state"):
            os.makedirs("state")
        shutil.copyfile("target/manifest.json", "state/manifest.json")

    def run_and_save_state(self):
        run_dbt(["seed"])
        run_dbt(["run"])
        self.copy_state()


class TestChangedSeedContents(BaseModifiedState):
    def test_changed_seed_contents_state(self, project):
        self.run_and_save_state()
        results = run_dbt(
            ["ls", "--resource-type", "seed", "--select", "state:modified", "--state", "./state"],
            expect_pass=True,
        )
        assert len(results) == 0

        results = run_dbt(
            [
                "ls",
                "--resource-type",
                "seed",
                "--exclude",
                "state:unmodified",
                "--state",
                "./state",
            ],
            expect_pass=True,
        )
        assert len(results) == 0

        results = run_dbt(
            [
                "ls",
                "--resource-type",
                "seed",
                "--select",
                "state:unmodified",
                "--state",
                "./state",
            ],
            expect_pass=True,
        )
        assert len(results) == 1

        # add a new row to the seed
        changed_seed_contents = seed_csv + "\n" + "3,carl"
        write_file(changed_seed_contents, "seeds", "seed.csv")

        results = run_dbt(
            ["ls", "--resource-type", "seed", "--select", "state:modified", "--state", "./state"]
        )
        assert len(results) == 1
        assert results[0] == "test.seed"

        results = run_dbt(
            [
                "ls",
                "--resource-type",
                "seed",
                "--exclude",
                "state:unmodified",
                "--state",
                "./state",
            ]
        )
        assert len(results) == 1
        assert results[0] == "test.seed"

        results = run_dbt(
            ["ls", "--resource-type", "seed", "--select", "state:unmodified", "--state", "./state"]
        )
        assert len(results) == 0

        results = run_dbt(["ls", "--select", "state:modified", "--state", "./state"])
        assert len(results) == 1
        assert results[0] == "test.seed"

        results = run_dbt(["ls", "--exclude", "state:unmodified", "--state", "./state"])
        assert len(results) == 1
        assert results[0] == "test.seed"

        results = run_dbt(["ls", "--select", "state:unmodified", "--state", "./state"])
        assert len(results) == 6

        results = run_dbt(["ls", "--select", "state:modified+", "--state", "./state"])
        assert len(results) == 7
        assert set(results) == {
            "test.seed",
            "test.table_model",
            "test.view_model",
            "test.ephemeral_model",
            "test.not_null_view_model_id",
            "test.unique_view_model_id",
            "exposure:test.my_exposure",
        }

        results = run_dbt(["ls", "--select", "state:unmodified+", "--state", "./state"])
        assert len(results) == 6
        assert set(results) == {
            "test.table_model",
            "test.view_model",
            "test.ephemeral_model",
            "test.not_null_view_model_id",
            "test.unique_view_model_id",
            "exposure:test.my_exposure",
        }

        shutil.rmtree("./state")
        self.copy_state()

        # make a very big seed
        # assume each line is ~2 bytes + len(name)
        target_size = 1 * 1024 * 1024
        line_size = 64
        num_lines = target_size // line_size
        maxlines = num_lines + 4
        seed_lines = [seed_csv]
        for idx in range(4, maxlines):
            value = "".join(random.choices(string.ascii_letters, k=62))
            seed_lines.append(f"{idx},{value}")
        seed_contents = "\n".join(seed_lines)
        write_file(seed_contents, "seeds", "seed.csv")

        # now if we run again, we should get a warning
        results = run_dbt(
            ["ls", "--resource-type", "seed", "--select", "state:modified", "--state", "./state"]
        )
        assert len(results) == 1
        assert results[0] == "test.seed"

        with pytest.raises(CompilationError) as exc:
            run_dbt(
                [
                    "--warn-error",
                    "ls",
                    "--resource-type",
                    "seed",
                    "--select",
                    "state:modified",
                    "--state",
                    "./state",
                ]
            )
        assert ">1MB" in str(exc.value)

        # now check if unmodified returns none
        results = run_dbt(
            ["ls", "--resource-type", "seed", "--select", "state:unmodified", "--state", "./state"]
        )
        assert len(results) == 0

        shutil.rmtree("./state")
        self.copy_state()

        # once it"s in path mode, we don"t mark it as modified if it changes
        write_file(seed_contents + "\n1,test", "seeds", "seed.csv")

        results = run_dbt(
            ["ls", "--resource-type", "seed", "--select", "state:modified", "--state", "./state"],
            expect_pass=True,
        )
        assert len(results) == 0

        results = run_dbt(
            [
                "ls",
                "--resource-type",
                "seed",
                "--exclude",
                "state:unmodified",
                "--state",
                "./state",
            ],
            expect_pass=True,
        )
        assert len(results) == 0

        results = run_dbt(
            [
                "ls",
                "--resource-type",
                "seed",
                "--select",
                "state:unmodified",
                "--state",
                "./state",
            ],
            expect_pass=True,
        )
        assert len(results) == 1


class TestChangedSeedConfig(BaseModifiedState):
    def test_changed_seed_config(self, project):
        self.run_and_save_state()
        results = run_dbt(
            ["ls", "--resource-type", "seed", "--select", "state:modified", "--state", "./state"],
            expect_pass=True,
        )
        assert len(results) == 0

        results = run_dbt(
            [
                "ls",
                "--resource-type",
                "seed",
                "--exclude",
                "state:unmodified",
                "--state",
                "./state",
            ],
            expect_pass=True,
        )
        assert len(results) == 0

        results = run_dbt(
            [
                "ls",
                "--resource-type",
                "seed",
                "--select",
                "state:unmodified",
                "--state",
                "./state",
            ],
            expect_pass=True,
        )
        assert len(results) == 1

        update_config_file({"seeds": {"test": {"quote_columns": False}}}, "dbt_project.yml")

        # quoting change -> seed changed
        results = run_dbt(
            ["ls", "--resource-type", "seed", "--select", "state:modified", "--state", "./state"]
        )
        assert len(results) == 1
        assert results[0] == "test.seed"

        results = run_dbt(
            [
                "ls",
                "--resource-type",
                "seed",
                "--exclude",
                "state:unmodified",
                "--state",
                "./state",
            ]
        )
        assert len(results) == 1
        assert results[0] == "test.seed"

        results = run_dbt(
            ["ls", "--resource-type", "seed", "--select", "state:unmodified", "--state", "./state"]
        )
        assert len(results) == 0


class TestUnrenderedConfigSame(BaseModifiedState):
    def test_unrendered_config_same(self, project):
        self.run_and_save_state()
        results = run_dbt(
            ["ls", "--resource-type", "model", "--select", "state:modified", "--state", "./state"],
            expect_pass=True,
        )
        assert len(results) == 0

        results = run_dbt(
            [
                "ls",
                "--resource-type",
                "model",
                "--exclude",
                "state:unmodified",
                "--state",
                "./state",
            ],
            expect_pass=True,
        )
        assert len(results) == 0

        results = run_dbt(
            [
                "ls",
                "--resource-type",
                "model",
                "--select",
                "state:unmodified",
                "--state",
                "./state",
            ],
            expect_pass=True,
        )
        assert len(results) == 3

        # although this is the default value, dbt will recognize it as a change
        # for previously-unconfigured models, because it"s been explicitly set
        update_config_file({"models": {"test": {"materialized": "view"}}}, "dbt_project.yml")
        results = run_dbt(
            ["ls", "--resource-type", "model", "--select", "state:modified", "--state", "./state"]
        )
        assert len(results) == 1
        assert results[0] == "test.view_model"

        # converse of above statement
        results = run_dbt(
            [
                "ls",
                "--resource-type",
                "model",
                "--exclude",
                "state:unmodified",
                "--state",
                "./state",
            ]
        )
        assert len(results) == 1
        assert results[0] == "test.view_model"

        results = run_dbt(
            [
                "ls",
                "--resource-type",
                "model",
                "--select",
                "state:unmodified",
                "--state",
                "./state",
            ]
        )
        assert len(results) == 2
        assert set(results) == {
            "test.table_model",
            "test.ephemeral_model",
        }


class TestChangedModelContents(BaseModifiedState):
    def test_changed_model_contents(self, project):
        self.run_and_save_state()
        results = run_dbt(["run", "--models", "state:modified", "--state", "./state"])
        assert len(results) == 0

        table_model_update = """
        {{ config(materialized="table") }}

        select * from {{ ref("seed") }}
        """

        write_file(table_model_update, "models", "table_model.sql")

        results = run_dbt(["run", "--models", "state:modified", "--state", "./state"])
        assert len(results) == 1
        assert results[0].node.name == "table_model"

        results = run_dbt(["run", "--exclude", "state:unmodified", "--state", "./state"])
        assert len(results) == 1
        assert results[0].node.name == "table_model"


class TestNewMacro(BaseModifiedState):
    def test_new_macro(self, project):
        self.run_and_save_state()

        new_macro = """
            {% macro my_other_macro() %}
            {% endmacro %}
        """

        # add a new macro to a new file
        write_file(new_macro, "macros", "second_macro.sql")

        results = run_dbt(["run", "--models", "state:modified", "--state", "./state"])
        assert len(results) == 0

        os.remove("macros/second_macro.sql")
        # add a new macro to the existing file
        with open("macros/macros.sql", "a") as fp:
            fp.write(new_macro)

        results = run_dbt(["run", "--models", "state:modified", "--state", "./state"])
        assert len(results) == 0

        results = run_dbt(["run", "--exclude", "state:unmodified", "--state", "./state"])
        assert len(results) == 0


class TestChangedMacroContents(BaseModifiedState):
    def test_changed_macro_contents(self, project):
        self.run_and_save_state()

        # modify an existing macro
        updated_macro = """
        {% macro my_macro() %}
            {% do log("in a macro", info=True) %}
        {% endmacro %}
        """
        write_file(updated_macro, "macros", "macros.sql")

        # table_model calls this macro
        results = run_dbt(["run", "--models", "state:modified", "--state", "./state"])
        assert len(results) == 1

        results = run_dbt(["run", "--exclude", "state:unmodified", "--state", "./state"])
        assert len(results) == 1


class TestChangedExposure(BaseModifiedState):
    def test_changed_exposure(self, project):
        self.run_and_save_state()

        # add an "owner.name" to existing exposure
        updated_exposure = exposures_yml + "\n      name: John Doe\n"
        write_file(updated_exposure, "models", "exposures.yml")

        results = run_dbt(["run", "--models", "+state:modified", "--state", "./state"])
        assert len(results) == 1
        assert results[0].node.name == "view_model"

        results = run_dbt(["run", "--exclude", "state:unmodified", "--state", "./state"])
        assert len(results) == 0


class TestChangedContractUnversioned(BaseModifiedState):
    MODEL_UNIQUE_ID = "model.test.table_model"
    CONTRACT_SCHEMA_YML = contract_schema_yml
    MODIFIED_SCHEMA_YML = modified_contract_schema_yml
    UNENFORCED_SCHEMA_YML = unenforced_contract_schema_yml
    NO_CONTRACT_SCHEMA_YML = no_contract_schema_yml

    def test_changed_contract(self, project):
        self.run_and_save_state()

        # update contract for table_model
        write_file(self.CONTRACT_SCHEMA_YML, "models", "schema.yml")

        # This will find the table_model node modified both through a config change
        # and by a non-breaking change to contract: true
        results = run_dbt(["run", "--models", "state:modified", "--state", "./state"])
        assert len(results) == 1
        assert results[0].node.name == "table_model"

        results = run_dbt(["run", "--exclude", "state:unmodified", "--state", "./state"])
        assert len(results) == 1
        assert results[0].node.name == "table_model"

        manifest = get_manifest(project.project_root)
        model_unique_id = self.MODEL_UNIQUE_ID
        model = manifest.nodes[model_unique_id]
        expected_unrendered_config = {"contract": {"enforced": True}, "materialized": "table"}
        assert model.unrendered_config == expected_unrendered_config

        # Run it again with "state:modified:contract", still finds modified due to contract: true
        results = run_dbt(["run", "--models", "state:modified.contract", "--state", "./state"])
        assert len(results) == 1
        manifest = get_manifest(project.project_root)
        model = manifest.nodes[model_unique_id]
        first_contract_checksum = model.contract.checksum
        assert first_contract_checksum
        # save a new state
        self.copy_state()

        # This should raise because a column name has changed
        write_file(self.MODIFIED_SCHEMA_YML, "models", "schema.yml")
        results = run_dbt(["run"], expect_pass=False)
        assert len(results) == 2
        manifest = get_manifest(project.project_root)
        model = manifest.nodes[model_unique_id]
        second_contract_checksum = model.contract.checksum
        # double check different contract_checksums
        assert first_contract_checksum != second_contract_checksum

        _, logs = run_dbt_and_capture(
            ["run", "--models", "state:modified.contract", "--state", "./state"], expect_pass=False
        )
        expected_error = "This model has an enforced contract that failed."
        expected_warning = "While comparing to previous project state, dbt detected a breaking change to an unversioned model"
        expected_change = "Please ensure the name, data_type, and number of columns in your contract match the columns in your model's definition"
        assert expected_error in logs
        assert expected_warning in logs
        assert expected_change in logs

        # Go back to schema file without contract. Should throw a warning.
        write_file(self.NO_CONTRACT_SCHEMA_YML, "models", "schema.yml")
        _, logs = run_dbt_and_capture(
            ["run", "--models", "state:modified.contract", "--state", "./state"]
        )
        expected_warning = "While comparing to previous project state, dbt detected a breaking change to an unversioned model"
        expected_change = "Contract enforcement was removed"

        # Now unenforce the contract. Should throw a warning - force warning into an error.
        write_file(self.UNENFORCED_SCHEMA_YML, "models", "schema.yml")
        with pytest.raises(CompilationError):
            _, logs = run_dbt_and_capture(
                [
                    "--warn-error",
                    "run",
                    "--models",
                    "state:modified.contract",
                    "--state",
                    "./state",
                ]
            )
            expected_warning = "While comparing to previous project state, dbt detected a breaking change to an unversioned model"
            expected_change = "Contract enforcement was removed"


class TestChangedContractVersioned(BaseModifiedState):
    MODEL_UNIQUE_ID = "model.test.table_model.v1"
    CONTRACT_SCHEMA_YML = versioned_contract_schema_yml
    MODIFIED_SCHEMA_YML = versioned_modified_contract_schema_yml
    UNENFORCED_SCHEMA_YML = versioned_unenforced_contract_schema_yml
    NO_CONTRACT_SCHEMA_YML = versioned_no_contract_schema_yml

    def test_changed_contract_versioned(self, project):
        self.run_and_save_state()

        # update contract for table_model
        write_file(self.CONTRACT_SCHEMA_YML, "models", "schema.yml")

        # This will find the table_model node modified both through a config change
        # and by a non-breaking change to contract: true
        results = run_dbt(["run", "--models", "state:modified", "--state", "./state"])
        assert len(results) == 1
        assert results[0].node.name == "table_model"

        results = run_dbt(["run", "--exclude", "state:unmodified", "--state", "./state"])
        assert len(results) == 1
        assert results[0].node.name == "table_model"

        manifest = get_manifest(project.project_root)
        model_unique_id = self.MODEL_UNIQUE_ID
        model = manifest.nodes[model_unique_id]
        expected_unrendered_config = {"contract": {"enforced": True}, "materialized": "table"}
        assert model.unrendered_config == expected_unrendered_config

        # Run it again with "state:modified:contract", still finds modified due to contract: true
        results = run_dbt(["run", "--models", "state:modified.contract", "--state", "./state"])
        assert len(results) == 1
        manifest = get_manifest(project.project_root)
        model = manifest.nodes[model_unique_id]
        first_contract_checksum = model.contract.checksum
        assert first_contract_checksum
        # save a new state
        self.copy_state()

        # This should raise because a column name has changed
        write_file(self.MODIFIED_SCHEMA_YML, "models", "schema.yml")
        results = run_dbt(["run"], expect_pass=False)
        assert len(results) == 2
        manifest = get_manifest(project.project_root)
        model = manifest.nodes[model_unique_id]
        second_contract_checksum = model.contract.checksum
        # double check different contract_checksums
        assert first_contract_checksum != second_contract_checksum
        with pytest.raises(ContractBreakingChangeError):
            results = run_dbt(["run", "--models", "state:modified.contract", "--state", "./state"])

        # Go back to schema file without contract. Should raise an error.
        write_file(self.NO_CONTRACT_SCHEMA_YML, "models", "schema.yml")
        with pytest.raises(ContractBreakingChangeError):
            results = run_dbt(["run", "--models", "state:modified.contract", "--state", "./state"])

        # Now unenforce the contract. Should raise an error.
        write_file(self.UNENFORCED_SCHEMA_YML, "models", "schema.yml")
        with pytest.raises(ContractBreakingChangeError):
            results = run_dbt(["run", "--models", "state:modified.contract", "--state", "./state"])


class TestDeleteUnversionedContractedModel(BaseModifiedState):
    MODEL_UNIQUE_ID = "model.test.table_model"
    CONTRACT_SCHEMA_YML = contract_schema_yml

    def test_delete_unversioned_contracted_model(self, project):
        # ensure table_model is contracted
        write_file(self.CONTRACT_SCHEMA_YML, "models", "schema.yml")
        self.run_and_save_state()

        # delete versioned contracted model
        rm_file(project.project_root, "models", "table_model.sql")

        # since the models are unversioned, they raise a warning but not an error
        _, logs = run_dbt_and_capture(
            ["run", "--models", "state:modified.contract", "--state", "./state"]
        )

        expected_warning = "While comparing to previous project state, dbt detected a breaking change to an unversioned model"
        expected_change = "Contracted model 'model.test.table_model' was deleted or renamed"
        assert expected_warning in logs
        assert expected_change in logs


class TestDeleteVersionedContractedModel(BaseModifiedState):
    MODEL_UNIQUE_ID = "model.test.table_model.v1"
    CONTRACT_SCHEMA_YML = versioned_contract_schema_yml

    def test_delete_versioned_contracted_model(self, project):
        # ensure table_model is versioned + contracted
        write_file(self.CONTRACT_SCHEMA_YML, "models", "schema.yml")
        self.run_and_save_state()

        # delete versioned contracted model
        rm_file(project.project_root, "models", "table_model.sql")

        # since the models are versioned, they raise an error
        with pytest.raises(ContractBreakingChangeError) as e:
            run_dbt(["run", "--models", "state:modified.contract", "--state", "./state"])

        assert "Contracted model 'model.test.table_model.v1' was deleted or renamed." in str(
            e.value
        )


class TestDisableUnversionedContractedModel(BaseModifiedState):
    MODEL_UNIQUE_ID = "model.test.table_model"
    CONTRACT_SCHEMA_YML = contract_schema_yml
    DISABLED_CONTRACT_SCHEMA_YML = disabled_contract_schema_yml

    def test_disable_unversioned_contracted_model(self, project):
        # ensure table_model is contracted and enabled
        write_file(self.CONTRACT_SCHEMA_YML, "models", "schema.yml")
        self.run_and_save_state()

        # disable unversioned + contracted model
        write_file(self.DISABLED_CONTRACT_SCHEMA_YML, "models", "schema.yml")

        # since the models are unversioned, they raise a warning but not an error
        _, logs = run_dbt_and_capture(
            ["run", "--models", "state:modified.contract", "--state", "./state"]
        )

        expected_warning = "While comparing to previous project state, dbt detected a breaking change to an unversioned model"
        expected_change = "Contracted model 'model.test.table_model' was disabled"
        assert expected_warning in logs
        assert expected_change in logs


class TestDisableVersionedContractedModel(BaseModifiedState):
    MODEL_UNIQUE_ID = "model.test.table_model.v1"
    CONTRACT_SCHEMA_YML = versioned_contract_schema_yml
    DISABLED_CONTRACT_SCHEMA_YML = disabled_versioned_contract_schema_yml

    def test_disable_versioned_contracted_model(self, project):
        # ensure table_model is versioned + contracted
        write_file(self.CONTRACT_SCHEMA_YML, "models", "schema.yml")
        self.run_and_save_state()

        # disable versioned + contracted model
        write_file(self.DISABLED_CONTRACT_SCHEMA_YML, "models", "schema.yml")

        # since the models are versioned, they raise an error
        with pytest.raises(ContractBreakingChangeError) as e:
            run_dbt(["run", "--models", "state:modified.contract", "--state", "./state"])

        assert "Contracted model 'model.test.table_model.v1' was disabled." in str(e.value)


class TestDisableUnversionedUncontractedModel(BaseModifiedState):
    MODEL_UNIQUE_ID = "model.test.table_model"
    NO_CONTRACT_SCHEMA_YML = unenforced_contract_schema_yml
    DISABLED_NO_CONTRACT_SCHEMA_YML = disabled_unenforced_contract_schema_yml

    def test_delete_versioned_contracted_model(self, project):
        # ensure table_model is not contracted
        write_file(self.NO_CONTRACT_SCHEMA_YML, "models", "schema.yml")
        self.run_and_save_state()

        # disable uncontracted model
        write_file(self.DISABLED_NO_CONTRACT_SCHEMA_YML, "models", "schema.yml")

        # since the models are unversioned, no warning or error is raised
        _, logs = run_dbt_and_capture(
            ["run", "--models", "state:modified.contract", "--state", "./state"]
        )

        assert "breaking change" not in logs.lower()


class TestDisableVersionedUncontractedModel(BaseModifiedState):
    MODEL_UNIQUE_ID = "model.test.table_model.v1"
    NO_CONTRACT_SCHEMA_YML = versioned_unenforced_contract_schema_yml
    DISABLED_NO_CONTRACT_SCHEMA_YML = disabled_versioned_unenforced_contract_schema_yml

    def test_delete_versioned_contracted_model(self, project):
        # ensure table_model is not contracted
        write_file(self.NO_CONTRACT_SCHEMA_YML, "models", "schema.yml")
        self.run_and_save_state()

        # disable uncontracted model
        write_file(self.DISABLED_NO_CONTRACT_SCHEMA_YML, "models", "schema.yml")

        # since the models are unversioned, no warning or error is raised
        run_dbt_and_capture(["run", "--models", "state:modified.contract", "--state", "./state"])


class TestChangedConstraintUnversioned(BaseModifiedState):
    def test_changed_constraint(self, project):
        self.run_and_save_state()

        # update constraint for table_model
        write_file(constraint_schema_yml, "models", "schema.yml")

        # This will find the table_model node modified both through adding constraint
        # and by a non-breaking change to contract: true
        results = run_dbt(["run", "--models", "state:modified", "--state", "./state"])
        assert len(results) == 1
        assert results[0].node.name == "table_model"

        results = run_dbt(["run", "--exclude", "state:unmodified", "--state", "./state"])
        assert len(results) == 1
        assert results[0].node.name == "table_model"

        manifest = get_manifest(project.project_root)
        model_unique_id = "model.test.table_model"
        model = manifest.nodes[model_unique_id]
        expected_unrendered_config = {"contract": {"enforced": True}, "materialized": "table"}
        assert model.unrendered_config == expected_unrendered_config

        # Run it again with "state:modified:contract", still finds modified due to contract: true
        results = run_dbt(["run", "--models", "state:modified.contract", "--state", "./state"])
        assert len(results) == 1
        manifest = get_manifest(project.project_root)
        model = manifest.nodes[model_unique_id]
        first_contract_checksum = model.contract.checksum
        assert first_contract_checksum
        # save a new state
        self.copy_state()

        # This should raise because a column level constraint was removed
        write_file(modified_column_constraint_schema_yml, "models", "schema.yml")
        # we don't have a way to know this failed unless we have a previous state to refer to, so the run succeeds
        results = run_dbt(["run"])
        assert len(results) == 2
        manifest = get_manifest(project.project_root)
        model = manifest.nodes[model_unique_id]
        second_contract_checksum = model.contract.checksum
        # double check different contract_checksums
        assert first_contract_checksum != second_contract_checksum
        # since the models are unversioned, they raise a warning but not an error
        _, logs = run_dbt_and_capture(
            ["run", "--models", "state:modified.contract", "--state", "./state"]
        )
        expected_warning = "While comparing to previous project state, dbt detected a breaking change to an unversioned model"
        expected_change = "Enforced column level constraints were removed"
        assert expected_warning in logs
        assert expected_change in logs

        # This should raise because a model level constraint was removed (primary_key on id)
        write_file(modified_model_constraint_schema_yml, "models", "schema.yml")
        # we don't have a way to know this failed unless we have a previous state to refer to, so the run succeeds
        results = run_dbt(["run"])
        assert len(results) == 2
        manifest = get_manifest(project.project_root)
        model = manifest.nodes[model_unique_id]
        second_contract_checksum = model.contract.checksum
        # double check different contract_checksums
        assert first_contract_checksum != second_contract_checksum
        _, logs = run_dbt_and_capture(
            ["run", "--models", "state:modified.contract", "--state", "./state"]
        )
        expected_warning = "While comparing to previous project state, dbt detected a breaking change to an unversioned model"
        expected_change = "Enforced model level constraints were removed"
        assert expected_warning in logs
        assert expected_change in logs


class TestChangedMaterializationConstraint(BaseModifiedState):
    def test_changed_materialization(self, project):
        self.run_and_save_state()

        # update constraint for table_model
        write_file(constraint_schema_yml, "models", "schema.yml")

        # This will find the table_model node modified both through adding constraint
        # and by a non-breaking change to contract: true
        results = run_dbt(["run", "--models", "state:modified", "--state", "./state"])
        assert len(results) == 1
        assert results[0].node.name == "table_model"

        results = run_dbt(["run", "--exclude", "state:unmodified", "--state", "./state"])
        assert len(results) == 1
        assert results[0].node.name == "table_model"

        manifest = get_manifest(project.project_root)
        model_unique_id = "model.test.table_model"
        model = manifest.nodes[model_unique_id]
        expected_unrendered_config = {"contract": {"enforced": True}, "materialized": "table"}
        assert model.unrendered_config == expected_unrendered_config

        # Run it again with "state:modified:contract", still finds modified due to contract: true
        results = run_dbt(["run", "--models", "state:modified.contract", "--state", "./state"])
        assert len(results) == 1
        manifest = get_manifest(project.project_root)
        model = manifest.nodes[model_unique_id]
        first_contract_checksum = model.contract.checksum
        assert first_contract_checksum
        # save a new state
        self.copy_state()

        # This should raise because materialization changed from table to view
        write_file(table_model_now_view_sql, "models", "table_model.sql")
        # we don't have a way to know this failed unless we have a previous state to refer to, so the run succeeds
        results = run_dbt(["run"])
        assert len(results) == 2
        manifest = get_manifest(project.project_root)
        model = manifest.nodes[model_unique_id]
        second_contract_checksum = model.contract.checksum
        # double check different contract_checksums
        assert first_contract_checksum != second_contract_checksum
        _, logs = run_dbt_and_capture(
            ["run", "--models", "state:modified.contract", "--state", "./state"]
        )
        expected_warning = "While comparing to previous project state, dbt detected a breaking change to an unversioned model"
        expected_change = "Materialization changed with enforced constraints"
        assert expected_warning in logs
        assert expected_change in logs

        # This should not raise because materialization changed from table to incremental, both enforce constraints
        write_file(table_model_now_incremental_sql, "models", "table_model.sql")
        # we don't have a way to know this failed unless we have a previous state to refer to, so the run succeeds
        results = run_dbt(["run"])
        assert len(results) == 2

        # This should pass because materialization changed from view to table which is the same as just adding new constraint, not breaking
        write_file(view_model_now_table_sql, "models", "view_model.sql")
        write_file(table_model_sql, "models", "table_model.sql")
        results = run_dbt(["run"])
        assert len(results) == 2
        manifest = get_manifest(project.project_root)
        model = manifest.nodes[model_unique_id]
        second_contract_checksum = model.contract.checksum
        # contract_checksums should be equal because we only save constraint related changes if the materialization is table/incremental
        assert first_contract_checksum == second_contract_checksum
        run_dbt(["run", "--models", "state:modified.contract", "--state", "./state"])
        assert len(results) == 2


my_model_sql = """
select 1 as id
"""

modified_my_model_sql = """
-- a comment
select 1 as id
"""

modified_my_model_non_breaking_sql = """
-- a comment
select 1 as id, 'blue' as color
"""

my_model_yml = """
models:
  - name: my_model
    latest_version: 1
    config:
      contract:
        enforced: true
    columns:
      - name: id
        data_type: int
    versions:
      - v: 1
"""

modified_my_model_yml = """
models:
  - name: my_model
    latest_version: 1
    config:
      contract:
        enforced: true
    columns:
      - name: id
        data_type: text
    versions:
      - v: 1
"""

modified_my_model_non_breaking_yml = """
models:
  - name: my_model
    latest_version: 1
    config:
      contract:
        enforced: true
    columns:
      - name: id
        data_type: int
      - name: color
        data_type: text
    versions:
      - v: 1
"""


class TestModifiedBodyAndContract:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "my_model.sql": my_model_sql,
            "my_model.yml": my_model_yml,
        }

    def copy_state(self):
        if not os.path.exists("state"):
            os.makedirs("state")
        shutil.copyfile("target/manifest.json", "state/manifest.json")

    def test_modified_body_and_contract(self, project):
        results = run_dbt(["run"])
        assert len(results) == 1
        self.copy_state()

        # Change both body and contract in a *breaking* way (= changing data_type of existing column)
        write_file(modified_my_model_yml, "models", "my_model.yml")
        write_file(modified_my_model_sql, "models", "my_model.sql")

        # Should raise even without specifying state:modified.contract
        with pytest.raises(ContractBreakingChangeError):
            results = run_dbt(["run", "-s", "state:modified", "--state", "./state"])

        with pytest.raises(ContractBreakingChangeError):
            results = run_dbt(["run", "--exclude", "state:unmodified", "--state", "./state"])

        # Change both body and contract in a *non-breaking* way (= adding a new column)
        write_file(modified_my_model_non_breaking_yml, "models", "my_model.yml")
        write_file(modified_my_model_non_breaking_sql, "models", "my_model.sql")

        # Should pass
        run_dbt(["run", "-s", "state:modified", "--state", "./state"])

        # The model's contract has changed, even if non-breaking, so it should be selected by 'state:modified.contract'
        results = run_dbt(["list", "-s", "state:modified.contract", "--state", "./state"])
        assert results == ["test.my_model.v1"]


modified_table_model_access_yml = """
version: 2
models:
  - name: table_model
    access: public
"""


class TestModifiedAccess(BaseModifiedState):
    def test_changed_access(self, project):
        self.run_and_save_state()

        # No access change
        assert not run_dbt(["list", "-s", "state:modified", "--state", "./state"])

        # Modify access (protected -> public)
        write_file(modified_table_model_access_yml, "models", "schema.yml")
        assert run_dbt(["list", "-s", "state:modified", "--state", "./state"])

        results = run_dbt(["list", "-s", "state:modified", "--state", "./state"])
        assert results == ["test.table_model"]


modified_table_model_access_yml = """
version: 2
models:
  - name: table_model
    deprecation_date: 2020-01-01
"""


class TestModifiedDeprecationDate(BaseModifiedState):
    def test_changed_access(self, project):
        self.run_and_save_state()

        # No access change
        assert not run_dbt(["list", "-s", "state:modified", "--state", "./state"])

        # Modify deprecation_date (None -> 2020-01-01)
        write_file(modified_table_model_access_yml, "models", "schema.yml")
        assert run_dbt(["list", "-s", "state:modified", "--state", "./state"])

        results = run_dbt(["list", "-s", "state:modified", "--state", "./state"])
        assert results == ["test.table_model"]


modified_table_model_version_yml = """
version: 2
models:
  - name: table_model
    versions:
      - v: 1
        defined_in: table_model
"""


class TestModifiedVersion(BaseModifiedState):
    def test_changed_access(self, project):
        self.run_and_save_state()

        # Change version (null -> v1)
        write_file(modified_table_model_version_yml, "models", "schema.yml")

        results = run_dbt(["list", "-s", "state:modified", "--state", "./state"])
        assert results == ["test.table_model.v1"]


table_model_latest_version_yml = """
version: 2
models:
  - name: table_model
    latest_version: 1
    versions:
      - v: 1
        defined_in: table_model
"""


modified_table_model_latest_version_yml = """
version: 2
models:
  - name: table_model
    latest_version: 2
    versions:
      - v: 1
        defined_in: table_model
      - v: 2
"""


class TestModifiedLatestVersion(BaseModifiedState):
    def test_changed_access(self, project):
        # Setup initial latest_version: 1
        write_file(table_model_latest_version_yml, "models", "schema.yml")

        self.run_and_save_state()

        # Bump latest version
        write_file(table_model_sql, "models", "table_model_v2.sql")
        write_file(modified_table_model_latest_version_yml, "models", "schema.yml")

        results = run_dbt(["list", "-s", "state:modified", "--state", "./state"])
        assert results == ["test.table_model.v1", "test.table_model.v2"]


class TestChangedSemanticModelContents(BaseModifiedState):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "view_model.sql": view_model_sql,
            "schema.yml": semantic_model_schema_yml,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
        }

    def test_changed_semantic_model_contents(self, project):
        self.run_and_save_state()
        results = run_dbt(["list", "-s", "state:modified", "--state", "./state"])
        assert len(results) == 0

        write_file(modified_semantic_model_schema_yml, "models", "schema.yml")
        results = run_dbt(["list", "-s", "state:modified", "--state", "./state"])
        assert len(results) == 1


class TestModifiedVarsLegacy(BaseModifiedState):
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "flags": {
                "state_modified_compare_vars": False,
            },
            "vars": {"my_var": 1},
        }

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "model_with_var.sql": "select {{ var('my_var') }} as id",
        }

    def test_changed_vars(self, project):
        self.run_and_save_state()

        # No var change
        assert not run_dbt(["list", "-s", "state:modified", "--state", "./state"])
        assert not run_dbt(["list", "-s", "state:modified.vars", "--state", "./state"])

        # Modify var (my_var: 1 -> 2)
        update_config_file({"vars": {"my_var": 2}}, "dbt_project.yml")

        # By default, do not detect vars in state:modified to preserve legacy behaviour
        assert run_dbt(["list", "-s", "state:modified", "--state", "./state"]) == []

        # state:modified.vars is a new selector, opt-in method -> returns results
        assert run_dbt(["list", "-s", "state:modified.vars", "--state", "./state"]) == [
            "test.model_with_var"
        ]

        # Reset dbt_project.yml
        update_config_file({"vars": {"my_var": 1}}, "dbt_project.yml")

        # Modify var via --var CLI flag
        assert not run_dbt(
            ["list", "--vars", '{"my_var": 1}', "-s", "state:modified", "--state", "./state"]
        )
        assert (
            run_dbt(
                ["list", "--vars", '{"my_var": 2}', "-s", "state:modified", "--state", "./state"]
            )
            == []
        )
        # state:modified.vars is a new selector, opt-in method -> returns results
        assert run_dbt(
            ["list", "--vars", '{"my_var": 2}', "-s", "state:modified.vars", "--state", "./state"]
        ) == ["test.model_with_var"]


class TestModifiedVars(BaseModifiedState):
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "flags": {
                "state_modified_compare_vars": True,
            },
            "vars": {"my_var": 1},
        }

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "model_with_var.sql": "select {{ var('my_var') }} as id",
        }

    def test_changed_vars(self, project):
        self.run_and_save_state()

        # No var change
        assert not run_dbt(["list", "-s", "state:modified", "--state", "./state"])
        assert not run_dbt(["list", "-s", "state:modified.vars", "--state", "./state"])

        # Modify var (my_var: 1 -> 2)
        update_config_file({"vars": {"my_var": 2}}, "dbt_project.yml")
        assert run_dbt(["list", "-s", "state:modified", "--state", "./state"]) == [
            "test.model_with_var"
        ]
        assert run_dbt(["list", "-s", "state:modified.vars", "--state", "./state"]) == [
            "test.model_with_var"
        ]

        # Reset dbt_project.yml
        update_config_file({"vars": {"my_var": 1}}, "dbt_project.yml")

        # Modify var via --var CLI flag
        assert not run_dbt(
            ["list", "--vars", '{"my_var": 1}', "-s", "state:modified", "--state", "./state"]
        )
        assert run_dbt(
            ["list", "--vars", '{"my_var": 2}', "-s", "state:modified", "--state", "./state"]
        ) == ["test.model_with_var"]


macro_with_var_sql = """
{% macro macro_with_var() %}
    {{ var('my_var') }}
{% endmacro %}
"""


class TestModifiedMacroVars(BaseModifiedState):
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "flags": {
                "state_modified_compare_vars": True,
            },
            "vars": {"my_var": 1},
        }

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "model_with_macro.sql": "select {{ macro_with_var() }} as id",
        }

    @pytest.fixture(scope="class")
    def macros(self):
        return {
            "macro_with_var.sql": macro_with_var_sql,
        }

    def test_changed_vars(self, project):
        self.run_and_save_state()

        # No var change
        assert not run_dbt(["list", "-s", "state:modified", "--state", "./state"])
        assert not run_dbt(["list", "-s", "state:modified.vars", "--state", "./state"])

        # Modify var (my_var: 1 -> 2)
        update_config_file({"vars": {"my_var": 2}}, "dbt_project.yml")
        assert run_dbt(["list", "-s", "state:modified", "--state", "./state"]) == [
            "test.model_with_macro"
        ]
        assert run_dbt(["list", "-s", "state:modified.vars", "--state", "./state"]) == [
            "test.model_with_macro"
        ]

        # Macros themselves not captured as modified because the var value depends on a node's context
        assert not run_dbt(["list", "-s", "state:modified.macros", "--state", "./state"])


# TODO: test versioned models, tests
model_with_var_schema_yml = """
version: 2
models:
  - name: model_with_var
    config:
        materialized: "{{ var('my_var') }}"

exposures:
  - name: exposure_name
    type: dashboard
    owner:
      name: "{{ var('my_var') }}"

sources:
  - name: jaffle_shop
    database: "{{ var('my_var') }}"
    schema: jaffle_shop
    tables:
      - name: orders
      - name: customers
"""


class TestModifiedVarsSchemaYml(BaseModifiedState):
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "flags": {
                "state_modified_compare_vars": True,
            },
            "vars": {"my_var": "table"},
        }

    @pytest.fixture(scope="class")
    def models(self):
        return {"model_with_var.sql": "select 1 as id", "schema.yml": model_with_var_schema_yml}

    def test_changed_vars(self, project):
        self.run_and_save_state()

        # No var change
        assert not run_dbt(["list", "-s", "state:modified", "--state", "./state"])
        assert not run_dbt(["list", "-s", "state:modified.vars", "--state", "./state"])

        # Modify var (my_var: table -> view)
        update_config_file({"vars": {"my_var": "view"}}, "dbt_project.yml")
        assert sorted(run_dbt(["list", "-s", "state:modified", "--state", "./state"])) == sorted(
            [
                "test.model_with_var",
                "exposure:test.exposure_name",
                "source:test.jaffle_shop.customers",
                "source:test.jaffle_shop.orders",
            ]
        )
        assert sorted(
            run_dbt(["list", "-s", "state:modified.vars", "--state", "./state"])
        ) == sorted(
            [
                "test.model_with_var",
                "exposure:test.exposure_name",
                "source:test.jaffle_shop.customers",
                "source:test.jaffle_shop.orders",
            ]
        )
