import json
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
    numeric_precision_contract_schema_yml,
    numeric_precision_increased_contract_schema_yml,
    schema_yml,
    seed_csv,
    semantic_model_schema_yml,
    table_model_now_incremental_sql,
    table_model_now_view_sql,
    table_model_sql,
    unenforced_contract_schema_yml,
    varchar_size_contract_schema_yml,
    varchar_size_increased_contract_schema_yml,
    varchar_size_lowercase_increased_contract_schema_yml,
    varchar_size_unversioned_contract_schema_yml,
    varchar_size_unversioned_increased_contract_schema_yml,
    varchar_size_uppercase_contract_schema_yml,
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
        assert ">1MiB" in str(exc.value)

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

        # the same but for general-purpose state:modified
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

        # the same but for general-purpose state:modified
        with pytest.raises(ContractBreakingChangeError) as e:
            run_dbt(["run", "--models", "state:modified", "--state", "./state"])

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


class TestVersionedContractVarcharSizeChange(BaseModifiedState):
    """
    Test that changing varchar size (e.g., varchar(5) to varchar(20))
    does not trigger a breaking change error for versioned models.
    Per dbt docs, size/precision/scale changes should NOT be breaking changes.
    Reproduces issue: https://github.com/dbt-labs/dbt-core/issues/11186
    """

    MODEL_UNIQUE_ID = "model.test.table_model.v1"

    def test_varchar_size_increase_not_breaking(self, project):
        # Start with varchar(5)
        write_file(varchar_size_contract_schema_yml, "models", "schema.yml")
        self.run_and_save_state()

        # Change to varchar(20) - should NOT be a breaking change
        write_file(varchar_size_increased_contract_schema_yml, "models", "schema.yml")

        # This should PASS without errors or breaking change warnings
        _, logs = run_dbt_and_capture(
            ["run", "--models", "state:modified.contract", "--state", "./state"]
        )
        # Verify no breaking change warning/error
        assert "breaking change" not in logs.lower()
        assert "ContractBreakingChangeError" not in logs

    def test_varchar_case_sensitivity_not_breaking(self, project):
        # Start with VARCHAR(5) - uppercase
        write_file(varchar_size_uppercase_contract_schema_yml, "models", "schema.yml")
        self.run_and_save_state()

        # Change to varchar(20) - lowercase with different size
        # Should NOT be a breaking change (case-insensitive comparison)
        write_file(varchar_size_lowercase_increased_contract_schema_yml, "models", "schema.yml")

        # This should PASS without errors or breaking change warnings
        _, logs = run_dbt_and_capture(
            ["run", "--models", "state:modified.contract", "--state", "./state"]
        )
        # Verify no breaking change warning/error
        assert "breaking change" not in logs.lower()
        assert "ContractBreakingChangeError" not in logs

    def test_numeric_precision_increase_not_breaking(self, project):
        # Start with numeric(10,2)
        write_file(numeric_precision_contract_schema_yml, "models", "schema.yml")
        # Need to modify the table_model.sql to have an amount column instead of name
        modified_table_model = """
select 1 as id, 100.50 as amount
"""
        write_file(modified_table_model, "models", "table_model.sql")
        self.run_and_save_state()

        # Change to numeric(12,4) - should NOT be a breaking change
        write_file(numeric_precision_increased_contract_schema_yml, "models", "schema.yml")

        # This should PASS without errors or breaking change warnings
        _, logs = run_dbt_and_capture(
            ["run", "--models", "state:modified.contract", "--state", "./state"]
        )
        # Verify no breaking change warning/error
        assert "breaking change" not in logs.lower()
        assert "ContractBreakingChangeError" not in logs


class TestUnversionedContractVarcharSizeChange(BaseModifiedState):
    """
    Test that changing varchar size for UNVERSIONED models behaves correctly.
    Unversioned models should also NOT issue warnings for size-only changes.
    This ensures versioned and unversioned models handle size changes consistently.
    """

    MODEL_UNIQUE_ID = "model.test.table_model"

    def test_varchar_size_increase_not_breaking_unversioned(self, project):
        # Start with varchar(5) - no version
        write_file(varchar_size_unversioned_contract_schema_yml, "models", "schema.yml")
        self.run_and_save_state()

        # Change to varchar(20) - should NOT be a breaking change
        write_file(varchar_size_unversioned_increased_contract_schema_yml, "models", "schema.yml")

        # For unversioned models, should also have no breaking change warnings
        _, logs = run_dbt_and_capture(
            ["run", "--models", "state:modified.contract", "--state", "./state"]
        )
        # Verify no breaking change warning (consistent with versioned behavior)
        assert "breaking change" not in logs.lower()
        # Model should still be detected as modified and run successfully
        assert "Completed successfully" in logs


class TestSeedHashMigration(BaseModifiedState):
    """Tests for seamless migration from old (binary-mode) seed hashing to new (text-mode)."""

    @staticmethod
    def _patch_seed_checksum_in_state(project, seed_name, checksum_dict):
        """Replace a seed's checksum in state/manifest.json."""
        import json

        manifest_path = os.path.join(project.project_root, "state", "manifest.json")
        with open(manifest_path, "r") as f:
            manifest = json.load(f)
        for node_id, node in manifest["nodes"].items():
            if node["resource_type"] == "seed" and node["name"] == seed_name:
                node["checksum"] = checksum_dict
                break
        with open(manifest_path, "w") as f:
            json.dump(manifest, f)

    def test_seed_hash_migration_no_false_modified(self, project):
        """Upgrading from old hash to new hash should not falsely flag seed as modified."""
        from dbt.artifacts.resources.base import FileHash

        self.run_and_save_state()

        # Compute the legacy hash for the seed file
        seed_path = os.path.join(project.project_root, "seeds", "seed.csv")
        legacy_hash = FileHash.from_path_legacy(seed_path)

        # Patch the saved state with the legacy hash (simulating an old manifest)
        self._patch_seed_checksum_in_state(
            project, "seed", {"name": legacy_hash.name, "checksum": legacy_hash.checksum}
        )

        # The seed should NOT be reported as modified
        results = run_dbt(
            ["ls", "--resource-type", "seed", "--select", "state:modified", "--state", "./state"]
        )
        assert len(results) == 0

    def test_seed_actual_change_still_detected(self, project):
        """Real seed changes are still detected even with fallback logic."""
        from dbt.artifacts.resources.base import FileHash

        self.run_and_save_state()

        # Patch with legacy hash
        seed_path = os.path.join(project.project_root, "seeds", "seed.csv")
        legacy_hash = FileHash.from_path_legacy(seed_path)
        self._patch_seed_checksum_in_state(
            project, "seed", {"name": legacy_hash.name, "checksum": legacy_hash.checksum}
        )

        # Now actually modify the seed
        changed_seed = seed_csv + "\n" + "3,carl"
        write_file(changed_seed, "seeds", "seed.csv")

        # The seed SHOULD be reported as modified
        results = run_dbt(
            ["ls", "--resource-type", "seed", "--select", "state:modified", "--state", "./state"]
        )
        assert len(results) == 1
        assert results[0] == "test.seed"

    @pytest.mark.skipif(
        os.name != "nt",
        reason="CRLF/LF hash divergence and the Windows-only fallback only apply on Windows",
    )
    def test_seed_crlf_migration(self, project):
        """On Windows: seed with CRLF endings and legacy hash in state should not be flagged.

        Python's text-mode open() normalises \\r\\n → \\n on all platforms (universal newlines),
        so from_path() and from_path_legacy() produce different hashes for CRLF files everywhere.
        But the migration fallback in same_seeds() is guarded by os.name == 'nt', so this test
        only exercises the real end-to-end scenario on Windows CI.
        """
        from dbt.artifacts.resources.base import FileHash

        # Write the seed file with explicit CRLF line endings
        seed_path = os.path.join(project.project_root, "seeds", "seed.csv")
        with open(seed_path, "wb") as f:
            f.write(b"id,name\r\n1,Alice\r\n2,Bob\r\n")

        run_dbt(["seed"])
        run_dbt(["run"])
        self.copy_state()

        # Compute legacy hash (preserves \r\n) and patch into state
        legacy_hash = FileHash.from_path_legacy(seed_path)
        self._patch_seed_checksum_in_state(
            project, "seed", {"name": legacy_hash.name, "checksum": legacy_hash.checksum}
        )

        # Seed should NOT be reported as modified — fallback recognises the legacy hash
        results = run_dbt(
            ["ls", "--resource-type", "seed", "--select", "state:modified", "--state", "./state"]
        )
        assert len(results) == 0


def _layered_grants_config(layer_2_select_key):
    return {
        "models": {
            "test": {
                "layer_1": {
                    "+grants": {"select": ["role_a"]},
                    "layer_2": {
                        "+grants": {layer_2_select_key: ["role_b"]},
                        "layer_3": {
                            "+grants": {"+select": "role_c"},
                        },
                    },
                },
            }
        }
    }


class TestModifiedInheritedGrants(BaseModifiedState):
    @pytest.fixture(scope="class")
    def models(self):
        return {"layer_1": {"layer_2": {"layer_3": {"my_model.sql": "select 1 as id"}}}}

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return _layered_grants_config("select")

    def test_changed_inherited_grants(self, project):
        manifest = run_dbt(["parse"])
        model = manifest.nodes["model.test.my_model"]
        assert model.config.grants == {"select": ["role_b", "role_c"]}
        self.copy_state()
        assert run_dbt(["ls", "--select", "state:modified", "--state", "./state"]) == []

        # switch the inherited `select` grant from replacing to additive
        update_config_file(
            _layered_grants_config("+select"), project.project_root, "dbt_project.yml"
        )
        manifest = run_dbt(["parse"])
        model = manifest.nodes["model.test.my_model"]
        assert model.config.grants == {"select": ["role_a", "role_b", "role_c"]}

        results = run_dbt(["ls", "--select", "state:modified", "--state", "./state"])
        assert results == ["test.layer_1.layer_2.layer_3.my_model"]


class TestModifiedInheritedGrantsLegacyUnrenderedValues(TestModifiedInheritedGrants):
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            **_layered_grants_config("select"),
            "flags": {"state_modified_compare_more_unrendered_values": False},
        }


def _extend_then_child_grants(layer_3_select_key):
    return {
        "models": {
            "test": {
                "layer_1": {
                    "+grants": {"+select": ["role_b"]},
                    "layer_2": {
                        "+grants": {layer_3_select_key: ["role_c"]},
                    },
                },
            }
        }
    }


class TestModifiedChildGrantsAfterAdditiveParent(BaseModifiedState):
    @pytest.fixture(scope="class")
    def models(self):
        return {"layer_1": {"layer_2": {"my_model.sql": "select 1 as id"}}}

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return _extend_then_child_grants("+select")

    def test_child_grants_switch_to_replacing(self, project):
        manifest = run_dbt(["parse"])
        assert manifest.nodes["model.test.my_model"].config.grants == {
            "select": ["role_b", "role_c"]
        }
        self.copy_state()

        # the child level stops extending the parent's `+select` grant
        update_config_file(
            _extend_then_child_grants("select"), project.project_root, "dbt_project.yml"
        )
        manifest = run_dbt(["parse"])
        assert manifest.nodes["model.test.my_model"].config.grants == {"select": ["role_c"]}

        results = run_dbt(["ls", "--select", "state:modified", "--state", "./state"])
        assert results == ["test.layer_1.layer_2.my_model"]


inline_grants_static_model_sql = """
{{ config(grants={'+select': ['role_b']}) }}
select 1 as id
"""

# the `set` block keeps the static parser from handling this model
inline_grants_jinja_model_sql = """
{{ config(grants={'+select': ['role_b']}) }}
{% set n = 1 %}
select {{ n }} as id
"""

yaml_grants_schema_yml = """
models:
  - name: yaml_model
    config:
      grants:
        +select: ["role_b"]
"""


def _project_grants_config(role):
    return {"models": {"test": {"+grants": {"select": [role]}}}}


class TestModifiedProjectGrantsUnderModelGrants(BaseModifiedState):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "static_model.sql": inline_grants_static_model_sql,
            "jinja_model.sql": inline_grants_jinja_model_sql,
            "yaml_model.sql": "select 1 as id",
            "schema.yml": yaml_grants_schema_yml,
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return _project_grants_config("role_a")

    def test_changed_project_grants(self, project):
        manifest = run_dbt(["parse"])
        for name in ["static_model", "jinja_model", "yaml_model"]:
            assert manifest.nodes[f"model.test.{name}"].config.grants == {
                "select": ["role_a", "role_b"]
            }
        self.copy_state()
        assert run_dbt(["ls", "--select", "state:modified", "--state", "./state"]) == []

        # state from another dbt version goes through the manifest upgrade
        with open("state/manifest.json") as f:
            state_manifest = json.load(f)
        state_manifest["metadata"]["dbt_version"] = "1.10.0"
        with open("state/manifest.json", "w") as f:
            json.dump(state_manifest, f)
        assert run_dbt(["ls", "--select", "state:modified", "--state", "./state"]) == []

        update_config_file(
            _project_grants_config("role_x"), project.project_root, "dbt_project.yml"
        )
        results = run_dbt(["ls", "--select", "state:modified", "--state", "./state"])
        assert sorted(results) == ["test.jinja_model", "test.static_model", "test.yaml_model"]
