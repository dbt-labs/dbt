import os
from contextlib import contextmanager
from datetime import datetime
from pathlib import Path
from typing import Optional


@contextmanager
def up_one(return_path: Optional[Path] = None):
    current_path = Path.cwd()
    os.chdir("../")
    try:
        yield
    finally:
        os.chdir(return_path or current_path)


def is_aware(dt: datetime) -> bool:
    return dt.tzinfo is not None and dt.tzinfo.utcoffset(dt) is not None


def installed_package_dirs(path: str = "dbt_packages") -> list:
    """Entries in the packages install dir, excluding the CACHEDIR.TAG marker dbt writes."""
    return [entry for entry in os.listdir(path) if entry != "CACHEDIR.TAG"]
