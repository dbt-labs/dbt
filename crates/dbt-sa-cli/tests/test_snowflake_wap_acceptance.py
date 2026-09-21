"""Offline checks for the live acceptance runner; no warehouse calls are made."""

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import unittest
from unittest import mock
import uuid

from snowflake_wap_acceptance import (
    Acceptance,
    DOWNSTREAM_ID,
    MODEL_ID,
    PROJECT,
    CREATED_CANDIDATE,
    STAGED_CANDIDATE,
    TRANSFORMATION_ERROR,
    created_candidates,
    fixture_audit_ids,
    relation_components,
    staged_candidates,
)

AUDIT_NAMES = {
    f"test.{PROJECT}.nonnegative": "nonnegative",
    f"test.{PROJECT}.not_null_orders_id.abc": "not_null_orders_id",
    f"test.{PROJECT}.unique_orders_id.def": "unique_orders_id",
}


def manifest():
    return {"nodes": {
        unique_id: {"resource_type": "test", "name": name, "depends_on": {"nodes": [MODEL_ID]}}
        for unique_id, name in AUDIT_NAMES.items()
    }}


class AcceptanceEvidenceTests(unittest.TestCase):
    def verify_results(self, artifact, *, verdict):
        return Acceptance.verify_results(artifact, verdict=verdict, expected_audit_ids=set(AUDIT_NAMES))

    def verify_audits(self, artifact, *, verdict):
        return Acceptance.verify_audits(artifact, verdict=verdict, expected_audit_ids=set(AUDIT_NAMES))

    def artifact(self, verdict):
        skipped_audits = verdict in ("transform_error", "lifecycle_error", "collision_error")
        messages = {
            "lifecycle_error": "WAP cannot clone transient public table into a permanent working table",
            "collision_error": "WAP working relation already exists; it will not be overwritten",
        }
        return {"metadata": {"invocation_id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"}, "results": [
            {"unique_id": MODEL_ID,
             "status": {"pass": "success", "fail": "skipped", "warn": "error",
                        "transform_error": "error", "silenced_warn": "error",
                        "unique_fail": "skipped", "lifecycle_error": "error",
                        "collision_error": "error", "null_audit": "skipped"}[verdict],
             "message": messages.get(verdict, f"Numeric value '{TRANSFORMATION_ERROR}' is not recognized")},
            {"unique_id": DOWNSTREAM_ID,
             "status": "success" if verdict == "pass" else "skipped"},
            {"unique_id": f"test.{PROJECT}.nonnegative",
             "status": "skipped" if skipped_audits else (
                 "pass" if verdict in ("silenced_warn", "unique_fail") else (
                     "error" if verdict == "null_audit" else verdict
                 )
             ),
             "message": "WAP audit failures must be a non-NULL, nonnegative integer",
             "failures": 1 if verdict in ("fail", "warn", "silenced_warn") else 0},
            {"unique_id": f"test.{PROJECT}.not_null_orders_id.abc",
             "status": "skipped" if skipped_audits else "pass",
             "failures": 0},
            {"unique_id": f"test.{PROJECT}.unique_orders_id.def",
             "status": "skipped" if skipped_audits else (
                 "fail" if verdict == "unique_fail" else "pass"
             ), "failures": 1 if verdict == "unique_fail" else 0},
        ]}

    def test_expected_verdicts(self):
        for verdict in (
            "pass", "fail", "warn", "transform_error", "silenced_warn", "unique_fail",
            "lifecycle_error", "collision_error", "null_audit",
        ):
            with self.subTest(verdict=verdict):
                audits = self.verify_results(self.artifact(verdict), verdict=verdict)
                self.assertEqual(len(audits), 3)

    def test_silenced_warning_requires_evidence_of_failing_rows(self):
        for failures in (None, 0):
            with self.subTest(failures=failures):
                artifact = self.artifact("silenced_warn")
                artifact["results"][2]["failures"] = failures
                with self.assertRaisesRegex(RuntimeError, "failing row behind the silenced"):
                    self.verify_results(artifact, verdict="silenced_warn")

    def test_silenced_warning_cannot_publish_despite_passed_audits(self):
        artifact = self.artifact("silenced_warn")
        artifact["results"][0]["status"] = "success"
        with self.assertRaisesRegex(RuntimeError, "Unexpected model result"):
            self.verify_results(artifact, verdict="silenced_warn")

    def test_transformation_failure_requires_the_execution_error_marker(self):
        artifact = self.artifact("transform_error")
        artifact["results"][0]["message"] = "Missing permission during preflight"
        with self.assertRaisesRegex(RuntimeError, "deliberate SQL transformation error"):
            self.verify_results(artifact, verdict="transform_error")

    def test_transformation_failure_cannot_report_a_passed_audit(self):
        artifact = self.artifact("transform_error")
        artifact["results"][3]["status"] = "pass"
        with self.assertRaisesRegex(RuntimeError, "Unexpected audit result"):
            self.verify_results(artifact, verdict="transform_error")

    def test_candidate_collision_requires_the_specific_preflight_rejection(self):
        artifact = self.artifact("collision_error")
        artifact["results"][0]["message"] = "Missing permission during preflight"
        with self.assertRaisesRegex(RuntimeError, "candidate collision preflight rejection"):
            self.verify_results(artifact, verdict="collision_error")

    def test_candidate_collision_cannot_report_a_passed_audit(self):
        artifact = self.artifact("collision_error")
        artifact["results"][3]["status"] = "pass"
        with self.assertRaisesRegex(RuntimeError, "Unexpected audit result"):
            self.verify_results(artifact, verdict="collision_error")

    def test_null_audit_requires_the_result_validation_error(self):
        artifact = self.artifact("null_audit")
        artifact["results"][2]["message"] = "Missing permission during test execution"
        with self.assertRaisesRegex(RuntimeError, "NULL audit result validation error"):
            self.verify_results(artifact, verdict="null_audit")

    def test_null_audit_cannot_report_a_passed_audit(self):
        artifact = self.artifact("null_audit")
        artifact["results"][2]["status"] = "pass"
        with self.assertRaisesRegex(RuntimeError, "Unexpected audit result"):
            self.verify_results(artifact, verdict="null_audit")

    def test_missing_previously_passed_audit_is_rejected(self):
        artifact = self.artifact("pass")
        artifact["results"].pop()
        with self.assertRaisesRegex(RuntimeError, "all three audits"):
            self.verify_results(artifact, verdict="pass")

    def test_duplicate_canonical_result_is_rejected(self):
        artifact = self.artifact("pass")
        artifact["results"].append(dict(artifact["results"][0]))
        with self.assertRaisesRegex(RuntimeError, "one canonical model result"):
            self.verify_results(artifact, verdict="pass")

    def test_duplicate_audit_cannot_hide_a_failure(self):
        artifact = self.artifact("fail")
        duplicate = dict(artifact["results"][2], status="pass")
        artifact["results"].append(duplicate)
        with self.assertRaisesRegex(RuntimeError, "one result per audit"):
            self.verify_audits(artifact, verdict="pass")

    def test_standalone_tests_validate_only_audit_results(self):
        artifact = self.artifact("pass")
        artifact["results"] = artifact["results"][2:]
        self.assertEqual(len(self.verify_audits(artifact, verdict="pass")), 3)
        artifact["results"][0]["status"] = "fail"
        with self.assertRaisesRegex(RuntimeError, "Unexpected audit result"):
            self.verify_audits(artifact, verdict="pass")

    def test_unrelated_audit_cannot_replace_a_required_generic_test(self):
        artifact = self.artifact("pass")
        artifact["results"][3]["unique_id"] = "test.other_project.unrelated"
        with self.assertRaisesRegex(RuntimeError, "all three audits from the manifest"):
            self.verify_results(artifact, verdict="pass")

    def test_retry_must_fail_the_previously_passed_uniqueness_audit(self):
        artifact = self.artifact("unique_fail")
        artifact["results"][4]["status"] = "pass"
        with self.assertRaisesRegex(RuntimeError, "Unexpected audit result"):
            self.verify_results(artifact, verdict="unique_fail")

    def test_manifest_defines_exact_fixture_audits(self):
        self.assertEqual(fixture_audit_ids(manifest()), set(AUDIT_NAMES))
        unexpected = manifest()
        unexpected["nodes"][f"test.{PROJECT}.unique_orders_id.def"]["name"] = "unrelated"
        with self.assertRaisesRegex(RuntimeError, "exactly the three expected"):
            fixture_audit_ids(unexpected)

    def test_collision_notice_does_not_establish_candidate_ownership(self):
        candidate = "__DBT_WAP_" + "A" * 32 + "_" + "B" * 32
        self.assertEqual(STAGED_CANDIDATE.findall(
            f"WAP working table retained if created: DB.SCHEMA.{candidate}"
        ), [])
        self.assertEqual(STAGED_CANDIDATE.findall(
            f'WAP: building "DB"."SCHEMA"."PUBLIC" in working table "DB"."SCHEMA"."{candidate}"'
        ), [candidate])
        self.assertEqual(CREATED_CANDIDATE.findall(
            f'WAP: building "DB"."SCHEMA"."PUBLIC" in working table "DB"."SCHEMA"."{candidate}"'
        ), [])


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
        created = (
            f'WAP: created working table "Db""Name"."schema.with.dot"."{self.candidate}" '
            'for "Db""Name"."schema.with.dot"."ORDERS"'
        )
        self.assertEqual(created_candidates(created, "ORDERS"), {self.candidate})

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

    def test_created_relation_in_another_schema_cannot_establish_ownership(self):
        output = (
            f'WAP: created working table "DB"."OTHER"."{self.candidate}" '
            'for "DB"."PUBLIC"."ORDERS"'
        )
        with self.assertRaisesRegex(RuntimeError, "share the public database and schema"):
            created_candidates(output, "ORDERS")


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
        invocation_uuids = mock.patch("snowflake_wap_acceptance.uuid.uuid4", return_value=uuid.UUID(self.invocation))
        self.invocation_uuids = invocation_uuids.start()
        self.addCleanup(invocation_uuids.stop)
        self.stdout = self.stage_message() + "\n" + self.created_message()
        self.returncode = 0
        self.write_results = True

    def stage_message(self, database="DB", schema="PUBLIC"):
        return (
            f'WAP: building "DB"."PUBLIC"."{self.public_names[0]}" in working table '
            f'"{database}"."{schema}"."{self.candidate}"'
        )

    def created_message(self, database="DB", schema="PUBLIC"):
        return (
            f'WAP: created working table "{database}"."{schema}"."{self.candidate}" '
            f'for "DB"."PUBLIC"."{self.public_names[0]}"'
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
        self.assertEqual(run.call_args.kwargs["env"], {
            "DBT_QUIET": "false", "DBT_USE_COLORS": "false",
            "DBT_LOG_FORMAT": "text", "DBT_LOG_LEVEL": "info",
            "DBT_WRITE_JSON": "true", "DBT_WRITE_METADATA": "false",
            "DBT_WARN_ERROR": "false", "DBT_WARN_ERROR_OPTIONS": "{}",
            "DBT_STORE_FAILURES": "false",
        })
        self.assertEqual(run.call_args.kwargs["cwd"], self.fixture.project)
        self.assertFalse(run.call_args.kwargs["check"])

    def test_inherited_output_and_warning_settings_cannot_change_scenarios(self):
        inherited = {
            "DBT_QUIET": "true", "DBT_USE_COLORS": "true",
            "DBT_LOG_FORMAT": "json", "DBT_LOG_LEVEL": "off",
            "DBT_WRITE_JSON": "false", "DBT_WRITE_METADATA": "true",
            "DBT_WARN_ERROR": "true", "DBT_WARN_ERROR_OPTIONS": '{"error": ["LogTestResult"]}',
            "DBT_STORE_FAILURES": "true",
            "DBT_PROFILES_DIR": "/preserved/profiles",
            "SNOWFLAKE_PASSWORD": "offline-secret",
        }
        with (
            mock.patch.dict("snowflake_wap_acceptance.os.environ", inherited),
            mock.patch("snowflake_wap_acceptance.subprocess.run", side_effect=self.completed) as run,
        ):
            self.fixture.invoke(["build"], results=True, staged=True)
        environment = run.call_args.kwargs["env"]
        self.assertEqual(environment, {
            "DBT_QUIET": "false", "DBT_USE_COLORS": "false",
            "DBT_LOG_FORMAT": "text", "DBT_LOG_LEVEL": "info",
            "DBT_WRITE_JSON": "true", "DBT_WRITE_METADATA": "false",
            "DBT_WARN_ERROR": "false", "DBT_WARN_ERROR_OPTIONS": "{}",
            "DBT_STORE_FAILURES": "false",
            "DBT_PROFILES_DIR": "/preserved/profiles",
            "SNOWFLAKE_PASSWORD": "offline-secret",
        })

    def test_unexpected_command_failure_still_records_owned_candidate_for_cleanup(self):
        self.returncode = 1
        with mock.patch("snowflake_wap_acceptance.subprocess.run", side_effect=self.completed):
            with self.assertRaisesRegex(RuntimeError, "Unexpected exit 1"):
                self.fixture.invoke(["build"], results=True, staged=True)
        self.assertIn(self.candidate, self.fixture.owned[self.prefix]["identifiers"])

    def test_each_command_gets_fresh_uuid_despite_inherited_invocation_id(self):
        requested = [uuid.UUID(self.invocation), uuid.UUID("cccccccc-cccc-cccc-cccc-cccccccccccc")]
        self.invocation_uuids.side_effect = requested

        def completed(argv, **kwargs):
            self.invocation = argv[argv.index("--invocation-id") + 1]
            self.candidate = f"__DBT_WAP_{uuid.UUID(self.invocation).hex.upper()}_" + "B" * 32
            self.stdout = self.stage_message() + "\n" + self.created_message()
            return self.completed(argv, **kwargs)

        with (
            mock.patch.dict("snowflake_wap_acceptance.os.environ", {"DBT_INVOCATION_ID": "0"}),
            mock.patch("snowflake_wap_acceptance.subprocess.run", side_effect=completed) as run,
        ):
            _, first, _ = self.fixture.invoke(["build"], results=True, staged=True)
            _, second, _ = self.fixture.invoke(["retry"], results=True, staged=True)
        self.assertTrue(first.isdisjoint(second))
        for call, expected in zip(run.call_args_list, requested):
            argv = call.args[0]
            self.assertEqual(argv[argv.index("--invocation-id") + 1], str(expected))

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
        for results in (True, False):
            with self.subTest(results=results):
                with mock.patch("snowflake_wap_acceptance.subprocess.run", side_effect=self.completed):
                    with self.assertRaisesRegex(RuntimeError, "does not belong to this invocation"):
                        self.fixture.invoke(["build"], results=results, staged=True)
                self.assertEqual(self.fixture.owned[self.prefix]["identifiers"], self.public_names)
                inventory = json.loads((self.fixture.work / "owned_objects.json").read_text())
                self.assertEqual(inventory[0]["identifiers"], self.public_names)
                with mock.patch.object(self.fixture, "invoke") as invoke:
                    self.fixture.cleanup()
                cleanup_args = json.loads(invoke.call_args.args[0][3])
                self.assertEqual(cleanup_args["identifiers"], self.public_names)

    def test_missing_results_cannot_reuse_previous_artifacts(self):
        self.write_results = False
        self.stdout = self.stage_message()
        self.fixture.artifacts.mkdir()
        (self.fixture.artifacts / "run_results.json").write_text('{"stale": true}')
        with mock.patch("snowflake_wap_acceptance.subprocess.run", side_effect=self.completed):
            with self.assertRaisesRegex(RuntimeError, "Missing run_results.json"):
                self.fixture.invoke(["build"], results=True, staged=True)
        self.assertEqual(self.fixture.owned[self.prefix]["identifiers"], self.public_names)
        inventory = json.loads((self.fixture.work / "owned_objects.json").read_text())
        self.assertEqual(inventory[0]["identifiers"], self.public_names)

    def test_confirmed_creation_remains_owned_when_results_are_missing(self):
        self.write_results = False
        with mock.patch("snowflake_wap_acceptance.subprocess.run", side_effect=self.completed):
            with self.assertRaisesRegex(RuntimeError, "Missing run_results.json"):
                self.fixture.invoke(["build"], results=True, staged=True)
        self.assertIn(self.candidate, self.fixture.owned[self.prefix]["identifiers"])

    def test_failed_create_claim_never_enters_cleanup_inventory(self):
        self.returncode = 1
        self.stdout = self.stage_message() + "\nWAP working table creation failed: Object already exists"
        with mock.patch("snowflake_wap_acceptance.subprocess.run", side_effect=self.completed):
            with self.assertRaisesRegex(RuntimeError, "lacks confirmed creation"):
                self.fixture.invoke(["build"], success=False, results=True, staged=True)
        with mock.patch.object(self.fixture, "invoke") as invoke:
            self.fixture.cleanup()
        self.assertEqual(self.fixture.owned[self.prefix]["identifiers"], self.public_names)
        self.assertEqual(json.loads(invoke.call_args.args[0][3])["identifiers"], self.public_names)

    def test_creation_without_staging_evidence_is_rejected_before_ownership(self):
        self.stdout = self.created_message()
        with mock.patch("snowflake_wap_acceptance.subprocess.run", side_effect=self.completed):
            with self.assertRaisesRegex(RuntimeError, "no staging event"):
                self.fixture.invoke(["build"], results=True, staged=True)
        self.assertEqual(self.fixture.owned[self.prefix]["identifiers"], self.public_names)

    def test_collision_notice_never_enters_cleanup_inventory(self):
        self.returncode = 1
        self.stdout = f"WAP working table retained if created: DB.PUBLIC.{self.candidate}"
        with mock.patch("snowflake_wap_acceptance.subprocess.run", side_effect=self.completed) as run:
            _, candidates, _ = self.fixture.invoke(
                ["build"], success=False, results=True, staged=False,
                invocation_id=uuid.UUID(self.invocation),
            )
        self.invocation_uuids.assert_not_called()
        argv = run.call_args.args[0]
        self.assertEqual(argv[argv.index("--invocation-id") + 1], self.invocation)
        self.assertEqual(candidates, set())
        self.assertCountEqual(self.fixture.owned[self.prefix]["identifiers"], self.public_names)
        inventory = json.loads((self.fixture.work / "owned_objects.json").read_text())
        self.assertEqual(inventory[0]["identifiers"], self.public_names)
        with mock.patch.object(self.fixture, "invoke") as invoke:
            self.fixture.cleanup()
        self.assertEqual(json.loads(invoke.call_args.args[0][3])["identifiers"], self.public_names)

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

    def test_begin_resets_failed_candidate_retention_to_the_default(self):
        self.fixture.variables["wap_retain_failed"] = True
        with (
            mock.patch.object(self.fixture, "check"),
            mock.patch.object(self.fixture, "operation"),
        ):
            self.fixture.begin("permanent", "default_retention", sentinel=True)
        self.assertIsNone(self.fixture.variables["wap_retain_failed"])

    def test_failed_candidate_cleanup_requires_exact_removal_and_checks_absence(self):
        self.returncode = 1
        self.stdout += f'\nWAP: removed failed working table "DB"."PUBLIC"."{self.candidate}"'
        with mock.patch("snowflake_wap_acceptance.subprocess.run", side_effect=self.completed):
            _, candidates, capture = self.fixture.invoke(["build"], success=False, results=True, staged=True)
        with mock.patch.object(self.fixture, "check") as check:
            self.fixture.verify_failed_candidates_removed(candidates, capture)
        check.assert_called_once_with(self.candidate, present=False)
        # Removal does not replace the creation evidence in the exact inventory.
        self.assertIn(self.candidate, self.fixture.owned[self.prefix]["identifiers"])

    def test_failed_candidate_cleanup_rejects_missing_or_foreign_removal_event(self):
        capture = self.fixture.work / "cleanup_evidence"
        capture.mkdir()
        (capture / "stderr.log").write_text("")
        for removal in (
            "",
            f'WAP: removed failed working table "DB"."OTHER_SCHEMA"."{self.candidate}"',
        ):
            with self.subTest(removal=removal):
                (capture / "stdout.log").write_text(self.stdout + "\n" + removal)
                with mock.patch.object(self.fixture, "check") as check:
                    with self.assertRaisesRegex(RuntimeError, "exact failed candidate removal events"):
                        self.fixture.verify_failed_candidates_removed({self.candidate}, capture)
                check.assert_not_called()

    def test_failed_candidate_removal_without_creation_does_not_prove_cleanup(self):
        capture = self.fixture.work / "cleanup_without_creation"
        capture.mkdir()
        (capture / "stderr.log").write_text("")
        (capture / "stdout.log").write_text(
            f'WAP: removed failed working table "DB"."PUBLIC"."{self.candidate}"'
        )
        with self.assertRaisesRegex(RuntimeError, "confirmed candidate creation"):
            self.fixture.verify_failed_candidates_removed({self.candidate}, capture)

    def test_live_assertions_verify_lifecycle_for_public_and_candidates(self):
        self.fixture.variables["transient"] = True
        with mock.patch.object(self.fixture, "operation") as operation:
            self.fixture.check(self.public_names[0], ids=[999])
            self.fixture.check(self.candidate, ids=[-1, 2])
            self.fixture.check(self.public_names[1], ids=[777])
        self.assertEqual(
            [call.kwargs["expected_transient"] for call in operation.call_args_list],
            [True, True, None],
        )

    def test_transformation_error_build_disables_static_analysis(self):
        with mock.patch.object(self.fixture, "invoke") as invoke:
            self.fixture.build(success=False, static_analysis="off")
        invoke.assert_called_once_with(
            ["build", "--select", "orders+", "--threads", "4", "--static-analysis", "off"],
            success=False, results=True, staged=True,
        )

    def test_silenced_warning_build_uses_real_warning_options(self):
        with mock.patch.object(self.fixture, "invoke") as invoke:
            self.fixture.build(success=False, silence_audit_warnings=True)
        invoke.assert_called_once_with(
            ["build", "--select", "orders+", "--threads", "4", "--warn-error-options",
             '{"silence": ["LogTestResult"]}'],
            success=False, results=True, staged=True,
        )

    def test_live_scenarios_check_failed_transform_clone_and_self_read_publication(self):
        artifact = AcceptanceEvidenceTests().artifact
        state = self.fixture.work / "failed_state"
        state.mkdir()
        (state / "manifest.json").write_text(json.dumps(manifest()))
        build_results = iter([
            (artifact("fail"), {"failed_audit"}, state),
            (artifact("null_audit"), {"null_audit"}, state),
            (artifact("transform_error"), {"failed_transform"}, state),
            (artifact("warn"), {"warning"}, state),
            (artifact("silenced_warn"), {"silenced_warning"}, state),
            (artifact("pass"), {"self_read"}, state),
            (artifact("pass"), {"first_publish"}, state),
            (artifact("fail"), {"first_failure"}, state),
        ])
        standalone = artifact("pass")
        standalone["results"] = standalone["results"][2:]
        invoke_results = iter([
            (artifact("collision_error"), set(), state),
            (standalone, set(), state),
            (artifact("unique_fail"), {"duplicate_retry"}, state),
            (artifact("pass"), {"retry"}, state),
        ])

        def invoke_result(command, **kwargs):
            if command[0] == "build":
                self.assertEqual(self.fixture.variables["audit_value"], 3)
                self.assertIsNone(self.fixture.variables["wap_retain_failed"])
            elif command[0] == "test":
                self.assertIs(self.fixture.variables["wap_retain_failed"], True)
                self.assertEqual(check.call_args_list[-3:], [
                    mock.call(self.fixture.public(), ids=[999]),
                    mock.call(self.fixture.public("DOWNSTREAM"), ids=[777]),
                    mock.call("failed_audit", ids=[-1, 2]),
                ])
            return next(invoke_results)

        def build_result(**kwargs):
            if not kwargs["success"]:
                self.assertIs(self.fixture.variables["wap_retain_failed"], True)
            return next(build_results)

        with (
            mock.patch.object(self.fixture, "build", side_effect=build_result) as build,
            mock.patch.object(self.fixture, "invoke", side_effect=invoke_result) as invoke,
            mock.patch.object(self.fixture, "operation"),
            mock.patch.object(self.fixture, "check") as check,
            mock.patch.object(self.fixture, "verify_published") as published,
            mock.patch.object(self.fixture, "run_failed_candidate_cleanup") as cleanup,
            mock.patch.object(self.fixture, "run_hooks_and_header") as hooks_and_header,
            mock.patch("builtins.print"),
        ):
            self.fixture.run_kind("permanent")
        self.assertEqual(invoke.call_args_list[0], mock.call(
            ["build", "--select", "orders+", "--threads", "4"],
            success=False, results=True, staged=False, invocation_id=uuid.UUID(self.invocation),
        ))
        check.assert_any_call("null_audit", ids=[1, 2])
        check.assert_any_call("failed_transform", ids=[999])
        check.assert_any_call("silenced_warning", ids=[-1, 2])
        check.assert_any_call("duplicate_retry", ids=[2, 2])
        published.assert_any_call([1000], {"self_read"})
        build.assert_any_call(success=True, static_analysis="off")
        build.assert_any_call(success=False, silence_audit_warnings=True)
        cleanup.assert_called_once_with("permanent", set(AUDIT_NAMES))
        hooks_and_header.assert_called_once_with("permanent", set(AUDIT_NAMES))

    def test_live_hooks_and_header_check_mutations_audits_and_session_state(self):
        build_scenarios = []
        published_scenarios = []

        def build_result(**kwargs):
            variables = dict(self.fixture.variables)
            build_scenarios.append((variables, kwargs))
            verdict = "fail" if variables["post_hook_id"] == -1 else "pass"
            return AcceptanceEvidenceTests().artifact(verdict), {self.candidate}, self.fixture.work

        def published_result(ids, candidates):
            published_scenarios.append((ids, candidates))

        with (
            mock.patch.object(self.fixture, "build", side_effect=build_result),
            mock.patch.object(self.fixture, "operation"),
            mock.patch.object(self.fixture, "check") as check,
            mock.patch.object(self.fixture, "verify_published", side_effect=published_result),
        ):
            self.fixture.run_hooks_and_header("transient", set(AUDIT_NAMES))
        self.assertEqual([kwargs for _, kwargs in build_scenarios], [
            {"success": True}, {"success": False},
            {"success": True, "static_analysis": "off"},
        ])
        passing, failing, header = [variables for variables, _ in build_scenarios]
        self.assertEqual((passing["audit_value"], passing["post_hook_id"]), (1, 3))
        self.assertEqual((failing["audit_value"], failing["post_hook_id"]), (1, -1))
        self.assertIs(failing["wap_retain_failed"], True)
        self.assertIs(header["header_session"], True)
        self.assertIsNone(header["post_hook_id"])
        self.assertIsNone(header["wap_retain_failed"])
        self.assertEqual(published_scenarios, [
            ([2, 3], {self.candidate}), ([41], {self.candidate}),
        ])
        check.assert_any_call(failing["prefix"] + "_ORDERS", ids=[999])
        check.assert_any_call(failing["prefix"] + "_DOWNSTREAM", ids=[777])
        check.assert_any_call(self.candidate, ids=[-1, 2])

    def test_live_default_cleanup_covers_audits_transformations_and_first_builds(self):
        scenarios = []
        retention_settings = []

        def build_result(**kwargs):
            retention_settings.append(self.fixture.variables["wap_retain_failed"])
            transformation_error = self.fixture.variables["transformation_error"]
            self.assertEqual(kwargs, {
                "success": False, "static_analysis": "off" if transformation_error else None,
            })
            scenarios.append((self.fixture.public(), self.fixture.public("DOWNSTREAM")))
            verdict = "transform_error" if transformation_error else "fail"
            return AcceptanceEvidenceTests().artifact(verdict), {self.candidate}, self.fixture.work

        with (
            mock.patch.object(self.fixture, "build", side_effect=build_result),
            mock.patch.object(self.fixture, "operation"),
            mock.patch.object(self.fixture, "check") as check,
            mock.patch.object(self.fixture, "verify_failed_candidates_removed") as removed,
        ):
            self.fixture.run_failed_candidate_cleanup("transient", set(AUDIT_NAMES))
        self.assertEqual(retention_settings, [None, None, False, None])
        self.assertEqual(len(scenarios), 4)
        for orders, downstream in scenarios[:3]:
            check.assert_any_call(orders, ids=[999])
            check.assert_any_call(downstream, ids=[777])
        self.assertEqual(check.call_args_list[-2:], [
            mock.call(scenarios[-1][0], present=False),
            mock.call(scenarios[-1][1], present=False),
        ])
        self.assertEqual(removed.call_args_list, [
            mock.call({self.candidate}, self.fixture.work),
        ] * 4)

    def test_null_audit_scenario_uses_a_direct_literal_and_restores_copied_sql(self):
        audit_path = self.fixture.project / "tests" / "nonnegative.sql"
        original_sql = audit_path.read_text()

        def build_result(**kwargs):
            self.assertEqual(kwargs, {"success": False})
            self.assertEqual(audit_path.read_text(), "{{ config(fail_calc='nullif(count(*), count(*))') }}\n" + original_sql)
            self.assertEqual(self.fixture.variables["audit_value"], 1)
            self.assertIs(self.fixture.variables["wap_retain_failed"], True)
            return AcceptanceEvidenceTests().artifact("null_audit"), {self.candidate}, self.fixture.work

        with (
            mock.patch.object(self.fixture, "begin") as begin,
            mock.patch.object(self.fixture, "build", side_effect=build_result),
            mock.patch.object(self.fixture, "check") as check,
        ):
            self.fixture.run_null_audit_result("transient", set(AUDIT_NAMES))
        begin.assert_called_once_with("transient", "null_audit", sentinel=True)
        self.assertEqual(check.call_args_list, [
            mock.call(self.public_names[0], ids=[999]),
            mock.call(self.public_names[1], ids=[777]),
            mock.call(self.candidate, ids=[1, 2]),
        ])
        self.assertEqual(audit_path.read_text(), original_sql)

    def test_null_audit_scenario_restores_sql_when_build_raises(self):
        audit_path = self.fixture.project / "tests" / "nonnegative.sql"
        original_sql = audit_path.read_text()
        with (
            mock.patch.object(self.fixture, "begin"),
            mock.patch.object(self.fixture, "build", side_effect=RuntimeError("build failed")),
        ):
            with self.assertRaisesRegex(RuntimeError, "build failed"):
                self.fixture.run_null_audit_result("permanent", set(AUDIT_NAMES))
        self.assertEqual(audit_path.read_text(), original_sql)

    def test_lifecycle_transition_rejection_preserves_the_transient_source(self):
        artifact = AcceptanceEvidenceTests().artifact
        state = self.fixture.work / "lifecycle_state"
        state.mkdir()
        (state / "manifest.json").write_text(json.dumps(manifest()))
        with (
            mock.patch.object(self.fixture, "build", side_effect=[
                (artifact("pass"), {"transient_candidate"}, state),
                (artifact("lifecycle_error"), set(), state),
            ]) as build,
            mock.patch.object(self.fixture, "operation"),
            mock.patch.object(self.fixture, "check") as check,
            mock.patch.object(self.fixture, "verify_published") as published,
            mock.patch("builtins.print"),
        ):
            self.fixture.run_lifecycle_transitions()
        published.assert_called_once_with([1, 2], {"transient_candidate"})
        build.assert_any_call(success=False, staged=False)
        check.assert_any_call(self.fixture.public(), ids=[999], transient=True)


if __name__ == "__main__":
    unittest.main()
