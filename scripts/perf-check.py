import json
import os
import pathlib
import subprocess
import sys
import time


projects = {
    # example of how to use project in separate public repo, not used yet.
    # "jaffle_shop": {
    # 	"name": "jaffle_shop",
    # 	"git_url": "https://github.com/dbt-labs/jaffle_shop.git",
    # 	"jobs": {
    # 		"jaffle_shop__parse_no_partial": {
    # 			"command": ["dbt", "parse", "--no-partial-parse"],
    # 		},
    # 	}
    # },
    "simple_models": {
        "name": "simple_models",
        "path": "./performance/projects/01_2000_simple_models",
        "jobs": {
            "simple_models__parse_no_partial": {
                "command": ["dbt", "parse", "--no-partial-parse"],
            },
            "simple_models__second_parse": {
                "command": ["dbt", "parse"],
            },
        },
    },
}


def print_usage() -> None:
    print("invalid usage")


def git_checkout(repo: str, path: pathlib.Path, commit: str = None) -> None:
    if not os.path.exists(path):
        print(f"Didn't find path {path}. Cloing {repo} into {path}.")
        res = subprocess.run(["git", "clone", repo, path], capture_output=True)
        res.check_returncode()
    else:
        print(f"Found path {path}. Skipping clone of {repo}.")

    if commit:
        print(f"Checking out commit {commit} for repo {repo}")
        res = subprocess.run(["git", "checkout", commit], cwd=path, capture_output=True)
        res.check_returncode()


def prepare_projects(projects) -> None:
    for project_name, project in projects.items():
        if "git_url" in project:
            git_checkout(project["git_url"], project_name)


def run_jobs(projects):
    results = {}

    for project_name, project in projects.items():
        for job_name, job in project["jobs"].items():
            print(f"running job {job_name}")
            cwd = project["path"] if "path" in project else project_name
            start = time.perf_counter()
            res = subprocess.run(job["command"], cwd=cwd)
            end = time.perf_counter()
            if res.returncode != 0:
                results[job_name] = {"succeeded": False}
            else:
                results[job_name] = {"succeeded": True, "time": end - start}

    return results


def compare(baseline_file: str, result_file: str) -> None:
    with open(baseline_file, "r") as b:
        baseline = json.load(b)

    with open(result_file, "r") as r:
        result = json.load(r)

    from rich.console import Console
    from rich.table import Table

    table = Table(title="Performance Comparison")
    table.add_column("Job Name")
    table.add_column("Baseline")
    table.add_column("Result")
    table.add_column("Change")

    for job_name, baseline_record in baseline.items():
        baseline_time = baseline_record.get("time")
        baseline_time_str = "{:.1f}s".format(baseline_time) if time is not None else "?"

        result_record = result[job_name]
        result_time = result_record.get("time")
        result_time_str = "{:.1f}s".format(baseline_time) if time is not None else "?"

        time_change_str = "-"
        if result_time and baseline_time:
            time_change_pct = 100.0 * (result_time - baseline_time) / baseline_time
            time_change_pfx = "[green]" if time_change_pct >= 0.0 else "[red]"
            time_change_str = time_change_pfx + "{:.1f}%".format(time_change_pct)

        table.add_row(job_name, baseline_time_str, result_time_str, time_change_str)

    print()
    Console().print(table)


def baseline(projects) -> None:
    prepare_projects(projects)
    results = run_jobs(projects)

    print("Writing results to 'perf_check.json'.")
    with open("perf_check.json", "w") as w:
        json.dump(results, w, indent=4)


if len(sys.argv) < 2:
    print_usage()
elif sys.argv[1] == "baseline":
    baseline(projects)
elif sys.argv[1] == "compare":
    compare(sys.argv[2], sys.argv[3])
else:
    print_usage()
