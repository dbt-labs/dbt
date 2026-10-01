from __future__ import annotations

from pathlib import Path

import yaml

from dbt.cli.resolvers import default_dbt_home_dir
from dbt.clients.yaml_helper import load_yaml_text
from dbt.constants import USER_SETTINGS_FILE_NAME
from dbt.contracts.user_settings import UserSettings
from dbt_common.clients.system import load_file_contents
from dbt_common.dataclass_schema import ValidationError
from dbt_common.exceptions import DbtValidationError


def _default_path() -> Path:
    return default_dbt_home_dir() / USER_SETTINGS_FILE_NAME


def read_user_settings(path: Path | None = None) -> UserSettings:
    if path is None:
        path = _default_path()

    if path.is_file():
        try:
            contents = load_file_contents(str(path), strip=False)
            yaml_content = load_yaml_text(contents)

            if not yaml_content:
                return UserSettings()

            return UserSettings.from_dict(yaml_content)
        except (ValidationError, DbtValidationError, ValueError) as e:
            raise DbtValidationError(f"invalid user settings in {path}: {e}") from e

    return UserSettings()


def write_user_settings(settings: UserSettings, path: Path | None = None) -> None:
    if path is None:
        path = _default_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(yaml.dump(settings.to_dict(), default_flow_style=False), encoding="utf-8")


def get_user_setting_flags(path: Path | None = None) -> dict:
    try:
        settings = read_user_settings(path)
    except (DbtValidationError, RuntimeError):
        # RuntimeError: Path.home() fails when HOME/USERPROFILE env vars are absent.
        return {}
    return settings.flags


def set_user_setting_flag(name: str, value: bool, path: Path | None = None) -> None:
    settings = read_user_settings(path)
    settings.flags[name] = value
    write_user_settings(settings, path)
