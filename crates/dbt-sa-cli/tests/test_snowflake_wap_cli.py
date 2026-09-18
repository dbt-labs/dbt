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
