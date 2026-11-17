import hashlib
import io
import os
import tarfile
from argparse import Namespace
from types import SimpleNamespace
from unittest import mock

import pytest

from dbt.cli.flags import Flags
from dbt.cli.main import cli
from dbt.contracts.project import ProjectFlags, RegistryPackageMetadata
from dbt.deps.base import PinnedPackage
from dbt.deps.registry import RegistryPinnedPackage, select_hub_download
from dbt.exceptions import DependencyError
from dbt.flags import set_from_args

ORIGINAL_URL = "https://codeload.github.com/org/pkg/tar.gz/1.0.0"
HUB_MIRROR_URL = "https://mirror.example-cdn.com/packages/org/pkg/tar.gz/1.0.0"
UPSTREAM_SHA1 = "3333333333333333333333333333333333333333"
HUB_SHA1 = "1111111111111111111111111111111111111111"

_MISSING = object()


def _metadata(hub=_MISSING) -> RegistryPackageMetadata:
    downloads = {"tarball": ORIGINAL_URL, "format": "tgz", "sha1": UPSTREAM_SHA1}
    if hub is not _MISSING:
        downloads["hub"] = hub
    return RegistryPackageMetadata.from_dict(
        {
            "id": "org/pkg/1.0.0",
            "name": "pkg",
            "version": "1.0.0",
            "packages": [],
            "works_with": [],
            "downloads": downloads,
        }
    )


def _mirror(sha1=HUB_SHA1):
    hub = {"tarball": HUB_MIRROR_URL, "format": "tgz"}
    if sha1 is not None:
        hub["sha1"] = sha1
    return _metadata(hub)


MALFORMED_HUB = [None, {}, {"sha1": HUB_SHA1}, {"tarball": None, "sha1": HUB_SHA1}, "garbage", []]


class TestDownloadsContract:
    def test_without_hub_mirror(self):
        metadata = _metadata()
        assert metadata.downloads.tarball == ORIGINAL_URL
        assert metadata.downloads.hub is None

    def test_with_hub_mirror(self):
        metadata = _mirror()
        assert metadata.downloads.hub["tarball"] == HUB_MIRROR_URL
        assert metadata.downloads.hub["sha1"] == HUB_SHA1

    @pytest.mark.parametrize("hub", MALFORMED_HUB)
    def test_malformed_hub_mirror_still_parses(self, hub):
        assert _metadata(hub).downloads.tarball == ORIGINAL_URL


class TestSelectHubDownload:
    def test_unverified_uses_original_url_without_sha1(self):
        assert select_hub_download(_mirror(), False, "org/pkg@1.0.0") == (
            ORIGINAL_URL,
            None,
        )

    def test_verified_uses_hub_mirror_and_its_sha1(self):
        assert select_hub_download(_mirror(), True, "org/pkg@1.0.0") == (
            HUB_MIRROR_URL,
            HUB_SHA1,
        )

    def test_verified_without_hub_mirror_is_an_error(self):
        with pytest.raises(DependencyError, match="did not provide a verifiable mirror download"):
            select_hub_download(_metadata(), True, "org/pkg@1.0.0")

    def test_verified_hub_mirror_without_sha1_is_an_error(self):
        with pytest.raises(DependencyError, match="did not provide a sha1 checksum"):
            select_hub_download(_mirror(None), True, "org/pkg@1.0.0")

    @pytest.mark.parametrize("hub", MALFORMED_HUB)
    def test_malformed_hub_mirror_unverified_uses_original_url(self, hub):
        assert select_hub_download(_metadata(hub), False, "org/pkg@1.0.0") == (ORIGINAL_URL, None)

    @pytest.mark.parametrize("hub", MALFORMED_HUB)
    def test_malformed_hub_mirror_verified_is_an_error(self, hub):
        with pytest.raises(DependencyError, match="did not provide a verifiable mirror download"):
            select_hub_download(_metadata(hub), True, "org/pkg@1.0.0")

    def test_verified_non_string_sha1_is_an_error(self):
        with pytest.raises(DependencyError, match="did not provide a sha1 checksum"):
            select_hub_download(_mirror(12345), True, "org/pkg@1.0.0")

    @pytest.mark.parametrize("flag_value", [True, False])
    def test_registry_pinned_package_reads_flag(self, flag_value):
        set_from_args(Namespace(REQUIRE_HUB_VERIFIED_DOWNLOADS=flag_value), None)
        try:
            pkg = RegistryPinnedPackage("org/pkg", "1.0.0", "1.0.0")
            expected = (HUB_MIRROR_URL, HUB_SHA1) if flag_value else (ORIGINAL_URL, None)
            assert pkg._get_download(_mirror()) == expected
        finally:
            set_from_args(Namespace(WARN_ERROR=False), None)


class TestDownloadAndUntar:
    BODY = b"not really a tarball, but untar is mocked"

    @pytest.fixture
    def pkg(self):
        with mock.patch.object(PinnedPackage, "__abstractmethods__", frozenset()):
            yield PinnedPackage()

    def _fake_download(self, url, path):
        with open(path, "wb") as handle:
            handle.write(self.BODY)

    def _run(self, pkg, tmp_path, expected_sha1):
        tar_path = str(tmp_path / "pkg.tar.gz")
        with mock.patch(
            "dbt_common.clients.system.download", side_effect=self._fake_download
        ), mock.patch("dbt_common.clients.system.untar_package") as untar_package:
            pkg.download_and_untar(
                HUB_MIRROR_URL, tar_path, str(tmp_path / "dbt_packages"), "pkg", expected_sha1
            )
        return tar_path, untar_package

    def test_matching_sha1_extracts(self, pkg, tmp_path):
        sha1 = hashlib.sha1(self.BODY).hexdigest().upper()
        _, untar_package = self._run(pkg, tmp_path, sha1)
        untar_package.assert_called_once()

    def test_mismatched_sha1_fails_and_extracts_nothing(self, pkg, tmp_path):
        tar_path = str(tmp_path / "pkg.tar.gz")
        with mock.patch(
            "dbt_common.clients.system.download", side_effect=self._fake_download
        ), mock.patch("dbt_common.clients.system.untar_package") as untar_package:
            with pytest.raises(DependencyError, match="sha1 checksum mismatch"):
                pkg.download_and_untar(
                    HUB_MIRROR_URL, tar_path, str(tmp_path / "dbt_packages"), "pkg", "0" * 40
                )
        untar_package.assert_not_called()
        assert not os.path.exists(tar_path)

    def test_no_expected_sha1_skips_verification(self, pkg, tmp_path):
        _, untar_package = self._run(pkg, tmp_path, None)
        untar_package.assert_called_once()


class TestRequireHubVerifiedDownloadsFlag:
    def _flags(self, args, project_flags=None):
        parent = cli.make_context("dbt", ["deps"])
        ctx = cli.commands["deps"].make_context("deps", list(args[1:]), parent=parent)
        return Flags(ctx, project_flags or ProjectFlags())

    def test_default_is_off(self):
        assert self._flags(["deps"]).REQUIRE_HUB_VERIFIED_DOWNLOADS is False

    def test_cli_flag(self):
        flags = self._flags(["deps", "--require-hub-verified-downloads"])
        assert flags.REQUIRE_HUB_VERIFIED_DOWNLOADS is True

    def test_env_var(self, monkeypatch):
        monkeypatch.setenv("DBT_ENGINE_REQUIRE_HUB_VERIFIED_DOWNLOADS", "true")
        assert self._flags(["deps"]).REQUIRE_HUB_VERIFIED_DOWNLOADS is True

    def test_project_flag(self):
        flags = self._flags(["deps"], ProjectFlags(require_hub_verified_downloads=True))
        assert flags.REQUIRE_HUB_VERIFIED_DOWNLOADS is True

    def test_cli_no_overrides_project_flag(self):
        flags = self._flags(
            ["deps", "--no-require-hub-verified-downloads"],
            ProjectFlags(require_hub_verified_downloads=True),
        )
        assert flags.REQUIRE_HUB_VERIFIED_DOWNLOADS is False

    def test_env_false_overrides_project_flag(self, monkeypatch):
        monkeypatch.setenv("DBT_ENGINE_REQUIRE_HUB_VERIFIED_DOWNLOADS", "false")
        flags = self._flags(["deps"], ProjectFlags(require_hub_verified_downloads=True))
        assert flags.REQUIRE_HUB_VERIFIED_DOWNLOADS is False


def _package_tarball() -> bytes:
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as tar:
        root = tarfile.TarInfo("org-pkg-1.0.0")
        root.type = tarfile.DIRTYPE
        tar.addfile(root)
        content = b"name: pkg\nversion: '1.0.0'\n"
        info = tarfile.TarInfo("org-pkg-1.0.0/dbt_project.yml")
        info.size = len(content)
        tar.addfile(info, io.BytesIO(content))
    return buf.getvalue()


class TestRegistryInstallFlow:
    """RegistryPinnedPackage.install end to end, with only the HTTP calls faked."""

    TARBALL = _package_tarball()
    TARBALL_SHA1 = hashlib.sha1(TARBALL).hexdigest()

    @pytest.fixture
    def install(self, tmp_path):
        self.downloaded = []

        def fake_download(url, path, timeout=None):
            self.downloaded.append(url)
            with open(path, "wb") as handle:
                handle.write(self.TARBALL)

        def run(hub, require_verified):
            metadata = _metadata(hub).to_dict()
            project = SimpleNamespace(packages_install_path=str(tmp_path / "dbt_packages"))
            set_from_args(Namespace(REQUIRE_HUB_VERIFIED_DOWNLOADS=require_verified), None)
            try:
                with mock.patch(
                    "dbt.clients.registry.package_version", return_value=metadata
                ), mock.patch(
                    "dbt_common.clients.system.download", side_effect=fake_download
                ), mock.patch(
                    "dbt.deps.base.get_downloads_path", return_value=str(tmp_path / "downloads")
                ):
                    RegistryPinnedPackage("org/pkg", "1.0.0", "1.0.0").install(project, None)
            finally:
                set_from_args(Namespace(WARN_ERROR=False), None)
            return self.downloaded

        return run

    def _installed(self, tmp_path):
        return (tmp_path / "dbt_packages" / "pkg" / "dbt_project.yml").exists()

    def test_flag_off_installs_from_upstream(self, install, tmp_path):
        mirror = {"tarball": HUB_MIRROR_URL, "sha1": self.TARBALL_SHA1}
        assert install(mirror, False) == [ORIGINAL_URL]
        assert self._installed(tmp_path)

    def test_flag_off_ignores_malformed_mirror(self, install, tmp_path):
        assert install({"format": "tgz"}, False) == [ORIGINAL_URL]
        assert self._installed(tmp_path)

    def test_flag_on_installs_verified_mirror(self, install, tmp_path):
        mirror = {"tarball": HUB_MIRROR_URL, "sha1": self.TARBALL_SHA1}
        assert install(mirror, True) == [HUB_MIRROR_URL]
        assert self._installed(tmp_path)

    def test_flag_on_sha1_mismatch_fails_once_without_installing(self, install, tmp_path):
        mirror = {"tarball": HUB_MIRROR_URL, "sha1": "0" * 40}
        with pytest.raises(DependencyError, match="sha1 checksum mismatch"):
            install(mirror, True)
        assert self.downloaded == [HUB_MIRROR_URL]
        assert not self._installed(tmp_path)

    def test_flag_on_missing_mirror_fails_before_download(self, install, tmp_path):
        with pytest.raises(DependencyError, match="did not provide a verifiable mirror download"):
            install(_MISSING, True)
        assert self.downloaded == []
        assert not self._installed(tmp_path)
