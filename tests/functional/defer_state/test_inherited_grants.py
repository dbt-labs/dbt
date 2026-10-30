import os
import shutil

import pytest

from dbt.tests.util import get_manifest, run_dbt, update_config_file


class TestInheritedGrantSelection:
    @pytest.fixture(scope="class", params=[True, False], ids=["raw-configs", "legacy"])
    def compare_unrendered(self, request):
        return request.param

    @pytest.fixture(scope="class", params=["reader_c", ["reader_c"]], ids=["scalar", "list"])
    def child_role(self, request):
        return request.param

    @pytest.fixture(
        scope="class",
        params=[
            ({"select": ["reader_b"]}, {"+select": ["reader_b"]}, True, True),
            ({"select": ["reader_b"]}, {"select": ["reader_z"]}, True, True),
            ({"+select": ["reader_b"]}, {"select": ["reader_b"]}, True, True),
            ({"+select": ["reader_b"]}, {"+select": ["reader_z"]}, False, False),
        ],
        ids=["replace-to-add", "inherited-value", "add-to-replace", "shadowed-addition"],
    )
    def grant_change(self, request):
        return request.param

    @pytest.fixture(scope="class")
    def models(self):
        return {"outer": {"middle": {"inner": {"granted.sql": "select 1 as id"}}}}

    @pytest.fixture(scope="class")
    def project_root(self, tmpdir_factory, compare_unrendered, child_role, grant_change):
        return tmpdir_factory.mktemp("inherited_grants")

    def config(self, middle, additive, child_role):
        return {
            "models": {
                "test": {
                    "outer": {
                        "+grants": {"select": ["reader_a"]},
                        "middle": {
                            "+grants": middle,
                            "inner": {
                                "+grants": {"+select" if additive else "select": child_role}
                            },
                        },
                    }
                }
            }
        }

    @pytest.fixture(scope="class")
    def project_config_update(self, compare_unrendered, child_role, grant_change):
        before, _, additive, _ = grant_change
        return {
            "flags": {"state_modified_compare_more_unrendered_values": compare_unrendered},
            **self.config(before, additive, child_role),
        }

    def test_selection(self, project, compare_unrendered, child_role, grant_change):
        before, after, additive, changed = grant_change
        run_dbt(["parse"])
        initial = get_manifest(project.project_root).nodes["model.test.granted"]
        os.makedirs("state", exist_ok=True)
        shutil.copyfile("target/manifest.json", "state/manifest.json")

        assert run_dbt(["ls", "-s", "state:modified", "--state", "state"]) == []
        update_config_file(self.config(after, additive, child_role), "dbt_project.yml")

        for selector in ["state:modified", "state:modified.configs"]:
            selected = run_dbt(["ls", "-s", selector, "--state", "state"])
            expected = (
                ["test.outer.middle.inner.granted"] if changed and compare_unrendered else []
            )
            assert selected == expected

        current = get_manifest(project.project_root).nodes["model.test.granted"]
        assert (current.config.grants != initial.config.grants) == changed
        expected = [] if changed and compare_unrendered else ["test.outer.middle.inner.granted"]
        assert run_dbt(["ls", "-s", "state:unmodified", "--state", "state"]) == expected


class TestGrantConfigSources:
    @pytest.fixture(scope="class", params=["schema", "inline"])
    def config_source(self, request):
        return request.param

    @pytest.fixture(scope="class", params=[True, False], ids=["add", "replace"])
    def additive(self, request):
        return request.param

    @pytest.fixture(scope="class")
    def project_root(self, tmpdir_factory, config_source, additive):
        return tmpdir_factory.mktemp("grant_sources")

    @pytest.fixture(scope="class")
    def models(self, config_source, additive):
        key = "+select" if additive else "select"
        if config_source == "inline":
            return {"granted.sql": "{{ config(grants={'" + key + "': 'inner'}) }} select 1 as id"}
        return {
            "granted.sql": "select 1 as id",
            "schema.yml": (
                "version: 2\nmodels:\n  - name: granted\n    config:\n"
                f"      grants:\n        '{key}': inner\n"
            ),
        }

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "flags": {"state_modified_compare_more_unrendered_values": True},
            "models": {"test": {"+grants": {"select": ["outer"]}}},
        }

    def test_project_grant_change(self, project, additive):
        run_dbt(["parse"])
        initial = get_manifest(project.project_root).nodes["model.test.granted"]
        os.makedirs("state", exist_ok=True)
        shutil.copyfile("target/manifest.json", "state/manifest.json")
        update_config_file(
            {"models": {"test": {"+grants": {"select": ["changed"]}}}}, "dbt_project.yml"
        )
        expected = ["test.granted"] if additive else []
        assert run_dbt(["ls", "-s", "state:modified.configs", "--state", "state"]) == expected
        current = get_manifest(project.project_root).nodes["model.test.granted"]
        assert (current.config.grants != initial.config.grants) == additive


class TestUnrenderedGrantExpressions:
    @pytest.fixture(scope="class")
    def models(self):
        return {"inner": {"granted.sql": "select 1 as id"}}

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "flags": {"state_modified_compare_more_unrendered_values": True},
            "vars": {"reader": "dev_reader"},
            "models": {
                "test": {
                    "+grants": {"select": "{{ var('reader', 'dev_reader') }}"},
                    "inner": {"+grants": {"+select": ["common_reader"]}},
                }
            },
        }

    def test_rendered_change_is_not_an_authored_change(self, project):
        run_dbt(["parse"])
        initial = get_manifest(project.project_root).nodes["model.test.granted"]
        os.makedirs("state", exist_ok=True)
        shutil.copyfile("target/manifest.json", "state/manifest.json")
        assert (
            run_dbt(
                [
                    "ls",
                    "-s",
                    "state:modified",
                    "--state",
                    "state",
                    "--vars",
                    "{reader: prod_reader}",
                ]
            )
            == []
        )
        current = get_manifest(project.project_root).nodes["model.test.granted"]
        assert current.config.grants != initial.config.grants
        assert current.unrendered_config == initial.unrendered_config
        assert current.unrendered_config["grants"] == {
            "select": ["{{ var('reader', 'dev_reader') }}", "common_reader"]
        }


class TestLegacyGrantManifest:
    @pytest.fixture(scope="class")
    def models(self):
        return {"inner": {"granted.sql": "select 1 as id"}}

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "flags": {"state_modified_compare_more_unrendered_values": False},
            "models": {
                "test": {
                    "+grants": {"select": ["outer"]},
                    "inner": {"+grants": {"+select": "inner"}},
                }
            },
        }

    def test_reselects_once_against_legacy_raw_grants(self, project):
        run_dbt(["parse"])
        initial = get_manifest(project.project_root).nodes["model.test.granted"]
        assert initial.unrendered_config["grants"] == {"+select": "inner"}
        os.makedirs("state", exist_ok=True)
        shutil.copyfile("target/manifest.json", "state/manifest.json")
        update_config_file(
            {"flags": {"state_modified_compare_more_unrendered_values": True}}, "dbt_project.yml"
        )
        assert run_dbt(["ls", "-s", "state:modified", "--state", "state"]) == [
            "test.inner.granted"
        ]
        current = get_manifest(project.project_root).nodes["model.test.granted"]
        assert current.config.grants == initial.config.grants
        shutil.copyfile("target/manifest.json", "state/manifest.json")
        assert run_dbt(["ls", "-s", "state:modified", "--state", "state"]) == []
