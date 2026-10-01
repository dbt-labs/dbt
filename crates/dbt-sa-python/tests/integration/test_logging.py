import re
import shutil

from dbt.cli.main import dbtRunner

# Each invocation opens the log with `==== <timestamp> | <invocation_id> ====`.
_BANNER = re.compile(r"^=+ \S+ \| ([0-9a-f-]{36}) =+$", re.MULTILINE)


def _parse(runner, proj):
    res = runner.invoke(
        [
            "parse",
            "--quiet",
            "--log-level-file",
            "debug",
            "--project-dir",
            str(proj),
            "--profiles-dir",
            str(proj),
        ]
    )
    assert res.success, res.exception
    return res


def test_each_invocation_logs_to_its_own_project(tmp_project, tmp_path):
    """Two invokes on different projects each get their own logs/dbt.log.

    Tracing used to initialize once from the first invoke's args, pinning the log path
    process-wide, so the second project never got a log at all.
    """
    first = tmp_project("hello_world")
    second = tmp_path / "second_project"
    shutil.copytree(first, second)

    runner = dbtRunner()
    _parse(runner, first)
    _parse(runner, second)

    invocation_ids = []
    for proj in (first, second):
        log = proj / "logs" / "dbt.log"
        assert log.is_file(), f"{proj.name} never got its own log file"
        text = log.read_text()

        banners = _BANNER.findall(text)
        # Exactly one: a shared file would collect both invocations' banners.
        assert len(banners) == 1, f"{proj.name} log holds {len(banners)} invocations, want 1"
        invocation_ids.append(banners[0])

        # The config, not just the path, is per-invoke: this came from --log-level-file.
        assert "[debug]" in text, f"{proj.name} log has no debug lines"

    assert invocation_ids[0] != invocation_ids[1], (
        "both invocations reported the same invocation_id"
    )


def test_repeated_invocations_on_one_project_reuse_its_log(tmp_project):
    """Invoking the same project twice appends, rather than losing the second run."""
    proj = tmp_project("hello_world")

    runner = dbtRunner()
    _parse(runner, proj)
    _parse(runner, proj)

    text = (proj / "logs" / "dbt.log").read_text()
    banners = _BANNER.findall(text)
    assert len(banners) == 2, f"expected 2 invocations in the log, found {len(banners)}"
    assert banners[0] != banners[1]
