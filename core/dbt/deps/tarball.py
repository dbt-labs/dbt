import functools
import os
from pathlib import Path
from typing import Dict

from dbt.config.project import PartialProject
from dbt.contracts.project import TarballPackage
from dbt.deps.base import PinnedPackage, UnpinnedPackage, get_downloads_path
from dbt.events.types import DepsScrubbedPackageName
from dbt.exceptions import DependencyError, env_secrets, scrub_secrets
from dbt_common.clients import system
from dbt_common.events.functions import fire_event
from dbt_common.utils.connection import connection_exception_retry


class TarballPackageMixin:
    def __init__(self, tarball: str, tarball_unrendered: str) -> None:
        super().__init__()
        self.tarball = tarball
        self.tarball_unrendered = tarball_unrendered

    @property
    def name(self):
        return self.tarball

    def source_type(self) -> str:
        return "tarball"


class TarballPinnedPackage(TarballPackageMixin, PinnedPackage):
    def __init__(self, tarball: str, tarball_unrendered: str, package: str) -> None:
        super().__init__(tarball, tarball_unrendered)
        self.package = package
        self.version = "tarball"
        downloads_path = get_downloads_path()
        self.tar_path = os.path.join(Path(downloads_path), self.package)
        self._validate_tar_path(downloads_path)
        self.untarred_path = f"{self.tar_path}_untarred"

    def _validate_tar_path(self, downloads_path: str) -> None:
        """Ensure the download location stays inside the downloads directory."""
        root = os.path.realpath(downloads_path)
        resolved = os.path.realpath(self.tar_path)
        if resolved == root or os.path.commonpath([root, resolved]) != root:
            raise DependencyError(
                f"Invalid package name '{self.package}': resolves outside of the "
                "downloads directory"
            )

    @property
    def name(self):
        return self.package

    def to_dict(self) -> Dict[str, str]:
        tarball_scrubbed = scrub_secrets(self.tarball_unrendered, env_secrets())
        if self.tarball_unrendered != tarball_scrubbed:
            fire_event(
                DepsScrubbedPackageName(package_name=tarball_scrubbed),
                force_warn_or_error_handling=True,
            )
        return {
            "tarball": tarball_scrubbed,
            "name": self.package,
        }

    def get_version(self):
        return self.version

    def nice_version_name(self):
        return f"tarball (url: {self.tarball})"

    def _fetch_metadata(self, project, renderer):
        """Download and untar the project and parse metadata from the project folder."""
        download_untar_fn = functools.partial(
            self.download_and_untar, self.tarball, self.tar_path, self.untarred_path, self.name
        )
        connection_exception_retry(download_untar_fn, 5)

        tar_contents = os.listdir(self.untarred_path)
        if len(tar_contents) != 1:
            raise DependencyError(
                f"Incorrect structure for package extracted from {self.tarball}."
                f"The extracted package needs to follow the structure {self.name}/<package_contents>."
            )
        child_folder = os.listdir(self.untarred_path)[0]

        self.untarred_path = os.path.join(self.untarred_path, child_folder)
        partial = PartialProject.from_project_root(self.untarred_path)
        metadata = partial.render_package_metadata(renderer)
        metadata.name = self.package if self.package else metadata.name
        return metadata

    def install(self, project, renderer):
        download_untar_fn = functools.partial(
            self.download_and_untar, self.tarball, self.tar_path, self.untarred_path, self.name
        )
        connection_exception_retry(download_untar_fn, 5)
        dest_path = self.get_installation_path(project, renderer)
        if os.path.exists(dest_path):
            if system.path_is_symlink(dest_path):
                system.remove_file(dest_path)
            else:
                system.rmdir(dest_path)
        system.move(self.untarred_path, dest_path)


class TarballUnpinnedPackage(TarballPackageMixin, UnpinnedPackage[TarballPinnedPackage]):
    def __init__(
        self,
        tarball: str,
        tarball_unrendered: str,
        package: str,
    ) -> None:
        super().__init__(tarball, tarball_unrendered)
        # setup to recycle RegistryPinnedPackage fns
        self.package = package
        self.version = "tarball"

    @classmethod
    def from_contract(cls, contract: TarballPackage) -> "TarballUnpinnedPackage":
        return cls(
            tarball=contract.tarball,
            tarball_unrendered=(contract.unrendered.get("tarball") or contract.tarball),
            package=contract.name,
        )

    def incorporate(self, other: "TarballUnpinnedPackage") -> "TarballUnpinnedPackage":
        return TarballUnpinnedPackage(
            tarball=self.tarball, tarball_unrendered=self.tarball_unrendered, package=self.package
        )

    def resolved(self) -> TarballPinnedPackage:
        return TarballPinnedPackage(
            tarball=self.tarball, tarball_unrendered=self.tarball_unrendered, package=self.package
        )
