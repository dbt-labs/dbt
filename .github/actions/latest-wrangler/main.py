import os
from typing import List

import requests
from packaging.version import Version, parse


def main():
    package_name: str = os.environ["INPUT_PACKAGE_NAME"]
    new_version: Version = parse(os.environ["INPUT_NEW_VERSION"])
    github_token: str = os.environ["INPUT_GITHUB_TOKEN"]

    response = _package_metadata(package_name, github_token)
    published_versions = _published_versions(response)
    new_version_tags = _new_version_tags(new_version, published_versions)
    _register_tags(new_version_tags, package_name)


def _package_metadata(package_name: str, github_token: str) -> requests.Response:
    url = f"https://api.github.com/orgs/dbt-labs/packages/container/{package_name}/versions"
    response = requests.get(url, auth=("", github_token))
    response.raise_for_status()
    return response


def _published_versions(response: requests.Response) -> List[Version]:
    package_metadata = response.json()
    return [
        parse(tag)
        for version in package_metadata
        for tag in version["metadata"]["container"]["tags"]
        if "latest" not in tag
    ]


def _new_version_tags(new_version: Version, published_versions: List[Version]) -> List[str]:
    # the package version is always a tag
    tags = [str(new_version)]

    # pre-releases don't get tagged with `latest`
    if new_version.is_prerelease:
        return tags

    if new_version > max(published_versions):
        tags.append("latest")

    published_patches = [
        version
        for version in published_versions
        if version.major == new_version.major and version.minor == new_version.minor
    ]
    if new_version > max(published_patches):
        tags.append(f"{new_version.major}.{new_version.minor}.latest")

    return tags


def _register_tags(tags: List[str], package_name: str) -> None:
    fully_qualified_tags = ",".join([f"ghcr.io/dbt-labs/{package_name}:{tag}" for tag in tags])
    github_output = os.environ.get("GITHUB_OUTPUT")
    with open(github_output, "at", encoding="utf-8") as gh_output:
        gh_output.write(f"fully_qualified_tags={fully_qualified_tags}")


if __name__ == "__main__":
    main()
