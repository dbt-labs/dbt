import abc
import functools
import os
import tempfile
from contextlib import contextmanager
from pathlib import Path
from typing import Dict, Generic, List, Optional, TypeVar

from dbt.contracts.project import ProjectPackageMetadata
from dbt.events.types import DepsSetDownloadDirectory
from dbt_common.clients import system
from dbt_common.events.functions import fire_event
from dbt_common.utils.connection import connection_exception_retry

DOWNLOADS_PATH = None


def get_downloads_path():
    return DOWNLOADS_PATH


@contextmanager
def downloads_directory():
    global DOWNLOADS_PATH
    remove_downloads = False
    # the user might have set an environment variable. Set it to that, and do
    # not remove it when finished.
    if DOWNLOADS_PATH is None:
        DOWNLOADS_PATH = os.getenv("DBT_DOWNLOADS_DIR")
        remove_downloads = False
    # if we are making a per-run temp directory, remove it at the end of
    # successful runs
    if DOWNLOADS_PATH is None:
        DOWNLOADS_PATH = tempfile.mkdtemp(prefix="dbt-downloads-")
        remove_downloads = True

    system.make_directory(DOWNLOADS_PATH)
    fire_event(DepsSetDownloadDirectory(path=DOWNLOADS_PATH))

    yield DOWNLOADS_PATH

    if remove_downloads:
        system.rmtree(DOWNLOADS_PATH)
        DOWNLOADS_PATH = None


class BasePackage(metaclass=abc.ABCMeta):
    @abc.abstractproperty
    def name(self) -> str:
        raise NotImplementedError

    def all_names(self) -> List[str]:
        return [self.name]

    @abc.abstractmethod
    def source_type(self) -> str:
        raise NotImplementedError


class PinnedPackage(BasePackage):
    def __init__(self) -> None:
        self._cached_metadata: Optional[ProjectPackageMetadata] = None

    def __str__(self) -> str:
        version = self.get_version()
        if not version:
            return self.name

        return "{}@{}".format(self.name, version)

    @abc.abstractmethod
    def get_version(self) -> Optional[str]:
        raise NotImplementedError

    @abc.abstractmethod
    def _fetch_metadata(self, project, renderer):
        raise NotImplementedError

    @abc.abstractmethod
    def install(self, project, renderer):
        raise NotImplementedError

    @abc.abstractmethod
    def nice_version_name(self):
        raise NotImplementedError

    @abc.abstractmethod
    def to_dict(self) -> Dict[str, Optional[str]]:
        raise NotImplementedError

    def fetch_metadata(self, project, renderer):
        if not self._cached_metadata:
            self._cached_metadata = self._fetch_metadata(project, renderer)
        return self._cached_metadata

    def get_project_name(self, project, renderer):
        metadata = self.fetch_metadata(project, renderer)
        return metadata.name

    def get_installation_path(self, project, renderer):
        dest_dirname = self.get_project_name(project, renderer)
        return os.path.join(project.packages_install_path, dest_dirname)

    def get_subdirectory(self):
        return None

    def _install(self, project, renderer):
        metadata = self.fetch_metadata(project, renderer)

        tar_name = f"{self.package}.{self.version}.tar.gz"
        tar_path = (Path(get_downloads_path()) / tar_name).resolve(strict=False)
        system.make_directory(str(tar_path.parent))

        download_url = metadata.downloads.tarball
        deps_path = project.packages_install_path
        package_name = self.get_project_name(project, renderer)

        download_untar_fn = functools.partial(
            self.download_and_untar, download_url, str(tar_path), deps_path, package_name
        )
        connection_exception_retry(download_untar_fn, 5)

    def download_and_untar(self, download_url, tar_path, deps_path, package_name):
        """
        Sometimes the download of the files fails and we want to retry.  Sometimes the
        download appears successful but the file did not make it through as expected
        (generally due to a github incident).  Either way we want to retry downloading
        and untarring to see if we can get a success.  Call this within
        `connection_exception_retry`
        """

        system.download(download_url, tar_path)
        system.untar_package(tar_path, deps_path, package_name)


SomePinned = TypeVar("SomePinned", bound=PinnedPackage)
SomeUnpinned = TypeVar("SomeUnpinned", bound="UnpinnedPackage")


class UnpinnedPackage(Generic[SomePinned], BasePackage):
    @classmethod
    @abc.abstractmethod
    def from_contract(cls, contract):
        raise NotImplementedError

    @abc.abstractmethod
    def incorporate(self: SomeUnpinned, other: SomeUnpinned) -> SomeUnpinned:
        raise NotImplementedError

    @abc.abstractmethod
    def resolved(self) -> SomePinned:
        raise NotImplementedError
