import os
from unittest import mock

import pytest

from dbt.tests.util import run_dbt
from dbt_common.clients import system

HUB_PACKAGES = {"packages": [{"package": "dbt-labs/dbt_utils", "version": "1.4.1"}]}


def _run_deps(args):
    with mock.patch("dbt_common.clients.system.download", wraps=system.download) as download:
        run_dbt(["deps", *args])
    return [call.args[0] for call in download.call_args_list]


def _assert_installed(project):
    assert os.path.exists(os.path.join(project.project_root, "dbt_packages", "dbt_utils"))


class TestHubVerifiedDownloadsCliFlag:
    @pytest.fixture(scope="class")
    def packages(self):
        return HUB_PACKAGES

    def test_installs_from_hub_mirror(self, project):
        urls = _run_deps(["--require-hub-verified-downloads"])
        assert urls and not any("codeload.github.com" in url for url in urls)
        _assert_installed(project)


class TestHubVerifiedDownloadsProjectFlag:
    @pytest.fixture(scope="class")
    def packages(self):
        return HUB_PACKAGES

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"flags": {"require_hub_verified_downloads": True}}

    def test_installs_from_hub_mirror(self, project):
        urls = _run_deps([])
        assert urls and not any("codeload.github.com" in url for url in urls)
        _assert_installed(project)


class TestHubVerifiedDownloadsOffByDefault:
    @pytest.fixture(scope="class")
    def packages(self):
        return HUB_PACKAGES

    def test_installs_from_upstream(self, project):
        urls = _run_deps([])
        assert urls and all("codeload.github.com" in url for url in urls)
        _assert_installed(project)
