"""Offline checks for the live acceptance runner; no warehouse calls are made."""

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import unittest
from unittest import mock

from snowflake_wap_acceptance import (
    Acceptance,
    DOWNSTREAM_ID,
    MODEL_ID,
    PROJECT,
    STAGED_CANDIDATE,
    TRANSFORMATION_ERROR,
    relation_components,
    staged_candidates,
)


class AcceptanceEvidenceTests(unittest.TestCase):
    def artifact(self, verdict):
        return {"results": [
            {"unique_id": MODEL_ID,
             "status": {"pass": "success", "fail": "skipped", "warn": "error",
                        "transform_error": "error"}[verdict],
             "message": f"Numeric value '{TRANSFORMATION_ERROR}' is not recognized"},
            {"unique_id": DOWNSTREAM_ID,
             "status": "success" if verdict == "pass" else "skipped"},
            {"unique_id": f"test.{PROJECT}.nonnegative",
             "status": "skipped" if verdict == "transform_error" else verdict},
            {"unique_id": f"test.{PROJECT}.not_null_orders_id.abc",
             "status": "skipped" if verdict == "transform_error" else "pass"},
            {"unique_id": f"test.{PROJECT}.unique_orders_id.def",
             "status": "skipped" if verdict == "transform_error" else "pass"},
        ]}

    def test_expected_verdicts(self):
        for verdict in ("pass", "fail", "warn", "transform_error"):
            with self.subTest(verdict=verdict):
                audits = Acceptance.verify_results(self.artifact(verdict), verdict=verdict)
                self.assertEqual(len(audits), 3)

    def test_transformation_failure_requires_the_execution_error_marker(self):
        artifact = self.artifact("transform_error")
        artifact["results"][0]["message"] = "Missing permission during preflight"
        with self.assertRaisesRegex(RuntimeError, "deliberate SQL transformation error"):
            Acceptance.verify_results(artifact, verdict="transform_error")

    def test_transformation_failure_cannot_report_a_passed_audit(self):
        artifact = self.artifact("transform_error")
        artifact["results"][3]["status"] = "pass"
        with self.assertRaisesRegex(RuntimeError, "Unexpected audit result"):
            Acceptance.verify_results(artifact, verdict="transform_error")

    def test_missing_previously_passed_audit_is_rejected(self):
        artifact = self.artifact("pass")
        artifact["results"].pop()
        with self.assertRaisesRegex(RuntimeError, "all three audits"):
            Acceptance.verify_results(artifact, verdict="pass")

    def test_duplicate_canonical_result_is_rejected(self):
        artifact = self.artifact("pass")
        artifact["results"].append(dict(artifact["results"][0]))
        with self.assertRaisesRegex(RuntimeError, "one canonical model result"):
            Acceptance.verify_results(artifact, verdict="pass")

    def test_duplicate_audit_cannot_hide_a_failure(self):
        artifact = self.artifact("fail")
        duplicate = dict(artifact["results"][2], status="pass")
        artifact["results"].append(duplicate)
        with self.assertRaisesRegex(RuntimeError, "one result per audit"):
            Acceptance.verify_audits(artifact, verdict="pass")

    def test_standalone_tests_validate_only_audit_results(self):
        artifact = self.artifact("pass")
        artifact["results"] = artifact["results"][2:]
        self.assertEqual(len(Acceptance.verify_audits(artifact, verdict="pass")), 3)
        artifact["results"][0]["status"] = "fail"
        with self.assertRaisesRegex(RuntimeError, "Unexpected audit result"):
            Acceptance.verify_audits(artifact, verdict="pass")

    def test_collision_notice_does_not_establish_candidate_ownership(self):
        candidate = "__DBT_WAP_" + "A" * 32 + "_" + "B" * 32
        self.assertEqual(STAGED_CANDIDATE.findall(
            f"WAP working table retained if created: DB.SCHEMA.{candidate}"
        ), [])
        self.assertEqual(STAGED_CANDIDATE.findall(
            f'WAP: building "DB"."SCHEMA"."PUBLIC" in working table "DB"."SCHEMA"."{candidate}"'
        ), [candidate])


class CandidateLocationTests(unittest.TestCase):
    candidate = "__DBT_WAP_" + "A" * 32 + "_" + "B" * 32

    def test_quoted_components_preserve_dots_and_embedded_quotes(self):
        self.assertEqual(
            relation_components('"Db""Name"."schema.with.dot".orders'),
            ('Db"Name', 'schema.with.dot', 'ORDERS'),
        )
        output = (
            'WAP: building "Db""Name"."schema.with.dot"."ORDERS" in working table '
            f'"Db""Name"."schema.with.dot"."{self.candidate}"'
        )
        self.assertEqual(staged_candidates(output, "ORDERS"), {self.candidate})

    def test_equivalent_unquoted_and_quoted_names_share_a_namespace(self):
        output = (
            'WAP: building db.public.orders in working table '
            f'"DB"."PUBLIC"."{self.candidate}"'
        )
        self.assertEqual(staged_candidates(output, "ORDERS"), {self.candidate})

    def test_unparseable_staging_relation_does_not_establish_ownership(self):
        output = f"WAP: building DB.PUBLIC.ORDERS in working table {self.candidate}"
        with self.assertRaisesRegex(RuntimeError, "Could not verify every staged candidate"):
            staged_candidates(output, "ORDERS")


class AcceptanceInvocationTests(unittest.TestCase):
    def setUp(self):
        # The runner never sees real environment variables, credentials, or a profile.
        environment = mock.patch.dict("snowflake_wap_acceptance.os.environ", {}, clear=True)
        environment.start()
        self.addCleanup(environment.stop)
        args = argparse.Namespace(
            dbt_bin=Path("/unused/dbt"), profiles_dir=Path("/unused/profiles"),
            profile="offline", target="offline",
        )
        with mock.patch("builtins.print"):
            self.fixture = Acceptance(args)
        self.addCleanup(shutil.rmtree, self.fixture.work)
        self.prefix = "DBT_WAP_ACCEPT_OFFLINE"
        self.public_names = [f"{self.prefix}_ORDERS", f"{self.prefix}_DOWNSTREAM"]
        self.fixture.variables = {"prefix": self.prefix}
        self.fixture.owned[self.prefix] = {
            "profile": "offline", "target": "offline",
            "variables": dict(self.fixture.variables), "identifiers": list(self.public_names),
        }
        self.fixture.persist_owned()
        self.candidate = "__DBT_WAP_" + "A" * 32 + "_" + "B" * 32
        self.invocation = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        self.stdout = self.stage_message()
        self.returncode = 0
        self.write_results = True

    def stage_message(self, database="DB", schema="PUBLIC"):
        return (
            f'WAP: building "DB"."PUBLIC"."{self.public_names[0]}" in working table '
            f'"{database}"."{schema}"."{self.candidate}"'
        )

    def completed(self, argv, **kwargs):
        result_path = self.fixture.artifacts / "run_results.json"
        self.assertFalse(result_path.exists(), "A previous artifact was not removed")
        self.fixture.artifacts.mkdir(parents=True, exist_ok=True)
        if self.write_results:
            result_path.write_text(json.dumps({
                "metadata": {"invocation_id": self.invocation}, "results": [],
            }))
        (self.fixture.artifacts / "manifest.json").write_text('{"nodes": {}}')
        return subprocess.CompletedProcess(
            argv, self.returncode, stdout=self.stdout, stderr="offline stderr",
        )

    def test_invoke_replaces_stale_results_captures_evidence_and_records_exact_ownership(self):
        self.fixture.artifacts.mkdir()
        (self.fixture.artifacts / "run_results.json").write_text('{"stale": true}')
        with mock.patch("snowflake_wap_acceptance.subprocess.run", side_effect=self.completed) as run:
            artifact, candidates, capture = self.fixture.invoke(
                ["build"], results=True, staged=True,
            )
        self.assertEqual(candidates, {self.candidate})
        self.assertEqual(artifact["metadata"]["invocation_id"], self.invocation)
        self.assertEqual(json.loads((capture / "run_results.json").read_text()), artifact)
        self.assertTrue((capture / "manifest.json").is_file())
        self.assertEqual((capture / "stdout.log").read_text(), self.stdout)
        self.assertEqual((capture / "stderr.log").read_text(), "offline stderr")
        inventory = json.loads((self.fixture.work / "owned_objects.json").read_text())
        self.assertEqual(set(inventory[0]["identifiers"]), set(self.public_names) | {self.candidate})
        self.assertEqual(run.call_args.kwargs["env"], {"DBT_QUIET": "false", "DBT_USE_COLORS": "false"})
        self.assertEqual(run.call_args.kwargs["cwd"], self.fixture.project)
        self.assertFalse(run.call_args.kwargs["check"])

    def test_unexpected_command_failure_still_records_owned_candidate_for_cleanup(self):
        self.returncode = 1
        with mock.patch("snowflake_wap_acceptance.subprocess.run", side_effect=self.completed):
            with self.assertRaisesRegex(RuntimeError, "Unexpected exit 1"):
                self.fixture.invoke(["build"], results=True, staged=True)
        self.assertIn(self.candidate, self.fixture.owned[self.prefix]["identifiers"])

    def test_candidate_in_another_database_or_scratch_schema_is_rejected_before_ownership(self):
        for database, schema in [("OTHER_DB", "PUBLIC"), ("DB", "DBT_WAP")]:
            with self.subTest(database=database, schema=schema):
                self.stdout = self.stage_message(database, schema)
                with mock.patch("snowflake_wap_acceptance.subprocess.run", side_effect=self.completed):
                    with self.assertRaisesRegex(RuntimeError, "share the public database and schema"):
                        self.fixture.invoke(["build"], results=True, staged=True)
                self.assertEqual(self.fixture.owned[self.prefix]["identifiers"], self.public_names)

    def test_candidate_from_another_invocation_is_rejected(self):
        self.invocation = "cccccccc-cccc-cccc-cccc-cccccccccccc"
        with mock.patch("snowflake_wap_acceptance.subprocess.run", side_effect=self.completed):
            with self.assertRaisesRegex(RuntimeError, "does not belong to this invocation"):
                self.fixture.invoke(["build"], results=True, staged=True)

    def test_missing_results_cannot_reuse_previous_artifacts(self):
        self.write_results = False
        self.fixture.artifacts.mkdir()
        (self.fixture.artifacts / "run_results.json").write_text('{"stale": true}')
        with mock.patch("snowflake_wap_acceptance.subprocess.run", side_effect=self.completed):
            with self.assertRaisesRegex(RuntimeError, "Missing run_results.json"):
                self.fixture.invoke(["build"], results=True, staged=True)

    def test_collision_notice_never_enters_cleanup_inventory(self):
        self.returncode = 1
        self.stdout = f"WAP working table retained if created: DB.PUBLIC.{self.candidate}"
        with mock.patch("snowflake_wap_acceptance.subprocess.run", side_effect=self.completed):
            _, candidates, _ = self.fixture.invoke(["build"], success=False, results=True)
        self.assertEqual(candidates, set())
        self.assertCountEqual(self.fixture.owned[self.prefix]["identifiers"], self.public_names)

    def test_cleanup_passes_only_the_exact_owned_identifiers(self):
        identifiers = self.fixture.owned[self.prefix]["identifiers"]
        identifiers.append(self.candidate)
        with mock.patch.object(self.fixture, "invoke") as invoke:
            self.fixture.cleanup()
        command = invoke.call_args.args[0]
        self.assertEqual(command[:3], ["run-operation", "wap_fixture_cleanup", "--args"])
        self.assertEqual(json.loads(command[3]), {"identifiers": identifiers})
        self.assertEqual(invoke.call_count, 1)

    def test_failure_preservation_checks_both_existing_tables(self):
        with mock.patch.object(self.fixture, "check") as check:
            self.fixture.verify_existing_data_preserved()
        self.assertEqual(check.call_args_list, [
            mock.call(self.public_names[0], ids=[999]),
            mock.call(self.public_names[1], ids=[777]),
        ])

    def test_transformation_error_build_disables_static_analysis(self):
        with mock.patch.object(self.fixture, "invoke") as invoke:
            self.fixture.build(success=False, static_analysis="off")
        invoke.assert_called_once_with(
            ["build", "--select", "orders+", "--threads", "4", "--static-analysis", "off"],
            success=False, results=True, staged=True,
        )


if __name__ == "__main__":
    unittest.main()
