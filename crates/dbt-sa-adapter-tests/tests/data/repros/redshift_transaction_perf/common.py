"""Shared helpers for the Redshift transaction experiment scripts."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

import redshift_connector
import yaml


def load_output(
    profile: str = "fusion_tests", output: str = "redshift"
) -> dict[str, Any]:
    """The connection settings of one profile output, from the user's profiles.yml."""
    profiles_dir = Path(os.environ.get("DBT_PROFILES_DIR", Path.home() / ".dbt"))
    profiles = yaml.safe_load((profiles_dir / "profiles.yml").read_text())
    return dict(profiles[profile]["outputs"][output])


def write_scoped_profiles(
    dest: Path, schema: str, profile: str = "fusion_tests", output: str = "redshift"
) -> Path:
    """Write a profiles.yml that targets `schema` on the same cluster. Returns its directory."""
    settings = {**load_output(profile, output), "schema": schema}
    dest.mkdir(parents=True, exist_ok=True)
    path = dest / "profiles.yml"
    path.write_text(
        yaml.safe_dump(
            {profile: {"target": "redshift", "outputs": {"redshift": settings}}}
        )
    )
    path.chmod(0o600)  # holds the password; the callers delete it when they finish
    return dest


def connect(settings: dict[str, Any]) -> Any:
    """An autocommit connection for admin work: schemas, catalog queries, visibility checks."""
    conn = redshift_connector.connect(
        host=settings["host"],
        port=settings["port"],
        user=settings["user"],
        password=settings["password"],
        database=settings["database"],
    )
    conn.autocommit = True
    return conn
