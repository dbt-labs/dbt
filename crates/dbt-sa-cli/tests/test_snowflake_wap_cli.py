"""Offline CLI integration checks; set DBT_WAP_TEST_BIN to the built dbt binary."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


@unittest.skipUnless(os.environ.get("DBT_WAP_TEST_BIN"), "set DBT_WAP_TEST_BIN to run CLI checks")
class WapCliTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory(prefix="dbt-wap-cli-")
        self.addCleanup(temporary.cleanup)
        self.project = Path(temporary.name)
        (self.project / "models").mkdir()
        (self.project / "dbt_project.yml").write_text(
            "name: wap_cli\nversion: '1.0'\nconfig-version: 2\nprofile: wap_cli\n"
            "models:\n  wap_cli:\n    +materialized: table\n    +wap: true\n"
        )
        # Parse must not contact this deliberately unusable Snowflake target.
        (self.project / "profiles.yml").write_text(
            "wap_cli:\n  target: offline\n  outputs:\n    offline:\n"
            "      type: snowflake\n      account: snowflake.local\n"
            "      user: offline\n      password: unused\n"
            "      database: WAP_DB\n      schema: EXISTING_SCHEMA\n"
            "      warehouse: UNUSED\n      threads: 2\n      connect_timeout: 1\n"
        )
        self.write_model("orders", "{{ config(alias='PUBLIC_ORDERS') }}\nselect 1 as id")
        self.write_model(
            "downstream", "{{ config(wap=false) }}\nselect * from {{ ref('orders') }}"
        )
        (self.project / "models/schema.yml").write_text(
            "version: 2\nmodels:\n  - name: orders\n    columns:\n"
            "      - name: id\n        data_tests: [not_null, unique]\n"
        )

    def write_model(self, name: str, sql: str) -> None:
        (self.project / f"models/{name}.sql").write_text(sql + "\n")

    def parse(self, *, success: bool = True, error: str = "wap") -> dict:
        environment = dict(os.environ)
        environment.update(DBT_USE_COLORS="false", DBT_QUIET="false")
        completed = subprocess.run(
            [
                str(Path(os.environ["DBT_WAP_TEST_BIN"]).resolve()), "parse",
                "--project-dir", str(self.project),
                "--profiles-dir", str(self.project),
                "--target-path", str(self.project / "target"),
                "--no-send-anonymous-usage-stats", "--no-version-check",
            ],
            cwd=self.project, env=environment, text=True, capture_output=True, timeout=60,
        )
        output = completed.stdout + "\n" + completed.stderr
        self.assertEqual(completed.returncode == 0, success, output[-6000:])
        if not success:
            self.assertIn(error, output.lower(), output[-6000:])
            return {}
        return json.loads((self.project / "target/manifest.json").read_text())

    def test_parse_preserves_public_identity_and_ref_dependencies(self) -> None:
        manifest = self.parse()
        orders = manifest["nodes"]["model.wap_cli.orders"]
        downstream = manifest["nodes"]["model.wap_cli.downstream"]
        self.assertTrue(orders["config"]["wap"])
        self.assertFalse(downstream["config"]["wap"])
        self.assertEqual(orders["alias"], "PUBLIC_ORDERS")
        self.assertEqual(orders["database"], "WAP_DB")
        self.assertEqual(orders["schema"], "EXISTING_SCHEMA")
        self.assertIn("PUBLIC_ORDERS", orders["relation_name"])
        self.assertNotIn("__DBT_WAP_", json.dumps(manifest))
        self.assertEqual(downstream["depends_on"]["nodes"], ["model.wap_cli.orders"])
        audits = [node for node in manifest["nodes"].values() if node["resource_type"] == "test"]
        self.assertEqual(len(audits), 2)
        for audit in audits:
            self.assertEqual(audit["attached_node"], "model.wap_cli.orders")

    def test_parse_wap_false_allows_view(self) -> None:
        self.write_model("orders", "{{ config(wap=false, materialized='view') }}\nselect 1 as id")
        manifest = self.parse()
        config = manifest["nodes"]["model.wap_cli.orders"]["config"]
        self.assertFalse(config["wap"])
        self.assertEqual(config["materialized"], "view")

    def test_parse_preserves_inherited_and_inline_hooks_and_header(self) -> None:
        original = self.parse()["nodes"]["model.wap_cli.orders"]
        project_path = self.project / "dbt_project.yml"
        project_path.write_text(project_path.read_text() + (
            "    +pre-hook: ['select 11']\n"
            "    +post-hook: ['select 21']\n"
            "    +sql_header: 'set wap_header = 31;'\n"
        ))
        inherited = self.parse()["nodes"]["model.wap_cli.orders"]["config"]
        self.assertEqual(inherited["sql_header"], "set wap_header = 31;")

        self.write_model("orders", (
            "{{ config(alias='PUBLIC_ORDERS', pre_hook=['select 12'], "
            "post_hook=['delete from {{ this }} where id < 0'], "
            "sql_header='set wap_header = 32;') }}\nselect 1 as id"
        ))
        for _ in range(2):
            manifest = self.parse()
            orders = manifest["nodes"]["model.wap_cli.orders"]
            config = orders["config"]
            self.assertEqual([hook["sql"] for hook in config["pre-hook"]], [
                "select 11", "select 12",
            ])
            self.assertEqual([hook["sql"] for hook in config["post-hook"]], [
                "select 21", "delete from {{ this }} where id < 0",
            ])
            self.assertEqual(config["sql_header"], "set wap_header = 32;")
            for field in ("unique_id", "database", "schema", "alias", "relation_name"):
                self.assertEqual(orders[field], original[field], field)
            self.assertNotIn("__DBT_WAP_", json.dumps(manifest))
            self.assertEqual(
                manifest["nodes"]["model.wap_cli.downstream"]["depends_on"]["nodes"],
                ["model.wap_cli.orders"],
            )

    def test_parse_rejects_unsupported_materializations(self) -> None:
        for materialized in ("view", "incremental"):
            with self.subTest(materialized=materialized):
                self.write_model(
                    "orders", "{{ config(materialized='" + materialized + "') }}\nselect 1 as id"
                )
                self.parse(success=False)

    def test_parse_rejects_invalid_wap_value(self) -> None:
        self.write_model("orders", "{{ config(wap='typo') }}\nselect 1 as id")
        self.parse(success=False, error="expected true, false, or null")

    def test_parse_failed_candidate_retention_inheritance(self) -> None:
        project_path = self.project / "dbt_project.yml"
        project_path.write_text(project_path.read_text() + "    +wap_retain_failed: false\n")
        inherited = self.parse()["nodes"]["model.wap_cli.orders"]["config"]
        self.assertFalse(inherited["wap_retain_failed"])

        properties_path = self.project / "models/schema.yml"
        properties_path.write_text(properties_path.read_text().replace(
            "  - name: orders\n",
            "  - name: orders\n    config:\n      wap_retain_failed: true\n",
        ))
        properties = self.parse()["nodes"]["model.wap_cli.orders"]["config"]
        self.assertTrue(properties["wap_retain_failed"])

        self.write_model(
            "orders", "{{ config(wap_retain_failed=false, alias='PUBLIC_ORDERS') }}\nselect 1 as id"
        )
        overridden = self.parse()["nodes"]["model.wap_cli.orders"]["config"]
        self.assertFalse(overridden["wap_retain_failed"])

    def test_parse_rejects_invalid_failed_candidate_retention_value(self) -> None:
        self.write_model("orders", "{{ config(wap_retain_failed='typo') }}\nselect 1 as id")
        self.parse(success=False, error="expected true, false, or null")

    def test_reparse_updates_retention_without_changing_public_identity(self) -> None:
        original = self.parse()["nodes"]["model.wap_cli.orders"]
        self.assertNotIn("wap_retain_failed", original["config"])
        for retain in (False, True):
            with self.subTest(retain=retain):
                self.write_model(
                    "orders",
                    "{{ config(wap_retain_failed=" + str(retain).lower()
                    + ", alias='PUBLIC_ORDERS') }}\nselect 1 as id",
                )
                updated = self.parse()["nodes"]["model.wap_cli.orders"]
                self.assertIs(updated["config"]["wap_retain_failed"], retain)
                self.assertTrue(updated["config"]["wap"])
                for field in ("unique_id", "database", "schema", "alias", "relation_name"):
                    self.assertEqual(updated[field], original[field], field)

    def test_reparse_updates_wap_without_changing_public_identity(self) -> None:
        original = self.parse()["nodes"]["model.wap_cli.orders"]
        self.write_model(
            "orders", "{{ config(wap=false, alias='PUBLIC_ORDERS') }}\nselect 1 as id"
        )
        disabled = self.parse()["nodes"]["model.wap_cli.orders"]
        self.write_model("orders", "{{ config(alias='PUBLIC_ORDERS') }}\nselect 1 as id")
        enabled = self.parse()["nodes"]["model.wap_cli.orders"]
        self.assertFalse(disabled["config"]["wap"])
        self.assertTrue(enabled["config"]["wap"])
        for node in (disabled, enabled):
            for field in ("unique_id", "database", "schema", "alias", "relation_name"):
                self.assertEqual(node[field], original[field], field)


if __name__ == "__main__":
    unittest.main()
