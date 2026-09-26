#!/usr/bin/env python3
"""Run WAP acceptance checks against an explicitly selected Snowflake target."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
from typing import Any
import uuid


PROJECT = "wap_live_acceptance"
MODEL_ID = f"model.{PROJECT}.orders"
DOWNSTREAM_ID = f"model.{PROJECT}.downstream"
TRANSFORMATION_ERROR = "WAP_TRANSFORM_FAILURE"
CANDIDATE_IDENTIFIER = r"__DBT_WAP_[0-9A-F]{32}_[0-9A-F]{32}"
STAGED_CANDIDATE = re.compile(
    r"WAP: building[^\r\n]*?in working table[^\r\n]*?"
    rf"({CANDIDATE_IDENTIFIER})"
)
CREATED_CANDIDATE = re.compile(
    rf"WAP: created working table[^\r\n]*?({CANDIDATE_IDENTIFIER})"
)
RELATION_COMPONENT = r'(?:"(?:[^"\r\n]|"")*"|[A-Za-z_][A-Za-z0-9_$]*)'
RELATION_NAME = rf"{RELATION_COMPONENT}\.{RELATION_COMPONENT}\.{RELATION_COMPONENT}"
STAGED_RELATIONS = re.compile(
    rf"WAP: building (?P<public>{RELATION_NAME}) in working table "
    rf"(?P<candidate>{RELATION_NAME})"
)
CREATED_RELATIONS = re.compile(
    rf"WAP: created working table (?P<candidate>{RELATION_NAME}) "
    rf"for (?P<public>{RELATION_NAME})"
)
REMOVED_FAILED_RELATIONS = re.compile(
    rf"WAP: removed failed working table (?P<candidate>{RELATION_NAME})"
)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def relation_components(relation: str) -> tuple[str, ...]:
    require(re.fullmatch(RELATION_NAME, relation) is not None, f"Invalid relation: {relation}")
    return tuple(
        part[1:-1].replace('""', '"') if part.startswith('"') else part.upper()
        for part in re.findall(RELATION_COMPONENT, relation)
    )


def candidate_locations(
    output: str, public_identifier: str, identifiers_pattern: re.Pattern[str],
    relations_pattern: re.Pattern[str], event: str,
) -> set[str]:
    identifiers = set(identifiers_pattern.findall(output))
    located = set()
    for match in relations_pattern.finditer(output):
        public = relation_components(match["public"])
        candidate = relation_components(match["candidate"])
        require(public[2] == public_identifier, f"Unexpected public relation: {match['public']}")
        require(
            public[:2] == candidate[:2],
            f"Candidate must share the public database and schema: {match['candidate']}; "
            f"public relation: {match['public']}",
        )
        require(
            re.fullmatch(CANDIDATE_IDENTIFIER, candidate[2]) is not None,
            f"Unexpected candidate identifier: {candidate[2]}",
        )
        located.add(candidate[2])
    require(located == identifiers, f"Could not verify every {event} candidate's database and schema")
    return located


def staged_candidates(output: str, public_identifier: str) -> set[str]:
    return candidate_locations(output, public_identifier, STAGED_CANDIDATE, STAGED_RELATIONS, "staged")


def created_candidates(output: str, public_identifier: str) -> set[str]:
    return candidate_locations(output, public_identifier, CREATED_CANDIDATE, CREATED_RELATIONS, "created")


def fixture_audit_ids(manifest: dict[str, Any]) -> set[str]:
    tests = {
        unique_id: node for unique_id, node in manifest["nodes"].items()
        if node.get("resource_type") == "test"
    }
    expected_names = {"nonnegative", "not_null_orders_id", "unique_orders_id"}
    require(
        len(tests) == 3 and {node["name"] for node in tests.values()} == expected_names,
        "Manifest must contain exactly the three expected fixture audits",
    )
    for unique_id, node in tests.items():
        require(
            unique_id.startswith(f"test.{PROJECT}.")
            and node["depends_on"]["nodes"] == [MODEL_ID]
            and node.get("config", {}).get("enabled", True),
            f"Unexpected fixture audit definition: {unique_id}",
        )
    return set(tests)


class Acceptance:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.work = Path(tempfile.mkdtemp(prefix="dbt-snowflake-wap-"))
        self.project = self.work / "project"
        fixture = Path(__file__).parent / "data" / "snowflake_wap"
        shutil.copytree(fixture, self.project)
        self.artifacts = self.project / "target"
        self.sequence = 0
        self.variables: dict[str, Any] = {}
        self.owned: dict[str, dict[str, Any]] = {}
        self.token = uuid.uuid4().hex.upper()
        print(f"Acceptance logs and artifacts: {self.work}", flush=True)

    def persist_owned(self) -> None:
        # An exact inventory also supports manual cleanup after interruption.
        (self.work / "owned_objects.json").write_text(
            json.dumps(list(self.owned.values()), indent=2) + "\n"
        )

    def invoke(
        self, command: list[str], *, success: bool = True, results: bool = False,
        staged: bool = False, invocation_id: uuid.UUID | None = None,
    ) -> tuple[dict[str, Any], set[str], Path]:
        self.sequence += 1
        capture = self.work / f"{self.sequence:02d}_{command[0]}"
        capture.mkdir()
        result_path = self.artifacts / "run_results.json"
        result_path.unlink(missing_ok=True)
        # Reuse is reserved for the deliberate candidate-collision check.
        invocation_id = invocation_id or uuid.uuid4()
        argv = [
            str(self.args.dbt_bin),
            *command,
            "--project-dir", str(self.project),
            "--profiles-dir", str(self.args.profiles_dir),
            "--profile", self.args.profile,
            "--target", self.args.target,
            "--target-path", str(self.artifacts),
            "--invocation-id", str(invocation_id),
            "--vars", json.dumps(self.variables),
        ]
        environment = dict(os.environ)
        # Ownership evidence requires readable info messages and run_results.
        # Reset warning overrides; individual scenarios supply their own CLI
        # options. Keep profile/authentication environment variables intact.
        environment.update(
            DBT_QUIET="false", DBT_USE_COLORS="false",
            DBT_LOG_FORMAT="text", DBT_LOG_LEVEL="info",
            DBT_WRITE_JSON="true", DBT_WRITE_METADATA="false",
            DBT_WARN_ERROR="false", DBT_WARN_ERROR_OPTIONS="{}",
            DBT_STORE_FAILURES="false",
        )
        (capture / "command.json").write_text(json.dumps(argv, indent=2) + "\n")
        completed = subprocess.run(
            argv, cwd=self.project, env=environment, text=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
        )
        (capture / "stdout.log").write_text(completed.stdout)
        (capture / "stderr.log").write_text(completed.stderr)
        output = completed.stdout + "\n" + completed.stderr
        for filename in ("run_results.json", "manifest.json"):
            artifact = self.artifacts / filename
            if artifact.exists():
                shutil.copy2(artifact, capture / filename)
        candidates = staged_candidates(output, self.public())
        created = created_candidates(output, self.public())
        require(created <= candidates, f"Created candidate has no staging event; see {capture}")
        require(
            all(name.startswith(f"__DBT_WAP_{invocation_id.hex.upper()}_") for name in candidates),
            f"Candidate does not belong to this invocation; see {capture}",
        )
        artifact = None
        if (results or candidates) and result_path.exists():
            artifact = json.loads(result_path.read_text())
            require(
                uuid.UUID(artifact["metadata"]["invocation_id"]) == invocation_id,
                f"Artifact does not belong to this invocation; see {capture}",
            )
        # A staging event is intent, not ownership: a create-only claim can fail
        # after preflight. Only a confirmed creation for our requested invocation
        # establishes ownership, even if run_results was not written afterward.
        prefix = self.variables.get("prefix")
        if prefix in self.owned and created:
            identifiers = self.owned[prefix]["identifiers"]
            identifiers[:] = sorted(set(identifiers) | created)
            self.persist_owned()
        require(
            (completed.returncode == 0) == success,
            f"Unexpected exit {completed.returncode}: {' '.join(command)}; see {capture}",
        )
        if not results:
            return {}, candidates, capture
        require(artifact is not None, f"Missing run_results.json; see {capture}")
        require(
            len(candidates) == int(staged),
            f"Expected {int(staged)} staged candidates; see {capture}",
        )
        require(created == candidates, f"Staged candidate lacks confirmed creation; see {capture}")
        return artifact, candidates, capture

    def operation(self, name: str, **kwargs: Any) -> None:
        self.invoke(["run-operation", name, "--args", json.dumps(kwargs)])

    def check(
        self, identifier: str, *, present: bool = True, ids: Any = None,
        transient: bool | None = None,
    ) -> None:
        if present and transient is None and (
            identifier == self.public() or identifier.startswith("__DBT_WAP_")
        ):
            transient = self.variables.get("transient", False)
        self.operation(
            "wap_fixture_assert", identifier=identifier,
            present=present, expected_ids=ids, expected_transient=transient,
        )

    def begin(self, kind: str, scenario: str, *, sentinel: bool) -> None:
        prefix = f"DBT_WAP_ACCEPT_{self.token}_{kind.upper()}_{scenario.upper()}"
        self.variables = {
            "prefix": prefix, "transient": kind == "transient",
            "audit_value": -1, "audit_severity": "error",
            "transformation_error": False, "self_read": False,
            "post_hook_id": None, "header_session": False,
            "wap_retain_failed": None,
        }
        identifiers = [f"{prefix}_ORDERS", f"{prefix}_DOWNSTREAM"]
        for identifier in identifiers:
            self.check(identifier, present=False)
        # Register only names verified absent in the selected schema.
        self.owned[prefix] = {
            "profile": self.args.profile, "target": self.args.target,
            "variables": dict(self.variables), "identifiers": identifiers,
        }
        self.persist_owned()
        if sentinel:
            self.operation("wap_fixture_prepare")

    def public(self, suffix: str = "ORDERS") -> str:
        return f"{self.variables['prefix']}_{suffix}"

    def build(
        self, *, success: bool, static_analysis: str | None = None,
        silence_audit_warnings: bool = False, staged: bool = True,
    ) -> tuple[dict[str, Any], set[str], Path]:
        command = ["build", "--select", "orders+", "--threads", "4"]
        if static_analysis is not None:
            command.extend(["--static-analysis", static_analysis])
        if silence_audit_warnings:
            command.extend(["--warn-error-options", json.dumps({"silence": ["LogTestResult"]})])
        return self.invoke(
            command,
            success=success, results=True, staged=staged,
        )

    @staticmethod
    def verify_audits(
        artifact: dict[str, Any], *, verdict: str, expected_audit_ids: set[str],
    ) -> set[str]:
        test_rows = [row for row in artifact["results"] if row["unique_id"].startswith("test.")]
        tests = {row["unique_id"]: row for row in test_rows}
        require(len(tests) == len(test_rows), "Expected one result per audit")
        require(set(tests) == expected_audit_ids, f"Expected all three audits from the manifest, got {list(tests)}")
        singular_id = f"test.{PROJECT}.nonnegative"
        require(singular_id in tests, "Missing nonnegative singular audit")
        for unique_id, row in tests.items():
            expected = "pass"
            if verdict in ("transform_error", "lifecycle_error", "collision_error"):
                expected = "skipped"
            elif verdict == "unique_fail" and unique_id.startswith(f"test.{PROJECT}.unique_orders_id."):
                expected = "fail"
            elif verdict == "null_audit" and unique_id == singular_id:
                expected = "error"
            elif unique_id == singular_id and verdict in ("fail", "warn"):
                expected = verdict
            require(row["status"] == expected, f"Unexpected audit result: {row}")
            if expected in ("fail", "warn"):
                require(row.get("failures") == 1, f"Expected one failing audit row: {row}")
        if verdict == "silenced_warn":
            require(
                tests[singular_id].get("failures") == 1,
                "Expected one failing row behind the silenced audit warning",
            )
        if verdict == "null_audit":
            require(
                "WAP audit failures must be" in str(tests[singular_id].get("message", "")),
                "Expected the NULL audit result validation error",
            )
        return set(tests)

    @staticmethod
    def verify_results(
        artifact: dict[str, Any], *, verdict: str, expected_audit_ids: set[str],
    ) -> set[str]:
        rows = artifact["results"]
        model_rows = [row for row in rows if row["unique_id"] == MODEL_ID]
        require(len(model_rows) == 1, "Expected exactly one canonical model result")
        expected_model = {
            "pass": "success", "fail": "skipped", "warn": "error", "transform_error": "error",
            "silenced_warn": "error",
            "unique_fail": "skipped", "lifecycle_error": "error", "collision_error": "error",
            "null_audit": "skipped",
        }[verdict]
        require(model_rows[0]["status"] == expected_model, f"Unexpected model result: {model_rows}")
        if verdict == "transform_error":
            require(
                TRANSFORMATION_ERROR in str(model_rows[0].get("message", "")),
                f"Expected the deliberate SQL transformation error: {model_rows}",
            )
        if verdict == "lifecycle_error":
            require(
                "cannot clone transient public table" in str(model_rows[0].get("message", "")),
                f"Expected the transient-to-permanent preflight rejection: {model_rows}",
            )
        if verdict == "collision_error":
            require(
                "already exists; it will not be overwritten" in str(model_rows[0].get("message", "")),
                f"Expected the candidate collision preflight rejection: {model_rows}",
            )
        audits = Acceptance.verify_audits(
            artifact, verdict=verdict, expected_audit_ids=expected_audit_ids,
        )
        downstream = [row for row in rows if row["unique_id"] == DOWNSTREAM_ID]
        expected_downstream = "success" if verdict == "pass" else "skipped"
        require(
            len(downstream) == 1 and downstream[0]["status"] == expected_downstream,
            f"Unexpected downstream result: {downstream}",
        )
        return audits

    def verify_existing_data_preserved(self) -> None:
        self.check(self.public(), ids=[999])
        self.check(self.public("DOWNSTREAM"), ids=[777])

    def verify_published(self, ids: list[int], candidates: set[str]) -> None:
        self.check(self.public(), ids=ids)
        self.check(self.public("DOWNSTREAM"), ids=ids)
        for candidate in candidates:
            self.check(candidate, present=False)
        compiled = list((self.artifacts / "compiled").rglob("downstream.sql"))
        require(len(compiled) == 1, "Expected compiled downstream SQL")
        sql = compiled[0].read_text()
        require(self.public() in sql, "Downstream ref did not use the public relation")
        require("__DBT_WAP_" not in sql, "Candidate relation leaked into downstream SQL")

    def run_null_audit_result(self, kind: str, audits: set[str]) -> None:
        self.begin(kind, "null_audit", sentinel=True)
        self.variables.update(audit_value=1, wap_retain_failed=True)
        audit_path = self.project / "tests" / "nonnegative.sql"
        original_sql = audit_path.read_text()
        try:
            # SQL-bearing audit settings must be direct literals. Edit only the
            # runner's copied fixture and restore it before later scenarios.
            # Aggregate even an empty test result into one row with NULL verdicts.
            audit_path.write_text("{{ config(fail_calc='nullif(count(*), count(*))') }}\n" + original_sql)
            errored, candidates, _ = self.build(success=False)
            self.verify_results(errored, verdict="null_audit", expected_audit_ids=audits)
            self.verify_existing_data_preserved()
            for candidate in candidates:
                self.check(candidate, ids=[1, 2])
        finally:
            audit_path.write_text(original_sql)

    def verify_failed_candidates_removed(self, candidates: set[str], capture: Path) -> None:
        output = (capture / "stdout.log").read_text() + "\n" + (capture / "stderr.log").read_text()
        require(
            candidates and created_candidates(output, self.public()) == candidates,
            "Expected confirmed candidate creation before failed-run cleanup",
        )
        created = {
            relation_components(match["candidate"])
            for match in CREATED_RELATIONS.finditer(output)
        }
        removed = {
            relation_components(match["candidate"])
            for match in REMOVED_FAILED_RELATIONS.finditer(output)
        }
        require(removed == created, "Expected exact failed candidate removal events")
        for candidate in candidates:
            self.check(candidate, present=False)

    def run_failed_candidate_cleanup(self, kind: str, audits: set[str]) -> None:
        for scenario, transformation_error, sentinel, retain_failed in (
            ("cleanup_audit", False, True, None),
            ("cleanup_transformation", True, True, None),
            ("cleanup_explicit_false", False, True, False),
            ("cleanup_first_failure", False, False, None),
        ):
            self.begin(kind, scenario, sentinel=sentinel)
            self.variables.update(wap_retain_failed=retain_failed, transformation_error=transformation_error)
            failed, candidates, capture = self.build(
                success=False, static_analysis="off" if transformation_error else None,
            )
            self.verify_results(
                failed, verdict="transform_error" if transformation_error else "fail",
                expected_audit_ids=audits,
            )
            if sentinel:
                self.verify_existing_data_preserved()
            else:
                self.check(self.public(), present=False)
                self.check(self.public("DOWNSTREAM"), present=False)
            self.verify_failed_candidates_removed(candidates, capture)

    def run_kind(self, kind: str) -> None:
        print(
            f"Checking {kind}: audit failure, candidate collision, retry, transformation error, warnings, "
            "NULL audit result, self-read, hooks and headers, first publish, first failure, "
            "failed candidate cleanup", flush=True,
        )
        self.begin(kind, "fail_retry", sentinel=True)
        self.variables["wap_retain_failed"] = True
        failed, retained, state = self.build(success=False)
        audits = fixture_audit_ids(json.loads((state / "manifest.json").read_text()))
        self.verify_results(failed, verdict="fail", expected_audit_ids=audits)
        self.verify_existing_data_preserved()
        for candidate in retained:
            self.check(candidate, ids=[-1, 2])

        # Deliberately reuse the failed invocation to collide with its retained
        # table. Even an otherwise-passing transformation must not overwrite it.
        self.variables.update(audit_value=3, wap_retain_failed=None)
        collided, _, _ = self.invoke(
            ["build", "--select", "orders+", "--threads", "4"],
            success=False, results=True, staged=False,
            invocation_id=uuid.UUID(failed["metadata"]["invocation_id"]),
        )
        self.verify_results(collided, verdict="collision_error", expected_audit_ids=audits)
        self.verify_existing_data_preserved()
        for candidate in retained:
            self.check(candidate, ids=[-1, 2])
        self.variables.update(audit_value=-1, wap_retain_failed=True)

        # A standalone test must use the public sentinel even with failing
        # candidate SQL left in the same target directory by the prior build.
        standalone, _, _ = self.invoke(
            ["test", "--select", "orders", "--threads", "4"], results=True,
        )
        require(
            self.verify_audits(standalone, verdict="pass", expected_audit_ids=audits) == audits,
            "Standalone test omitted an audit",
        )
        require(len(standalone["results"]) == 3, "Standalone test unexpectedly rebuilt a model")
        self.verify_existing_data_preserved()
        for candidate in retained:
            self.check(candidate, ids=[-1, 2])

        # Uniqueness passed before, but must fail on this new candidate. Reusing
        # its previous result, or testing the public sentinel, would publish bad data.
        self.variables["audit_value"] = 2
        duplicate, duplicate_candidates, duplicate_state = self.invoke(
            ["retry", "--state", str(state), "--threads", "4"],
            success=False, results=True, staged=True,
        )
        self.verify_results(duplicate, verdict="unique_fail", expected_audit_ids=audits)
        require(retained.isdisjoint(duplicate_candidates), "Retry reused a failed candidate")
        self.verify_existing_data_preserved()
        for candidate in duplicate_candidates:
            self.check(candidate, ids=[2, 2])

        self.variables["audit_value"] = 3
        retried, new_candidates, _ = self.invoke(
            ["retry", "--state", str(duplicate_state), "--threads", "4"], results=True, staged=True,
        )
        self.verify_results(retried, verdict="pass", expected_audit_ids=audits)
        require((retained | duplicate_candidates).isdisjoint(new_candidates), "Retry reused a failed candidate")
        self.verify_published([2, 3], new_candidates)
        for candidate in retained:
            self.check(candidate, ids=[-1, 2])
        for candidate in duplicate_candidates:
            self.check(candidate, ids=[2, 2])

        self.run_null_audit_result(kind, audits)

        self.begin(kind, "transformation_error", sentinel=True)
        self.variables.update(transformation_error=True, wap_retain_failed=True)
        # Force warehouse execution so local analysis cannot satisfy this case.
        errored, candidates, _ = self.build(success=False, static_analysis="off")
        self.verify_results(errored, verdict="transform_error", expected_audit_ids=audits)
        self.verify_existing_data_preserved()
        for candidate in candidates:
            self.check(candidate, ids=[999])

        self.begin(kind, "warning", sentinel=True)
        self.variables.update(audit_severity="warn", wap_retain_failed=True)
        warned, candidates, _ = self.build(success=False)
        self.verify_results(warned, verdict="warn", expected_audit_ids=audits)
        self.verify_existing_data_preserved()
        for candidate in candidates:
            self.check(candidate, ids=[-1, 2])

        self.begin(kind, "silenced_warning", sentinel=True)
        self.variables.update(audit_severity="warn", wap_retain_failed=True)
        warned, candidates, _ = self.build(success=False, silence_audit_warnings=True)
        self.verify_results(warned, verdict="silenced_warn", expected_audit_ids=audits)
        self.verify_existing_data_preserved()
        for candidate in candidates:
            self.check(candidate, ids=[-1, 2])

        self.begin(kind, "self_read", sentinel=True)
        self.variables["self_read"] = True
        # Exercise runtime `this` against the existing working clone. Static
        # analysis and compile-time relation introspection are separate paths.
        passed, candidates, _ = self.build(success=True, static_analysis="off")
        self.verify_results(passed, verdict="pass", expected_audit_ids=audits)
        self.verify_published([1000], candidates)

        self.run_hooks_and_header(kind, audits)

        self.begin(kind, "first_publish", sentinel=False)
        self.variables["audit_value"] = 1
        passed, candidates, _ = self.build(success=True)
        self.verify_results(passed, verdict="pass", expected_audit_ids=audits)
        self.verify_published([1, 2], candidates)

        self.begin(kind, "first_failure", sentinel=False)
        self.variables["wap_retain_failed"] = True
        failed, candidates, _ = self.build(success=False)
        self.verify_results(failed, verdict="fail", expected_audit_ids=audits)
        self.check(self.public(), present=False)
        self.check(self.public("DOWNSTREAM"), present=False)
        for candidate in candidates:
            self.check(candidate, ids=[-1, 2])

        self.run_failed_candidate_cleanup(kind, audits)

    def run_hooks_and_header(self, kind: str, audits: set[str]) -> None:
        self.begin(kind, "post_hook_publish", sentinel=True)
        self.variables.update(audit_value=1, post_hook_id=3)
        passed, candidates, _ = self.build(success=True)
        self.verify_results(passed, verdict="pass", expected_audit_ids=audits)
        self.verify_published([2, 3], candidates)

        self.begin(kind, "post_hook_audit_failure", sentinel=True)
        self.variables.update(audit_value=1, post_hook_id=-1, wap_retain_failed=True)
        failed, candidates, _ = self.build(success=False)
        self.verify_results(failed, verdict="fail", expected_audit_ids=audits)
        self.verify_existing_data_preserved()
        for candidate in candidates:
            self.check(candidate, ids=[-1, 2])

        self.begin(kind, "header_session", sentinel=True)
        self.variables["header_session"] = True
        # The session variable is established by sql_header on the CTAS connection.
        passed, candidates, _ = self.build(success=True, static_analysis="off")
        self.verify_results(passed, verdict="pass", expected_audit_ids=audits)
        self.verify_published([41], candidates)

    def run_lifecycle_transitions(self) -> None:
        print("Checking permanent-to-transient publication and transient-to-permanent rejection", flush=True)
        self.begin("permanent", "to_transient", sentinel=True)
        self.variables.update(transient=True, audit_value=1)
        passed, candidates, capture = self.build(success=True)
        audits = fixture_audit_ids(json.loads((capture / "manifest.json").read_text()))
        self.verify_results(passed, verdict="pass", expected_audit_ids=audits)
        self.verify_published([1, 2], candidates)

        self.begin("transient", "to_permanent", sentinel=True)
        self.variables["transient"] = False
        rejected, candidates, _ = self.build(success=False, staged=False)
        require(not candidates, "Rejected lifecycle conversion staged a working table")
        self.verify_results(rejected, verdict="lifecycle_error", expected_audit_ids=audits)
        self.check(self.public(), ids=[999], transient=True)
        self.check(self.public("DOWNSTREAM"), ids=[777])

    def cleanup(self) -> None:
        failures = []
        for entry in self.owned.values():
            self.variables = entry["variables"]
            try:
                self.operation("wap_fixture_cleanup", identifiers=entry["identifiers"])
            except Exception as error:
                failures.append(str(error))
        require(not failures, "Cleanup incomplete: " + "; ".join(failures))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dbt-bin", type=Path, required=True, help="Built binary with WAP changes")
    parser.add_argument("--profile", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--profiles-dir", type=Path, default=Path.home() / ".dbt")
    parser.add_argument("--table-kind", choices=("both", "permanent", "transient"), default="both")
    parser.add_argument("--keep-objects", action="store_true", help="Retain test-owned tables for inspection")
    args = parser.parse_args()
    args.dbt_bin = args.dbt_bin.expanduser().resolve()
    args.profiles_dir = args.profiles_dir.expanduser().resolve()
    require(args.dbt_bin.is_file(), f"No dbt binary: {args.dbt_bin}")
    require((args.profiles_dir / "profiles.yml").is_file(), "Missing profiles.yml")
    fixture = Acceptance(args)
    passed = False
    try:
        kinds = ("permanent", "transient") if args.table_kind == "both" else (args.table_kind,)
        for kind in kinds:
            fixture.run_kind(kind)
        if args.table_kind == "both":
            fixture.run_lifecycle_transitions()
        passed = True
    finally:
        if args.keep_objects:
            print(f"Objects retained; exact inventory: {fixture.work / 'owned_objects.json'}")
        else:
            fixture.cleanup()
    if passed:
        print(f"PASS: Snowflake WAP acceptance checks; evidence: {fixture.work}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print(f"FAIL: {error}", file=sys.stderr)
        sys.exit(1)
