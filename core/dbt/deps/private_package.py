import contextlib
import json
import os
import uuid
from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Any, Dict, List, Optional
from urllib.parse import urlparse

from dbt.contracts.project import PrivatePackage
from dbt.deps.git import GitPinnedPackage, GitUnpinnedPackage
from dbt_common.dataclass_schema import StrEnum

PRIVATE_PACKAGE_HELPER = None


class PrivatePackageResolutionError(Exception):
    pass


class GitProvider(StrEnum):
    GITHUB = "github"
    GITLAB = "gitlab"
    AZURE_ACTIVE_DIRECTORY = (
        "azure_active_directory"  # ADO: accepted for backward compatibility; prefer "ado"
    )
    AZURE_DEVOPS = "azure_devops"  # ADO: accepted for backward compatibility; prefer "ado"
    ADO = "ado"

    def __eq__(self, other: Any) -> bool:
        if other is None:
            return False
        other_value = other.value if isinstance(other, GitProvider) else str(other)

        # ADO variants (azure_devops, azure_active_directory, ado) are equivalent
        ado_values = (
            GitProvider.AZURE_DEVOPS.value,
            GitProvider.AZURE_ACTIVE_DIRECTORY.value,
            GitProvider.ADO.value,
        )
        if self.value in ado_values:
            return other_value in ado_values

        return self.value == other_value

    def __ne__(self, other: object) -> bool:
        return not self.__eq__(other)


class SmartString(str, ABC):
    @property
    @abstractmethod
    def is_wildcard(self) -> bool:
        pass

    def __eq__(self, other: Any) -> bool:
        if not isinstance(other, self.__class__):
            other = self.__class__(other)  # type: ignore[abstract]

        # Wildcard matches any non-empty path;
        # empty does not match wildcard because it cannot be interpolated
        if (self.is_wildcard and bool(str(other))) or (other.is_wildcard and bool(str(self))):
            return True
        return super().__eq__(other)

    def __ne__(self, other: Any) -> bool:
        return not self.__eq__(other)


class OrgName(SmartString):
    """
    This is a smart string that represents an organization name.
    It provides custom comparison logic to allow enhanced matching
    that a common string would not provide.
    """

    @property
    def is_wildcard(self) -> bool:
        return False  # Not supported


class GroupName(SmartString):
    """
    This is a smart string that represents a group name (whatever comes between org and repo).
    It provides custom comparison logic to allow enhanced matching
    that a common string would not provide.
    """

    @property
    def is_wildcard(self) -> bool:
        return str(self) in ("{group}", "{project}")


class RepoName(SmartString):
    """
    This is a smart string that represents a repository name.
    It provides custom comparison logic to allow enhanced matching
    that a common string would not provide.
    """

    @property
    def is_wildcard(self) -> bool:
        return str(self) == "{repo}"


class PrivatePackageName(str):
    """
    Represents a private package name from the packages.private key in packages.yaml.
    It provides properties to extract organization, groups, and repository names,
    with custom equality logic based on those components.
    """

    @property
    def _parts(self) -> tuple[str, list[str], str]:
        head, *groups, tail = self.split("/")
        return head, groups, tail

    @property
    def org_name(self) -> OrgName:
        return OrgName(self._parts[0])

    @property
    def repo_name(self) -> RepoName:
        return RepoName(self._parts[-1])

    @property
    def group(self) -> GroupName:
        """Path between org and repo as a single string (e.g. 'team/subproject' or '')."""
        return GroupName("/".join(self._parts[1]))

    def __eq__(self, other: Any) -> bool:
        if not isinstance(other, PrivatePackageName):
            other = PrivatePackageName(other)

        # B1: When comparing with ADOLegacy (2-part package), match by org+repo only
        if isinstance(other, ADOLegacyPrivatePackageName) and not str(other.group):
            return self.org_name == other.org_name and self.repo_name == other.repo_name
        return (
            self.org_name == other.org_name
            and self.repo_name == other.repo_name
            and self.group == other.group
        )

    def __ne__(self, other: Any) -> bool:
        return not self.__eq__(other)


class ADOLegacyPrivatePackageName(PrivatePackageName):
    """
    Legacy ADO package name that matches on org and repo only, ignoring intermediate path segments.
    Used for backward compatibility when the package name omits the project segment.
    When comparing with a full 3-part package name (org/project/repo),
    requires a group match to prevent a legacy URL from resolving the wrong repository.
    """

    def __eq__(self, other: Any) -> bool:
        if not isinstance(other, PrivatePackageName):
            other = PrivatePackageName(other)
        if not isinstance(other, ADOLegacyPrivatePackageName) and str(other.group):
            return super().__eq__(other)
        return self.org_name == other.org_name and self.repo_name == other.repo_name


@dataclass
class PrivatePackageDefinition:
    name: PrivatePackageName
    provider: GitProvider | None = None

    @classmethod
    def build(
        cls, name: str, provider: str | GitProvider | None = None
    ) -> "PrivatePackageDefinition":
        """Create a PrivatePackageDefinition from string inputs."""
        resolved_provider: GitProvider | None
        if isinstance(provider, str):
            try:
                resolved_provider = GitProvider(provider)
            except ValueError:
                valid = ", ".join(p.value for p in GitProvider)
                raise PrivatePackageResolutionError(
                    f"Invalid provider {provider!r}. Valid providers: {valid}"
                )
        else:
            resolved_provider = provider
        # Use legacy name format only for azure_active_directory/azure_devops (not ado)
        if resolved_provider is not None and resolved_provider.value in (
            GitProvider.AZURE_ACTIVE_DIRECTORY.value,
            GitProvider.AZURE_DEVOPS.value,
        ):
            resolved_name: PrivatePackageName = ADOLegacyPrivatePackageName(name)
        else:
            resolved_name = PrivatePackageName(name)
        return cls(name=resolved_name, provider=resolved_provider)


class GitURL(str):
    """
    This is a smart string that represents a Git URL.
    It provides properties to extract organization and repository names,
    and methods to resolve the URL with a token and repository name.
    """

    @property
    def _definition(self) -> PrivatePackageName:
        parts = urlparse(self)
        cleaned_path = parts.path.removeprefix("/").removesuffix(".git")
        return PrivatePackageName(cleaned_path)

    def resolve(self, token: str, private_def: PrivatePackageName) -> "GitURL":
        if self._definition == private_def:
            return self.__class__(self.format(token=token, private_def=private_def))

        raise PrivatePackageResolutionError(
            f"Unable to resolve the Git URL {self} for repo {private_def.repo_name}"
        )

    def format(self, token: str, private_def: PrivatePackageName) -> str:  # type: ignore[override]
        fill_kwargs = {
            "token": token,
            "repo": str(private_def.repo_name),
            "group": str(private_def.group),
            "project": str(private_def.group),  # ADO alias: {project} = group segment
        }
        return super().format(**fill_kwargs)


class ADOLegacyGitURL(GitURL):
    """
    Legacy ADO Git URL for 2-part format. Has '_git' in the path and
    uses ADOLegacyPrivatePackageName for matching (group is ignored).
    """

    @property
    def _definition(self) -> ADOLegacyPrivatePackageName:
        parts = urlparse(self)
        cleaned_path = parts.path.removeprefix("/").removesuffix(".git")
        definition_str = cleaned_path.replace("/_git/", "/")
        return ADOLegacyPrivatePackageName(definition_str)


class ADOGitURL(GitURL):
    """
    ADO Git URL for full org/project/repo format. Removes /_git/ from path
    and produces PrivatePackageName for matching (group is expected).
    """

    @property
    def _definition(self) -> PrivatePackageName:
        parts = urlparse(self)
        cleaned_path = parts.path.removeprefix("/").removesuffix(".git")
        definition_str = cleaned_path.replace("/_git/", "/")
        return PrivatePackageName(definition_str)


@dataclass
class GitRepository:
    url: GitURL
    token: str
    org: OrgName
    provider: GitProvider

    def __post_init__(self):
        if not isinstance(self.provider, GitProvider):
            self.provider = GitProvider(self.provider)

        if not isinstance(self.org, OrgName):
            self.org = OrgName(self.org)

        if not isinstance(self.url, GitURL):
            if self.provider.value == GitProvider.ADO.value:
                self.url = ADOGitURL(self.url)
            elif self.provider.value in (
                GitProvider.AZURE_DEVOPS.value,
                GitProvider.AZURE_ACTIVE_DIRECTORY.value,
            ):
                self.url = ADOLegacyGitURL(self.url)
            else:
                self.url = GitURL(self.url)

    def match(
        self,
        private_package: PrivatePackageDefinition,
    ) -> GitURL:
        name = private_package.name
        if self.org != name.org_name:
            raise PrivatePackageResolutionError(
                f"Provider org {self.org} does not match private definition org {name.org_name}"
            )
        if private_package.provider and self.provider != private_package.provider:
            raise PrivatePackageResolutionError(
                f"Provider {self.provider} does not match requested provider "
                f"{private_package.provider}"
            )
        # A2: provider=ado with 2-part must not use legacy repo (explicit ado = full format)
        if (
            isinstance(self.url, ADOLegacyGitURL)
            and private_package.provider is not None
            and private_package.provider.value == GitProvider.ADO.value
            and not str(name.group)
        ):
            raise PrivatePackageResolutionError(
                f"Package {name} with provider=ado requires full org/project/repo format; "
                f"legacy repo not applicable"
            )

        # 3-part paths (org/project/repo) also work with legacy providers for backward compatibility;
        # "ado" is the recommended provider for that format.
        return self.url.resolve(token=self.token, private_def=name)


class PrivatePackageHelper:
    def __init__(self, git_providers_str: str):
        """Load the git repository configuration and provide
        helper functions to convert PrivatePackage into GitUnpinnedPackage.

        Args:
            git_providers_str: JSON string containing a list of GitRepository configs, e.g.
                '[{"org": "dbt-labs", "url": "https://{token}@github.com/dbt-labs/awesome_repo.git", "token": "aaaaa", "provider": "github"}]'

        """
        self.git_repositories: list[GitRepository] = []
        git_repositories_json = json.loads(git_providers_str)
        for git_repository in git_repositories_json:
            git_repository = GitRepository(**git_repository)
            self.git_repositories.append(git_repository)

            # Registers token with dbt's secret scrubber; UUID name resists env_var() lookup.
            # See CORE-297
            os.environ[f"DBT_ENV_SECRETS_GIT_TOKEN_{uuid.uuid4()}"] = git_repository.token

    def get_resolved_url(self, private_def: str, provider: str | None = None) -> str:
        """Resolve a private package name to a Git URL with token.
        When multiple repositories could match, the first in the config list wins.
        """
        # If no providers were configured via DBT_ENV_PRIVATE_GIT_PROVIDER_INFO,
        # fall back to constructing an SSH URL.
        if not self.git_repositories:
            return _get_ssh_fallback_url(private_def, provider)

        private_package = PrivatePackageDefinition.build(name=private_def, provider=provider)

        for git_repository in self.git_repositories:
            with contextlib.suppress(PrivatePackageResolutionError):
                match = git_repository.match(private_package=private_package)
                return str(match)

        raise PrivatePackageResolutionError(
            f"No matching Git URLs for private definition {private_def} with provider {provider}"
        )


def _get_ssh_fallback_url(private_def: str, provider: Optional[str]) -> str:
    """SSH URL fallback when DBT_ENV_PRIVATE_GIT_PROVIDER_INFO is unset/empty.
    Matches the behavior of the v2 engine's get_local_resolved_url()."""
    resolved_provider = provider or "github"
    if resolved_provider == "github":
        return f"git@github.com:{private_def}.git"
    if resolved_provider == "gitlab":
        return f"git@gitlab.com:{private_def}.git"
    if resolved_provider in ("ado", "azure_devops"):
        if len(private_def.split("/")) < 3:
            raise PrivatePackageResolutionError(
                f"The '{resolved_provider}' provider requires org/project/repo format (3 parts), "
                f"got: '{private_def}'"
            )
        return f"git@ssh.dev.azure.com:v3/{private_def}"
    raise PrivatePackageResolutionError(
        f"Invalid private package configuration: '{private_def}' provider: "
        f"'{provider or ''}'. Valid providers are: github, gitlab, ado, azure_devops"
    )


def get_private_package_helper():
    global PRIVATE_PACKAGE_HELPER
    if PRIVATE_PACKAGE_HELPER is None:
        # DBT_ENV_PRIVATE_GIT_PROVIDER_INFO is set by dbt platform; its shape is not a stable public interface.
        PRIVATE_PACKAGE_HELPER = PrivatePackageHelper(
            os.environ.get("DBT_ENV_PRIVATE_GIT_PROVIDER_INFO", "[]")
        )
    return PRIVATE_PACKAGE_HELPER


class PrivatePinnedPackage(GitPinnedPackage):
    def __init__(
        self,
        git: str,
        git_unrendered: str,
        revision: str,
        warn_unpinned: bool = True,
        subdirectory: Optional[str] = None,
        # Preserved through the lock file round-trip so the URL can be
        # reconstructed when dbt reads the lock file back for installation.
        provider: Optional[str] = None,
    ) -> None:
        super().__init__(git, git_unrendered, revision, warn_unpinned, subdirectory)
        self.provider = provider

    def to_dict(self) -> Dict[str, str]:
        git_dict = super().to_dict()
        git_dict["private"] = git_dict.pop("git")
        if self.provider:
            git_dict["provider"] = self.provider
        return git_dict


class PrivateUnpinnedPackage(GitUnpinnedPackage):
    def __init__(
        self,
        git: str,
        git_unrendered: str,
        revisions: List[str],
        warn_unpinned: bool = True,
        subdirectory: Optional[str] = None,
        provider: Optional[str] = None,
    ) -> None:
        super().__init__(git, git_unrendered, revisions, warn_unpinned, subdirectory)
        self.provider = provider

    def resolved(self) -> PrivatePinnedPackage:
        git_pinned = super().resolved()
        return PrivatePinnedPackage(
            git=git_pinned.git,
            git_unrendered=git_pinned.git_unrendered,
            revision=git_pinned.revision,
            warn_unpinned=git_pinned.warn_unpinned,
            subdirectory=git_pinned.subdirectory,
            provider=self.provider,
        )

    def incorporate(self, other: "PrivateUnpinnedPackage") -> "PrivateUnpinnedPackage":  # type: ignore[override]
        # This is being used to handle duplicate packages.
        # See TestSimpleDependencyWithDuplicates in functional tests.
        warn_unpinned = self.warn_unpinned and other.warn_unpinned
        return PrivateUnpinnedPackage(
            git=self.git,
            git_unrendered=self.git_unrendered,
            revisions=self.revisions + other.revisions,
            warn_unpinned=warn_unpinned,
            subdirectory=self.subdirectory,
            provider=self.provider,
        )

    @classmethod
    def from_contract(cls, contract: PrivatePackage) -> "PrivateUnpinnedPackage":  # type: ignore[override]
        git_url = get_private_package_helper().get_resolved_url(
            private_def=contract.private,
            provider=contract.provider,
        )
        return cls(
            git=git_url,
            git_unrendered=contract.private,
            revisions=contract.get_revisions(),
            warn_unpinned=contract.warn_unpinned is not False,
            subdirectory=contract.subdirectory,
            provider=contract.provider,
        )
