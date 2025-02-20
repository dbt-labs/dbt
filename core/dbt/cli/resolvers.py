from pathlib import Path

from dbt.config.project import PartialProject
from dbt.exceptions import DbtProjectError


def default_project_dir() -> Path:
    paths = list(Path.cwd().parents)
    paths.insert(0, Path.cwd())
    return next((x for x in paths if (x / "dbt_project.yml").exists()), Path.cwd())


def default_profiles_dir() -> Path:
    paths = [Path.cwd(), Path.home() / ".dbt"]
    return next((x for x in paths if (x / "profiles.yml").exists()), Path.cwd())


def default_log_path(project_dir: Path, verify_version: bool = False) -> Path:
    """If available, derive a default log path from dbt_project.yml. Otherwise, default to "logs".
    Known limitations:
    1. Using PartialProject here, so no jinja rendering of log-path.
    2. Programmatic invocations of the cli via dbtRunner may pass a Project object directly,
       which is not being taken into consideration here to extract a log-path.
    """
    default_log_path = Path("logs")
    try:
        partial = PartialProject.from_project_root(str(project_dir), verify_version=verify_version)
        partial_log_path = partial.project_dict.get("log-path") or default_log_path
        default_log_path = Path(project_dir) / partial_log_path
    except DbtProjectError:
        pass

    return default_log_path
