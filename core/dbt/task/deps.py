import json
import os
from hashlib import sha1
from pathlib import Path
from typing import Any, Dict, List, Optional

import yaml

import dbt.deprecations
import dbt.exceptions
import dbt.utils
from dbt.config import Project
from dbt.config.project import load_yml_dict, package_config_from_data
from dbt.config.renderer import PackageRenderer
from dbt.constants import PACKAGE_LOCK_FILE_NAME, PACKAGE_LOCK_HASH_KEY
from dbt.contracts.project import PackageSpec
from dbt.deps.base import downloads_directory
from dbt.deps.registry import RegistryPinnedPackage
from dbt.deps.resolver import resolve_lock_packages, resolve_packages
from dbt.events.types import (
    DepsAddPackage,
    DepsFoundDuplicatePackage,
    DepsInstallInfo,
    DepsListSubdirectory,
    DepsLockUpdating,
    DepsNoPackagesFound,
    DepsNotifyUpdatesAvailable,
    DepsStartPackageInstall,
    DepsUpdateAvailable,
    DepsUpToDate,
)
from dbt.task.base import BaseTask, move_to_nearest_project_dir
from dbt_common.clients import system
from dbt_common.events.functions import fire_event
from dbt_common.events.types import Formatting, Note

CACHEDIR_TAG_FILE_NAME = "CACHEDIR.TAG"
CACHEDIR_TAG_SIGNATURE = "Signature: 8a477f597d28d172789f06886806bc55"
CACHEDIR_TAG_CONTENTS = (
    f"{CACHEDIR_TAG_SIGNATURE}\n"
    "# This file is a cache directory tag created by dbt.\n"
    "# For information about cache directory tags see https://bford.info/cachedir/\n"
)


class dbtPackageDumper(yaml.Dumper):
    def increase_indent(self, flow=False, indentless=False):
        return super(dbtPackageDumper, self).increase_indent(flow, False)


def _has_cachedir_tag(path: str) -> bool:
    tag_path = os.path.join(path, CACHEDIR_TAG_FILE_NAME)
    if os.path.islink(tag_path) or not os.path.isfile(tag_path):
        return False
    try:
        with open(tag_path, "r") as f:
            return f.read(len(CACHEDIR_TAG_SIGNATURE)) == CACHEDIR_TAG_SIGNATURE
    except (OSError, UnicodeDecodeError):
        return False


def _is_non_empty_dir(path: str) -> bool:
    return os.path.isdir(path) and bool(os.listdir(path))


def _warn_if_removing_unowned_dir(path: str) -> None:
    """Note when about to remove a non-empty directory that dbt did not create.

    Fired as a Note, not a warning, so `--warn-error` cannot block the migration of
    existing untagged install directories: the tag is only written after a successful run.
    """
    if not _is_non_empty_dir(path):
        return
    if _has_cachedir_tag(path):
        return
    fire_event(
        Note(
            msg=(
                f"Removing '{path}' before installing packages, but it is missing a valid "
                f"'{CACHEDIR_TAG_FILE_NAME}' file, so dbt cannot confirm that it created this "
                "directory."
            )
        )
    )


def _write_cachedir_tag(path: str) -> None:
    with open(os.path.join(path, CACHEDIR_TAG_FILE_NAME), "w") as f:
        f.write(CACHEDIR_TAG_CONTENTS)


def _create_sha1_hash(packages: List[PackageSpec]) -> str:
    """Create a SHA1 hash of the packages list,
    this is used to determine if the packages for current execution matches
    the previous lock.

    Args:
        list[Packages]: list of packages specified that are already rendered

    Returns:
        str: SHA1 hash of the packages list
    """
    package_strs = [json.dumps(package.to_dict(), sort_keys=True) for package in packages]
    package_strs = sorted(package_strs)

    return sha1("\n".join(package_strs).encode("utf-8")).hexdigest()


def _create_packages_yml_entry(package: str, version: Optional[str], source: str) -> dict:
    """Create a formatted entry to add to `packages.yml` or `package-lock.yml` file

    Args:
        package (str): Name of package to download
        version (str): Version of package to download
        source (str): Source of where to download package from

    Returns:
        dict: Formatted dict to write to `packages.yml` or `package-lock.yml` file
    """
    package_key = source
    version_key = "version"

    if source == "hub":
        package_key = "package"

    packages_yml_entry = {package_key: package}

    if source == "git":
        version_key = "revision"

    if version:
        if "," in version:
            version = version.split(",")  # type: ignore

        packages_yml_entry[version_key] = version

    return packages_yml_entry


class DepsTask(BaseTask):
    def __init__(self, args: Any, project: Project) -> None:
        super().__init__(args=args)
        # N.B. This is a temporary fix for a bug when using relative paths via
        # --project-dir with deps.  A larger overhaul of our path handling methods
        # is needed to fix this the "right" way.
        # See GH-7615
        project.project_root = str(Path(project.project_root).resolve())
        self.project = project
        self.cli_vars = args.vars

    def track_package_install(
        self, package_name: str, source_type: str, version: Optional[str]
    ) -> None:
        # Hub packages do not need to be hashed, as they are public
        if source_type == "local":
            package_name = dbt.utils.md5(package_name)
            version = "local"
        elif source_type == "tarball":
            package_name = dbt.utils.md5(package_name)
            version = "tarball"
        elif source_type != "hub":
            package_name = dbt.utils.md5(package_name)
            version = dbt.utils.md5(version)

        dbt.tracking.track_package_install(
            "deps",
            self.project.hashed_name(),
            {"name": package_name, "source": source_type, "version": version},
        )

    def check_for_duplicate_packages(self, packages_yml):
        """Loop through contents of `packages.yml` to remove entries that match the package being added.

        This method is called only during `dbt deps --add-package` to check if the package
        being added already exists in packages.yml. It uses substring matching to identify
        duplicates, which means it will match across different package sources. For example,
        adding a hub package "dbt-labs/dbt_utils" will remove an existing git package
        "https://github.com/dbt-labs/dbt-utils.git" since both contain "dbt_utils" or "dbt-utils".

        The matching is flexible to handle both underscore and hyphen variants of package names,
        as git repos often use hyphens (dbt-utils) while package names use underscores (dbt_utils).
        Word boundaries (/, .) are enforced to prevent false matches like "dbt-core" matching
        "dbt-core-utils".

        Args:
            packages_yml (dict): In-memory read of `packages.yml` contents

        Returns:
            dict: Updated packages_yml contents with matching packages removed
        """
        # Extract the package name for matching
        package_name = self.args.add_package["name"]

        # Create variants for flexible matching (handle _ vs -)
        # Check multiple variants to handle naming inconsistencies between hub and git
        package_name_parts = [
            package_name,  # Original: "dbt-labs/dbt_utils"
            package_name.replace("_", "-"),  # Hyphens: "dbt-labs/dbt-utils"
            package_name.replace("-", "_"),  # Underscores: "dbt_labs/dbt_utils"
        ]
        # Extract just the package name without org (after last /)
        if "/" in package_name:
            short_name = package_name.split("/")[-1]
            package_name_parts.extend(
                [
                    short_name,  # "dbt_utils"
                    short_name.replace("_", "-"),  # "dbt-utils"
                    short_name.replace("-", "_"),  # "dbt_utils" (deduplicated)
                ]
            )

        # Remove duplicates from package_name_parts
        package_name_parts = list(set(package_name_parts))

        # Iterate backwards to safely delete items without index shifting issues
        for i in range(len(packages_yml["packages"]) - 1, -1, -1):
            pkg_entry = packages_yml["packages"][i]

            # Get the package identifier key (package type determines which key exists)
            # This avoids iterating over non-string values like warn-unpinned: false
            package_identifier = (
                pkg_entry.get("package")  # hub/registry package
                or pkg_entry.get("git")  # git package
                or pkg_entry.get("local")  # local package
                or pkg_entry.get("tarball")  # tarball package
                or pkg_entry.get("private")  # private package
            )

            # Check if any variant of the package name appears in the identifier
            # Use word boundaries to avoid false matches (e.g., "dbt-core" shouldn't match "dbt-core-utils")
            # Word boundaries are: start/end of string, /, or .
            # Note: - and _ are NOT boundaries since they're used within compound package names
            if package_identifier:
                is_duplicate = False
                for name_variant in package_name_parts:
                    if name_variant in package_identifier:
                        # Found a match, now verify it's not a substring of a larger word
                        # Check characters before and after the match
                        idx = package_identifier.find(name_variant)
                        start_ok = idx == 0 or package_identifier[idx - 1] in "/."
                        end_idx = idx + len(name_variant)
                        end_ok = (
                            end_idx == len(package_identifier)
                            or package_identifier[end_idx] in "/."
                        )

                        if start_ok and end_ok:
                            is_duplicate = True
                            break

                if is_duplicate:
                    del packages_yml["packages"][i]
                    # Filter out non-string values (like warn-unpinned boolean) before logging
                    # Note: Check for bool first since bool is a subclass of int in Python
                    loggable_package = {
                        k: v
                        for k, v in pkg_entry.items()
                        if not isinstance(v, bool)
                        and isinstance(v, (str, int, float))
                        and k != "unrendered"
                    }
                    fire_event(DepsFoundDuplicatePackage(removed_package=loggable_package))

        return packages_yml

    def add(self):
        packages_yml_filepath = (
            f"{self.project.project_root}/{self.project.packages_specified_path}"
        )
        if not system.path_exists(packages_yml_filepath):
            with open(packages_yml_filepath, "w") as package_yml:
                yaml.safe_dump({"packages": []}, package_yml)
            fire_event(Formatting("Created packages.yml"))

        new_package_entry = _create_packages_yml_entry(
            self.args.add_package["name"], self.args.add_package["version"], self.args.source
        )

        with open(packages_yml_filepath, "r") as user_yml_obj:
            packages_yml = yaml.safe_load(user_yml_obj)
            packages_yml = self.check_for_duplicate_packages(packages_yml)
            packages_yml["packages"].append(new_package_entry)

        self.project.packages.packages = package_config_from_data(packages_yml).packages

        if packages_yml:
            with open(packages_yml_filepath, "w") as pkg_obj:
                pkg_obj.write(
                    yaml.dump(packages_yml, Dumper=dbtPackageDumper, default_flow_style=False)
                )

                fire_event(
                    DepsAddPackage(
                        package_name=self.args.add_package["name"],
                        version=self.args.add_package["version"],
                        packages_filepath=packages_yml_filepath,
                    )
                )

    def lock(self) -> None:
        lock_filepath = f"{self.project.project_root}/{PACKAGE_LOCK_FILE_NAME}"

        packages = self.project.packages.packages
        packages_installed: Dict[str, Any] = {"packages": []}

        if not packages:
            fire_event(DepsNoPackagesFound())
            return

        with downloads_directory():
            resolved_deps = resolve_packages(packages, self.project, self.cli_vars)

        # this loop is to create the package-lock.yml in the same format as original packages.yml
        # package-lock.yml includes both the stated packages in packages.yml along with dependent packages
        renderer = PackageRenderer(self.cli_vars)
        for package in resolved_deps:
            package_dict = package.to_dict()
            package_dict["name"] = package.get_project_name(self.project, renderer)
            packages_installed["packages"].append(package_dict)

        packages_installed[PACKAGE_LOCK_HASH_KEY] = _create_sha1_hash(
            self.project.packages.packages
        )

        with open(lock_filepath, "w") as lock_obj:
            yaml.dump(packages_installed, lock_obj, Dumper=dbtPackageDumper)

        fire_event(DepsLockUpdating(lock_filepath=lock_filepath))

    def run(self) -> None:
        move_to_nearest_project_dir(self.args.project_dir)
        if self.args.add_package:
            self.add()

        # Check lock file exist and generated by the same packages.yml
        # or dependencies.yml.
        lock_file_path = f"{self.project.project_root}/{PACKAGE_LOCK_FILE_NAME}"
        if not system.path_exists(lock_file_path):
            self.lock()
        elif self.args.upgrade:
            self.lock()
        else:
            # Check dependency definition is modified or not.
            current_hash = _create_sha1_hash(self.project.packages.packages)
            previous_hash = load_yml_dict(lock_file_path).get(PACKAGE_LOCK_HASH_KEY, None)
            if previous_hash != current_hash:
                self.lock()

        # Early return when 'dbt deps --lock'
        # Just resolve packages and write lock file, don't actually install packages
        if self.args.lock:
            return

        if system.path_exists(self.project.packages_install_path):
            _warn_if_removing_unowned_dir(self.project.packages_install_path)
            system.rmtree(self.project.packages_install_path)

        system.make_directory(self.project.packages_install_path)
        _write_cachedir_tag(self.project.packages_install_path)

        packages_lock_dict = load_yml_dict(f"{self.project.project_root}/{PACKAGE_LOCK_FILE_NAME}")

        renderer = PackageRenderer(self.cli_vars)
        packages_lock_config = package_config_from_data(
            renderer.render_data(packages_lock_dict), packages_lock_dict
        ).packages

        if not packages_lock_config:
            fire_event(DepsNoPackagesFound())
            return

        with downloads_directory():
            lock_defined_deps = resolve_lock_packages(packages_lock_config)
            renderer = PackageRenderer(self.cli_vars)

            packages_to_upgrade = []

            for package in lock_defined_deps:
                package_name = package.name
                source_type = package.source_type()
                version = package.get_version()

                fire_event(DepsStartPackageInstall(package_name=package_name))
                package.install(self.project, renderer)

                fire_event(DepsInstallInfo(version_name=package.nice_version_name()))

                if isinstance(package, RegistryPinnedPackage):
                    version_latest = package.get_version_latest()

                    if version_latest != version:
                        packages_to_upgrade.append(package_name)
                        fire_event(DepsUpdateAvailable(version_latest=version_latest))
                    else:
                        fire_event(DepsUpToDate())

                if package.get_subdirectory():
                    fire_event(DepsListSubdirectory(subdirectory=package.get_subdirectory()))

                self.track_package_install(
                    package_name=package_name, source_type=source_type, version=version
                )

            if packages_to_upgrade:
                fire_event(Formatting(""))
                fire_event(DepsNotifyUpdatesAvailable(packages=packages_to_upgrade))
