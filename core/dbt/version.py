import glob
import importlib
import importlib.util
import json
import os
from typing import Iterator, List, Optional, Tuple

import requests

import dbt_common.semver as semver
from dbt_common.ui import green, yellow

PYPI_VERSION_URL = "https://pypi.org/pypi/dbt-core/json"


def get_version_information() -> str:
    installed = get_installed_version()
    latest = get_latest_version()

    core_msg_lines, core_info_msg = _get_core_msg_lines(installed, latest)
    core_msg = _format_core_msg(core_msg_lines)
    plugin_version_msg = _get_plugins_msg()

    msg_lines = [core_msg]

    if core_info_msg != "":
        msg_lines.append(core_info_msg)

    msg_lines.append(plugin_version_msg)
    msg_lines.append("")

    return "\n\n".join(msg_lines)


def get_installed_version() -> semver.VersionSpecifier:
    return semver.VersionSpecifier.from_version_string(__version__)


def get_latest_version(
    version_url: str = PYPI_VERSION_URL,
) -> Optional[semver.VersionSpecifier]:
    try:
        resp = requests.get(version_url, timeout=1)
        data = resp.json()
        version_string = data["info"]["version"]
    except (json.JSONDecodeError, KeyError, requests.RequestException):
        return None

    return semver.VersionSpecifier.from_version_string(version_string)


def _get_core_msg_lines(
    installed: semver.VersionSpecifier,
    latest: Optional[semver.VersionSpecifier],
) -> Tuple[List[List[str]], str]:
    installed_s = installed.to_version_string(skip_matcher=True)
    installed_line = ["installed", installed_s, ""]
    update_info = ""

    if latest is None:
        update_info = (
            "  The latest version of dbt-core could not be determined!\n"
            "  Make sure that the following URL is accessible:\n"
            f"  {PYPI_VERSION_URL}"
        )
        return [installed_line], update_info

    latest_s = latest.to_version_string(skip_matcher=True)
    latest_line = ["latest", latest_s, green("Up to date!")]

    if installed > latest:
        latest_line[2] = yellow("Ahead of latest version!")
    elif installed < latest:
        latest_line[2] = yellow("Update available!")
        update_info = (
            "  Your version of dbt-core is out of date!\n"
            "  You can find instructions for upgrading here:\n"
            "  https://docs.getdbt.com/docs/installation"
        )

    return [
        installed_line,
        latest_line,
    ], update_info


def _format_core_msg(lines: List[List[str]]) -> str:
    msg = "Core:\n"
    msg_lines = []

    for name, version, update_msg in _pad_lines(lines, seperator=":"):
        line_msg = f"  - {name} {version}"
        if update_msg != "":
            line_msg += f" - {update_msg}"
        msg_lines.append(line_msg)

    return msg + "\n".join(msg_lines)


def _get_plugins_msg() -> str:
    msg_lines = ["Plugins:"]

    plugins = []
    display_update_msg = False
    for name, version_s in _get_dbt_plugins_info():
        compatability_msg, needs_update = _get_plugin_msg_info(name, version_s, installed)
        if needs_update:
            display_update_msg = True
        plugins.append([name, version_s, compatability_msg])

    for plugin in _pad_lines(plugins, seperator=":"):
        msg_lines.append(_format_single_plugin(plugin, ""))

    if display_update_msg:
        update_msg = (
            "  At least one plugin is out of date with dbt-core.\n"
            "  You can find instructions for upgrading here:\n"
            "  https://docs.getdbt.com/docs/installation"
        )
        msg_lines += ["", update_msg]

    return "\n".join(msg_lines)


def _get_plugin_msg_info(
    name: str, version_s: str, core: semver.VersionSpecifier
) -> Tuple[str, bool]:
    plugin = semver.VersionSpecifier.from_version_string(version_s)
    latest_plugin = get_latest_version(version_url=get_package_pypi_url(name))

    needs_update = False

    if not latest_plugin:
        compatibility_msg = yellow("Could not determine latest version")
        return (compatibility_msg, needs_update)

    if plugin < latest_plugin:
        compatibility_msg = yellow("Update available!")
        needs_update = True
    elif plugin > latest_plugin:
        compatibility_msg = yellow("Ahead of latest version!")
    else:
        compatibility_msg = green("Up to date!")

    return (compatibility_msg, needs_update)


def _format_single_plugin(plugin: List[str], update_msg: str) -> str:
    name, version_s, compatability_msg = plugin
    msg = f"  - {name} {version_s} - {compatability_msg}"
    if update_msg != "":
        msg += f"\n{update_msg}\n"
    return msg


def _pad_lines(lines: List[List[str]], seperator: str = "") -> List[List[str]]:
    if len(lines) == 0:
        return []

    # count the max line length for each column in the line
    counter = [0] * len(lines[0])
    for line in lines:
        for i, item in enumerate(line):
            counter[i] = max(counter[i], len(item))

    result: List[List[str]] = []
    for i, line in enumerate(lines):
        # add another list to hold padded strings
        if len(result) == i:
            result.append([""] * len(line))

        # iterate over columns in the line
        for j, item in enumerate(line):
            # the last column does not need padding
            if j == len(line) - 1:
                result[i][j] = item
                continue

            # if the following column has no length
            # the string does not need padding
            if counter[j + 1] == 0:
                result[i][j] = item
                continue

            # only add the seperator to the first column
            offset = 0
            if j == 0 and seperator != "":
                item += seperator
                offset = len(seperator)

            result[i][j] = item.ljust(counter[j] + offset)

    return result


def get_package_pypi_url(package_name: str) -> str:
    return f"https://pypi.org/pypi/dbt-{package_name}/json"


def _get_dbt_plugins_info() -> Iterator[Tuple[str, str]]:
    for plugin_name in _get_adapter_plugin_names():
        if plugin_name == "core":
            continue
        try:
            mod = importlib.import_module(f"dbt.adapters.{plugin_name}.__version__")
        except ImportError:
            # not an adapter
            continue
        yield plugin_name, mod.version


def _get_adapter_plugin_names() -> Iterator[str]:
    spec = importlib.util.find_spec("dbt.adapters")
    # If None, then nothing provides an importable 'dbt.adapters', so we will
    # not be reporting plugin versions today
    if spec is None or spec.submodule_search_locations is None:
        return

    for adapters_path in spec.submodule_search_locations:
        version_glob = os.path.join(adapters_path, "*", "__version__.py")
        for version_path in glob.glob(version_glob):
            # the path is like .../dbt/adapters/{plugin_name}/__version__.py
            # except it could be \\ on windows!
            plugin_root, _ = os.path.split(version_path)
            _, plugin_name = os.path.split(plugin_root)
            yield plugin_name


__version__ = "1.10.0a1.dev02162025"
installed = get_installed_version()
