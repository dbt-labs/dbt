"""Offline checks for the live acceptance runner's evidence handling."""

import unittest

from snowflake_wap_acceptance import (
    Acceptance,
    DOWNSTREAM_ID,
    MODEL_ID,
    PROJECT,
    STAGED_CANDIDATE,
)


class AcceptanceEvidenceTests(unittest.TestCase):
    def artifact(self, verdict):
        return {"results": [
            {"unique_id": MODEL_ID,
             "status": {"pass": "success", "fail": "skipped", "warn": "error"}[verdict]},
            {"unique_id": DOWNSTREAM_ID,
             "status": "success" if verdict == "pass" else "skipped"},
            {"unique_id": f"test.{PROJECT}.nonnegative", "status": verdict},
            {"unique_id": f"test.{PROJECT}.not_null_orders_id.abc", "status": "pass"},
            {"unique_id": f"test.{PROJECT}.unique_orders_id.def", "status": "pass"},
        ]}

    def test_expected_verdicts(self):
        for verdict in ("pass", "fail", "warn"):
            with self.subTest(verdict=verdict):
                audits = Acceptance.verify_results(self.artifact(verdict), verdict=verdict)
                self.assertEqual(len(audits), 3)

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


if __name__ == "__main__":
    unittest.main()
