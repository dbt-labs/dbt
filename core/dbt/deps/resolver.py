from dataclasses import dataclass, field
from typing import Any, Dict, Iterator, List, NoReturn, Optional, Set, Type

from dbt.config import Project
from dbt.config.renderer import PackageRenderer
from dbt.contracts.project import (
    GitPackage,
    LocalPackage,
    PackageSpec,
    PrivatePackage,
    RegistryPackage,
    TarballPackage,
)
from dbt.deps.base import BasePackage, PinnedPackage, UnpinnedPackage
from dbt.deps.git import GitUnpinnedPackage
from dbt.deps.local import LocalUnpinnedPackage
from dbt.deps.registry import RegistryUnpinnedPackage
from dbt.deps.tarball import TarballUnpinnedPackage
from dbt.exceptions import (
    DependencyError,
    DuplicateDependencyToRootError,
    DuplicateProjectDependencyError,
    MismatchedDependencyTypeError,
)


@dataclass
class PackageListing:
    packages: Dict[str, UnpinnedPackage] = field(default_factory=dict)

    def __len__(self):
        return len(self.packages)

    def __bool__(self):
        return bool(self.packages)

    def _pick_key(self, key: BasePackage) -> str:
        for name in key.all_names():
            if name in self.packages:
                return name
        return key.name

    def __contains__(self, key: BasePackage):
        for name in key.all_names():
            if name in self.packages:
                return True

    def __getitem__(self, key: BasePackage):
        key_str: str = self._pick_key(key)
        return self.packages[key_str]

    def __setitem__(self, key: BasePackage, value):
        key_str: str = self._pick_key(key)
        self.packages[key_str] = value

    def _mismatched_types(self, old: UnpinnedPackage, new: UnpinnedPackage) -> NoReturn:
        raise MismatchedDependencyTypeError(new, old)

    def incorporate(self, package: UnpinnedPackage):
        key: str = self._pick_key(package)
        if key in self.packages:
            existing: UnpinnedPackage = self.packages[key]
            if not isinstance(existing, type(package)):
                self._mismatched_types(existing, package)
            self.packages[key] = existing.incorporate(package)
        else:
            self.packages[key] = package

    def update_from(self, src: List[PackageSpec], project_root: Optional[str] = None) -> None:
        pkg: UnpinnedPackage
        for contract in src:
            if isinstance(contract, LocalPackage):
                pkg = LocalUnpinnedPackage.from_contract(contract)
                # Override the project root for the local package contract IFF it is provided
                pkg.project_root = project_root if project_root else pkg.project_root
            elif isinstance(contract, TarballPackage):
                pkg = TarballUnpinnedPackage.from_contract(contract)
            elif isinstance(contract, GitPackage):
                pkg = GitUnpinnedPackage.from_contract(contract)
            elif isinstance(contract, PrivatePackage):
                raise DependencyError(
                    f'Cannot resolve private package {contract.private} because git provider integration is missing. Please use a "git" package instead.'
                )
            elif isinstance(contract, RegistryPackage):
                pkg = RegistryUnpinnedPackage.from_contract(contract)
            else:
                raise DependencyError("Invalid package type {}".format(type(contract)))
            self.incorporate(pkg)

    @classmethod
    def from_contracts(
        cls: Type["PackageListing"], src: List[PackageSpec], project_root: Optional[str] = None
    ) -> "PackageListing":
        self = cls({})
        self.update_from(src, project_root=project_root)
        return self

    def resolved(self) -> List[PinnedPackage]:
        return [p.resolved() for p in self.packages.values()]

    def __iter__(self) -> Iterator[UnpinnedPackage]:
        return iter(self.packages.values())


def _check_for_duplicate_project_names(
    final_deps: List[PinnedPackage],
    project: Project,
    renderer: PackageRenderer,
):
    seen: Set[str] = set()
    for package in final_deps:
        project_name = package.get_project_name(project, renderer)
        if project_name in seen:
            raise DuplicateProjectDependencyError(project_name)
        elif project_name == project.project_name:
            raise DuplicateDependencyToRootError(project_name)
        seen.add(project_name)


def resolve_packages(
    packages: List[PackageSpec],
    project: Project,
    cli_vars: Dict[str, Any],
) -> List[PinnedPackage]:
    pending = PackageListing.from_contracts(packages, project_root=project.project_root)
    final = PackageListing()

    renderer = PackageRenderer(cli_vars)

    while pending:
        next_pending = PackageListing()
        # resolve the dependency in question
        for package in pending:
            final.incorporate(package)
            target = final[package].resolved().fetch_metadata(project, renderer)

            # Hack to get the project root if it is a LocalPackage
            # https://github.com/dbt-labs/dbt-core/issues/5410
            project_root = None
            if isinstance(package, LocalUnpinnedPackage):
                project_root = package.resolved().resolve_path(project)

            next_pending.update_from(target.packages, project_root=project_root)
        pending = next_pending

    resolved = final.resolved()
    _check_for_duplicate_project_names(resolved, project, renderer)
    return resolved


def resolve_lock_packages(packages: List[PackageSpec]) -> List[PinnedPackage]:
    lock_packages = PackageListing.from_contracts(packages)
    final = PackageListing()

    for package in lock_packages:
        final.incorporate(package)

    resolved = final.resolved()

    return resolved
