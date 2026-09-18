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
RELATION_COMPONENT = r'(?:"(?:[^"\r\n]|"")*"|[A-Za-z_][A-Za-z0-9_$]*)'
RELATION_NAME = rf"{RELATION_COMPONENT}\.{RELATION_COMPONENT}\.{RELATION_COMPONENT}"
STAGED_RELATIONS = re.compile(
    rf"WAP: building (?P<public>{RELATION_NAME}) in working table "
    rf"(?P<candidate>{RELATION_NAME})"
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


def staged_candidates(output: str, public_identifier: str) -> set[str]:
    identifiers = set(STAGED_CANDIDATE.findall(output))
    located = set()
    for match in STAGED_RELATIONS.finditer(output):
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
    require(located == identifiers, "Could not verify every staged candidate's database and schema")
    return located


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
        staged: bool = False,
    ) -> tuple[dict[str, Any], set[str], Path]:
        self.sequence += 1
        capture = self.work / f"{self.sequence:02d}_{command[0]}"
        capture.mkdir()
        result_path = self.artifacts / "run_results.json"
        result_path.unlink(missing_ok=True)
        argv = [
            str(self.args.dbt_bin),
            *command,
            "--project-dir", str(self.project),
            "--profiles-dir", str(self.args.profiles_dir),
            "--profile", self.args.profile,
            "--target", self.args.target,
            "--target-path", str(self.artifacts),
            "--vars", json.dumps(self.variables),
        ]
        environment = dict(os.environ)
        environment.update(DBT_QUIET="false", DBT_USE_COLORS="false")
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
        # Only the post-preflight "building" message establishes ownership.
        # A collision error's "retained if created" message does not.
        prefix = self.variables.get("prefix")
        if prefix in self.owned:
            identifiers = self.owned[prefix]["identifiers"]
            identifiers[:] = sorted(set(identifiers) | candidates)
            self.persist_owned()
        require(
            (completed.returncode == 0) == success,
            f"Unexpected exit {completed.returncode}: {' '.join(command)}; see {capture}",
        )
        if not results:
            return {}, candidates, capture
        require(result_path.exists(), f"Missing run_results.json; see {capture}")
        artifact = json.loads(result_path.read_text())
        invocation = uuid.UUID(artifact["metadata"]["invocation_id"]).hex.upper()
        require(
            all(name.startswith(f"__DBT_WAP_{invocation}_") for name in candidates),
            f"Candidate does not belong to this invocation; see {capture}",
        )
        require(
            len(candidates) == int(staged),
            f"Expected {int(staged)} staged candidates; see {capture}",
        )
        return artifact, candidates, capture

    def operation(self, name: str, **kwargs: Any) -> None:
        self.invoke(["run-operation", name, "--args", json.dumps(kwargs)])

    def check(self, identifier: str, *, present: bool = True, ids: Any = None) -> None:
        self.operation(
            "wap_fixture_assert", identifier=identifier,
            present=present, expected_ids=ids,
        )

    def begin(self, kind: str, scenario: str, *, sentinel: bool) -> None:
        prefix = f"DBT_WAP_ACCEPT_{self.token}_{kind.upper()}_{scenario.upper()}"
        self.variables = {
            "prefix": prefix, "transient": kind == "transient",
            "audit_value": -1, "audit_severity": "error",
            "transformation_error": False,
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
    ) -> tuple[dict[str, Any], set[str], Path]:
        command = ["build", "--select", "orders+", "--threads", "4"]
        if static_analysis is not None:
            command.extend(["--static-analysis", static_analysis])
        return self.invoke(
            command,
            success=success, results=True, staged=True,
        )

    @staticmethod
    def verify_audits(artifact: dict[str, Any], *, verdict: str) -> set[str]:
        test_rows = [row for row in artifact["results"] if row["unique_id"].startswith("test.")]
        tests = {row["unique_id"]: row for row in test_rows}
        require(len(tests) == len(test_rows), "Expected one result per audit")
        require(len(tests) == 3, f"Expected all three audits, got {list(tests)}")
        singular_id = f"test.{PROJECT}.nonnegative"
        require(singular_id in tests, "Missing nonnegative singular audit")
        for unique_id, row in tests.items():
            expected = "skipped" if verdict == "transform_error" else (
                verdict if unique_id == singular_id else "pass"
            )
            require(row["status"] == expected, f"Unexpected audit result: {row}")
        return set(tests)

    @staticmethod
    def verify_results(artifact: dict[str, Any], *, verdict: str) -> set[str]:
        rows = artifact["results"]
        model_rows = [row for row in rows if row["unique_id"] == MODEL_ID]
        require(len(model_rows) == 1, "Expected exactly one canonical model result")
        expected_model = {
            "pass": "success", "fail": "skipped", "warn": "error", "transform_error": "error",
        }[verdict]
        require(model_rows[0]["status"] == expected_model, f"Unexpected model result: {model_rows}")
        if verdict == "transform_error":
            require(
                TRANSFORMATION_ERROR in str(model_rows[0].get("message", "")),
                f"Expected the deliberate SQL transformation error: {model_rows}",
            )
        audits = Acceptance.verify_audits(artifact, verdict=verdict)
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

    def run_kind(self, kind: str) -> None:
        print(
            f"Checking {kind}: audit failure, retry, transformation error, warning, "
            "first publish, first failure", flush=True,
        )
        self.begin(kind, "fail_retry", sentinel=True)
        failed, retained, state = self.build(success=False)
        audits = self.verify_results(failed, verdict="fail")
        self.verify_existing_data_preserved()
        for candidate in retained:
            self.check(candidate, ids=[-1, 2])

        # A standalone test must use the public sentinel even with failing
        # candidate SQL left in the same target directory by the prior build.
        standalone, _, _ = self.invoke(
            ["test", "--select", "orders", "--threads", "4"], results=True,
        )
        require(
            self.verify_audits(standalone, verdict="pass") == audits,
            "Standalone test omitted an audit",
        )
        require(len(standalone["results"]) == 3, "Standalone test unexpectedly rebuilt a model")
        self.verify_existing_data_preserved()
        for candidate in retained:
            self.check(candidate, ids=[-1, 2])

        # The two generic audits passed in the failed build. They must appear
        # again when retry builds and certifies a new candidate.
        self.variables["audit_value"] = 3
        retried, new_candidates, _ = self.invoke(
            ["retry", "--state", str(state), "--threads", "4"], results=True, staged=True,
        )
        require(self.verify_results(retried, verdict="pass") == audits, "Retry omitted audits")
        require(retained.isdisjoint(new_candidates), "Retry reused a failed candidate")
        self.verify_published([2, 3], new_candidates)
        for candidate in retained:
            self.check(candidate, ids=[-1, 2])

        self.begin(kind, "transformation_error", sentinel=True)
        self.variables["transformation_error"] = True
        # Force warehouse execution so local analysis cannot satisfy this case.
        errored, _, _ = self.build(success=False, static_analysis="off")
        self.verify_results(errored, verdict="transform_error")
        self.verify_existing_data_preserved()

        self.begin(kind, "warning", sentinel=True)
        self.variables["audit_severity"] = "warn"
        warned, candidates, _ = self.build(success=False)
        self.verify_results(warned, verdict="warn")
        self.verify_existing_data_preserved()
        for candidate in candidates:
            self.check(candidate, ids=[-1, 2])

        self.begin(kind, "first_publish", sentinel=False)
        self.variables["audit_value"] = 1
        passed, candidates, _ = self.build(success=True)
        self.verify_results(passed, verdict="pass")
        self.verify_published([1, 2], candidates)

        self.begin(kind, "first_failure", sentinel=False)
        failed, candidates, _ = self.build(success=False)
        self.verify_results(failed, verdict="fail")
        self.check(self.public(), present=False)
        self.check(self.public("DOWNSTREAM"), present=False)
        for candidate in candidates:
            self.check(candidate, ids=[-1, 2])

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
