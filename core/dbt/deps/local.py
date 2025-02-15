import shutil
from typing import Dict

from dbt.config.project import PartialProject, Project
from dbt.config.renderer import PackageRenderer
from dbt.contracts.project import LocalPackage, ProjectPackageMetadata
from dbt.deps.base import PinnedPackage, UnpinnedPackage
from dbt.events.types import DepsCreatingLocalSymlink, DepsSymlinkNotAvailable
from dbt_common.clients import system
from dbt_common.events.functions import fire_event


class LocalPackageMixin:
    def __init__(self, local: str) -> None:
        super().__init__()
        self.local = local

    @property
    def name(self):
        return self.local

    def source_type(self):
        return "local"


class LocalPinnedPackage(LocalPackageMixin, PinnedPackage):
    def __init__(self, local: str) -> None:
        super().__init__(local)

    def to_dict(self) -> Dict[str, str]:
        return {
            "local": self.local,
        }

    def get_version(self):
        return None

    def nice_version_name(self):
        return "<local @ {}>".format(self.local)

    def resolve_path(self, project):
        return system.resolve_path_from_base(
            self.local,
            project.project_root,
        )

    def _fetch_metadata(
        self, project: Project, renderer: PackageRenderer
    ) -> ProjectPackageMetadata:
        partial = PartialProject.from_project_root(self.resolve_path(project))
        return partial.render_package_metadata(renderer)

    def install(self, project, renderer):
        src_path = self.resolve_path(project)
        dest_path = self.get_installation_path(project, renderer)

        if system.path_exists(dest_path):
            if not system.path_is_symlink(dest_path):
                system.rmdir(dest_path)
            else:
                system.remove_file(dest_path)
        try:
            fire_event(DepsCreatingLocalSymlink())
            system.make_symlink(src_path, dest_path)
        except OSError:
            fire_event(DepsSymlinkNotAvailable())
            shutil.copytree(
                src_path,
                dest_path,
                ignore=lambda directory, contents: project.packages_install_path
                if directory == project.project_root
                else [],
            )


class LocalUnpinnedPackage(LocalPackageMixin, UnpinnedPackage[LocalPinnedPackage]):
    @classmethod
    def from_contract(cls, contract: LocalPackage) -> "LocalUnpinnedPackage":
        return cls(local=contract.local)

    def incorporate(self, other: "LocalUnpinnedPackage") -> "LocalUnpinnedPackage":
        return LocalUnpinnedPackage(local=self.local)

    def resolved(self) -> LocalPinnedPackage:
        return LocalPinnedPackage(local=self.local)
