import os
import unittest
from argparse import Namespace
from copy import deepcopy
from types import SimpleNamespace
from unittest import mock

import dbt.deps
import dbt.exceptions
from dbt.clients.registry import is_compatible_version
from dbt.config.project import PartialProject
from dbt.config.renderer import DbtProjectYamlRenderer
from dbt.contracts.project import (
    GitPackage,
    LocalPackage,
    PackageConfig,
    RegistryPackage,
    TarballPackage,
)
from dbt.deps.base import PinnedPackage
from dbt.deps.git import GitUnpinnedPackage
from dbt.deps.local import LocalPinnedPackage, LocalUnpinnedPackage
from dbt.deps.registry import RegistryUnpinnedPackage
from dbt.deps.resolver import resolve_packages
from dbt.deps.tarball import TarballUnpinnedPackage
from dbt.flags import set_from_args
from dbt.task.deps import DepsTask
from dbt.version import get_installed_version
from dbt_common.dataclass_schema import ValidationError
from dbt_common.semver import VersionSpecifier

set_from_args(Namespace(WARN_ERROR=False), None)


class TestLocalPackage(unittest.TestCase):
    def test_init(self):
        a_contract = LocalPackage.from_dict({"local": "/path/to/package"})
        self.assertEqual(a_contract.local, "/path/to/package")
        a = LocalUnpinnedPackage.from_contract(a_contract)
        self.assertEqual(a.local, "/path/to/package")
        a_pinned = a.resolved()
        self.assertEqual(a_pinned.local, "/path/to/package")
        self.assertEqual(str(a_pinned), "/path/to/package")


class TestTarballPackage(unittest.TestCase):
    class MockMetadata:
        name = "mock_metadata_name"

    @mock.patch("dbt.config.project.PartialProject.from_project_root")
    @mock.patch("os.listdir")
    @mock.patch("dbt.deps.tarball.get_downloads_path")
    @mock.patch("dbt_common.clients.system.untar_package")
    @mock.patch("dbt_common.clients.system.download")
    def test_fetch_metadata(
        self,
        mock_download,
        mock_untar_package,
        mock_get_downloads_path,
        mock_listdir,
        mock_from_project_root,
    ):
        mock_listdir.return_value = ["one_directory/"]
        mock_get_downloads_path.return_value = "downloads_path"
        mock_from_project_root.return_value = object()
        mock_from_project_root.return_value
        dict_well_formed_contract = {
            "tarball": "http://example.com/invalid_url@/package.tar.gz",
            "name": "my_package",
        }

        a_contract = TarballPackage.from_dict(dict_well_formed_contract)
        a = TarballUnpinnedPackage.from_contract(a_contract)

        a_pinned = a.resolved()
        with mock.patch.object(PartialProject, "from_project_root", return_value=PartialProject):
            with mock.patch.object(
                PartialProject, "render_package_metadata", return_value=self.MockMetadata
            ):
                metadata = a_pinned.fetch_metadata("", DbtProjectYamlRenderer())

        assert metadata == self.MockMetadata
        mock_download.assert_called_once_with(
            "http://example.com/invalid_url@/package.tar.gz", "downloads_path/my_package"
        )
        mock_untar_package.assert_called_once_with(
            "downloads_path/my_package", "downloads_path/my_package_untarred", "my_package"
        )

    @mock.patch("dbt.config.project.PartialProject.from_project_root")
    @mock.patch("os.listdir")
    @mock.patch("dbt.deps.tarball.get_downloads_path")
    @mock.patch("dbt_common.clients.system.untar_package")
    @mock.patch("dbt_common.clients.system.download")
    def test_fetch_metadata_fails_on_incorrect_tar_folder_structure(
        self,
        mock_download,
        mock_untar_package,
        mock_get_downloads_path,
        mock_listdir,
        mock_from_project_root,
    ):
        mock_listdir.return_value = ["one_directory/", "another_directory/"]

        mock_get_downloads_path.return_value = "downloads_path"
        mock_from_project_root.return_value = object()
        mock_from_project_root.return_value
        dict_well_formed_contract = {
            "tarball": "http://example.com/invalid_url@/package.tar.gz",
            "name": "my_package",
        }

        a_contract = TarballPackage.from_dict(dict_well_formed_contract)
        a = TarballUnpinnedPackage.from_contract(a_contract)

        a_pinned = a.resolved()
        with mock.patch.object(PartialProject, "from_project_root", return_value=PartialProject):
            with mock.patch.object(
                PartialProject, "render_package_metadata", return_value=self.MockMetadata
            ):
                with self.assertRaises(dbt.exceptions.DependencyError):
                    a_pinned.fetch_metadata("", DbtProjectYamlRenderer())

    @mock.patch("dbt.deps.tarball.get_downloads_path")
    def test_tarball_package_name_must_stay_in_downloads_dir(self, mock_get_downloads_path):
        mock_get_downloads_path.return_value = "downloads_path"
        for bad_name in ["/tmp/escaped", "../escaped", "a/../../escaped", "."]:
            contract = TarballPackage.from_dict(
                {"tarball": "http://example.com/package.tar.gz", "name": bad_name}
            )
            unpinned = TarballUnpinnedPackage.from_contract(contract)
            with self.assertRaises(dbt.exceptions.DependencyError):
                unpinned.resolved()

    @mock.patch("dbt.deps.tarball.get_downloads_path")
    def test_tarball_package_contract(self, mock_get_downloads_path):
        dict_well_formed_contract = {
            "tarball": "http://example.com/invalid_url@/package.tar.gz",
            "name": "my_cool_package",
        }
        a_contract = TarballPackage.from_dict(dict_well_formed_contract)

        # check contract and resolver
        self.assertEqual(a_contract.tarball, "http://example.com/invalid_url@/package.tar.gz")
        self.assertEqual(a_contract.name, "my_cool_package")

        a = TarballUnpinnedPackage.from_contract(a_contract)
        self.assertEqual(a.tarball, "http://example.com/invalid_url@/package.tar.gz")
        self.assertEqual(a.package, "my_cool_package")

        a_pinned = a.resolved()
        self.assertEqual(a_pinned.source_type(), "tarball")

        a_pinned_dict = a_pinned.to_dict()
        self.assertEqual(
            a_pinned_dict,
            {
                "tarball": "http://example.com/invalid_url@/package.tar.gz",
                "name": "my_cool_package",
            },
        )

    @mock.patch("dbt.deps.tarball.get_downloads_path")
    def test_tarball_pinned_package_contract_with_unrendered(self, mock_get_downloads_path):
        contract = TarballPackage(
            tarball="http://example.com/invalid_url@/package.tar.gz",
            name="my_cool_package",
            unrendered={"tarball": "tarball_unrendered"},
        )
        tarball_unpinned_package = TarballUnpinnedPackage.from_contract(contract)

        self.assertEqual(
            tarball_unpinned_package.tarball, "http://example.com/invalid_url@/package.tar.gz"
        )
        self.assertEqual(tarball_unpinned_package.package, "my_cool_package")
        self.assertEqual(tarball_unpinned_package.tarball_unrendered, "tarball_unrendered")

        tarball_pinned_package = tarball_unpinned_package.resolved()
        tarball_unpinned_package_dict = tarball_pinned_package.to_dict()
        self.assertEqual(
            tarball_unpinned_package_dict,
            {"tarball": "tarball_unrendered", "name": "my_cool_package"},
        )

    def test_tarball_package_contract_fails_on_no_name(self):
        from mashumaro.exceptions import MissingField

        # check bad contract (no name) fails
        a_contract = {"tarball": "http://example.com"}
        with self.assertRaises(MissingField):
            TarballPackage.from_dict(a_contract)


class TestGitPackage(unittest.TestCase):
    def test_init(self):
        a_contract = GitPackage.from_dict(
            {"git": "http://example.com", "revision": "0.0.1"},
        )
        self.assertEqual(a_contract.git, "http://example.com")
        self.assertEqual(a_contract.revision, "0.0.1")
        self.assertIs(a_contract.warn_unpinned, None)

        a = GitUnpinnedPackage.from_contract(a_contract)
        self.assertEqual(a.git, "http://example.com")
        self.assertEqual(a.revisions, ["0.0.1"])
        self.assertIs(a.warn_unpinned, True)

        a_pinned = a.resolved()
        self.assertEqual(a_pinned.name, "http://example.com")
        self.assertEqual(a_pinned.get_version(), "0.0.1")
        self.assertEqual(a_pinned.source_type(), "git")
        self.assertIs(a_pinned.warn_unpinned, True)

        a_pinned_dict = a_pinned.to_dict()
        self.assertEqual(a_pinned_dict, {"git": "http://example.com", "revision": "0.0.1"})

    def test_init_with_unrendered(self):
        contract = GitPackage(
            git="http://example.com", revision="0.0.1", unrendered={"git": "git_unrendered"}
        )

        git_unpinned_package = GitUnpinnedPackage.from_contract(contract)
        self.assertEqual(git_unpinned_package.git, "http://example.com")
        self.assertEqual(git_unpinned_package.revisions, ["0.0.1"])
        self.assertIs(git_unpinned_package.git_unrendered, "git_unrendered")

        git_pinned_package = git_unpinned_package.resolved()
        git_pinned_package_dict = git_pinned_package.to_dict()
        self.assertEqual(git_pinned_package_dict, {"git": "git_unrendered", "revision": "0.0.1"})

    @mock.patch("shutil.copytree")
    @mock.patch("dbt.deps.local.system.make_symlink")
    @mock.patch("dbt.deps.local.LocalPinnedPackage.get_installation_path")
    @mock.patch("dbt.deps.local.LocalPinnedPackage.resolve_path")
    def test_deps_install(
        self, mock_resolve_path, mock_get_installation_path, mock_symlink, mock_shutil
    ):
        mock_resolve_path.return_value = "/tmp/source"
        mock_get_installation_path.return_value = "/tmp/dest"
        mock_symlink.side_effect = OSError("Install deps symlink error")

        LocalPinnedPackage("local").install("dummy", "dummy")
        self.assertEqual(mock_shutil.call_count, 1)
        mock_shutil.assert_called_once_with("/tmp/source", "/tmp/dest")

    def test_invalid(self):
        with self.assertRaises(ValidationError):
            GitPackage.validate(
                {"git": "http://example.com", "version": "0.0.1"},
            )

    def test_resolve_ok(self):
        a_contract = GitPackage.from_dict(
            {"git": "http://example.com", "revision": "0.0.1"},
        )
        b_contract = GitPackage.from_dict(
            {"git": "http://example.com", "revision": "0.0.1", "warn-unpinned": False},
        )
        d_contract = GitPackage.from_dict(
            {"git": "http://example.com", "revision": "0.0.1", "subdirectory": "foo-bar"},
        )
        a = GitUnpinnedPackage.from_contract(a_contract)
        b = GitUnpinnedPackage.from_contract(b_contract)
        c = a.incorporate(b)
        d = GitUnpinnedPackage.from_contract(d_contract)

        self.assertTrue(a.warn_unpinned)
        self.assertFalse(b.warn_unpinned)
        self.assertTrue(d.warn_unpinned)

        c_pinned = c.resolved()
        self.assertEqual(c_pinned.name, "http://example.com")
        self.assertEqual(c_pinned.get_version(), "0.0.1")
        self.assertEqual(c_pinned.source_type(), "git")
        self.assertFalse(c_pinned.warn_unpinned)

        d_pinned = d.resolved()
        self.assertEqual(d_pinned.name, "http://example.com/foo-bar")
        self.assertEqual(d_pinned.get_version(), "0.0.1")
        self.assertEqual(d_pinned.source_type(), "git")
        self.assertEqual(d_pinned.subdirectory, "foo-bar")

    def test_resolve_fail(self):
        a_contract = GitPackage.from_dict(
            {"git": "http://example.com", "revision": "0.0.1"},
        )
        b_contract = GitPackage.from_dict(
            {"git": "http://example.com", "revision": "0.0.2"},
        )
        a = GitUnpinnedPackage.from_contract(a_contract)
        b = GitUnpinnedPackage.from_contract(b_contract)
        c = a.incorporate(b)
        self.assertEqual(c.git, "http://example.com")
        self.assertEqual(c.revisions, ["0.0.1", "0.0.2"])

        with self.assertRaises(dbt.exceptions.DependencyError):
            c.resolved()

    def test_default_revision(self):
        a_contract = GitPackage.from_dict({"git": "http://example.com"})
        self.assertEqual(a_contract.revision, None)
        self.assertIs(a_contract.warn_unpinned, None)

        a = GitUnpinnedPackage.from_contract(a_contract)
        self.assertEqual(a.git, "http://example.com")
        self.assertEqual(a.revisions, [])
        self.assertIs(a.warn_unpinned, True)

        a_pinned = a.resolved()
        self.assertEqual(a_pinned.name, "http://example.com")
        self.assertEqual(a_pinned.get_version(), "HEAD")
        self.assertEqual(a_pinned.source_type(), "git")
        self.assertIs(a_pinned.warn_unpinned, True)


class TestHubPackage(unittest.TestCase):
    def setUp(self):
        self.patcher = mock.patch("dbt.deps.registry.registry")
        self.registry = self.patcher.start()
        self.index_cached = self.registry.index_cached
        self.get_compatible_versions = self.registry.get_compatible_versions
        self.package_version = self.registry.package_version

        self.index_cached.return_value = [
            "dbt-labs-test/a",
        ]
        self.get_compatible_versions.return_value = ["0.1.2", "0.1.3", "0.1.4a1"]
        self.package_version.return_value = {
            "id": "dbt-labs-test/a/0.1.2",
            "name": "a",
            "version": "0.1.2",
            "packages": [],
            "_source": {
                "blahblah": "asdfas",
            },
            "downloads": {
                "tarball": "https://example.com/invalid-url!",
                "extra": "field",
            },
            "newfield": ["another", "value"],
        }

    def tearDown(self):
        self.patcher.stop()

    def test_init(self):
        a_contract = RegistryPackage(
            package="dbt-labs-test/a",
            version="0.1.2",
        )
        self.assertEqual(a_contract.package, "dbt-labs-test/a")
        self.assertEqual(a_contract.version, "0.1.2")

        a = RegistryUnpinnedPackage.from_contract(a_contract)
        self.assertEqual(a.package, "dbt-labs-test/a")
        self.assertEqual(
            a.versions,
            [
                VersionSpecifier(
                    build=None, major="0", matcher="=", minor="1", patch="2", prerelease=None
                )
            ],
        )

        a_pinned = a.resolved()
        self.assertEqual(a_contract.package, "dbt-labs-test/a")
        self.assertEqual(a_contract.version, "0.1.2")
        self.assertEqual(a_pinned.source_type(), "hub")

    def test_invalid(self):
        with self.assertRaises(ValidationError):
            RegistryPackage.validate(
                {"package": "namespace/name", "key": "invalid"},
            )

    def test_resolve_ok(self):
        a_contract = RegistryPackage(package="dbt-labs-test/a", version="0.1.2")
        b_contract = RegistryPackage(package="dbt-labs-test/a", version="0.1.2")
        a = RegistryUnpinnedPackage.from_contract(a_contract)
        b = RegistryUnpinnedPackage.from_contract(b_contract)
        c = a.incorporate(b)

        self.assertEqual(c.package, "dbt-labs-test/a")
        self.assertEqual(
            c.versions,
            [
                VersionSpecifier(
                    build=None,
                    major="0",
                    matcher="=",
                    minor="1",
                    patch="2",
                    prerelease=None,
                ),
                VersionSpecifier(
                    build=None,
                    major="0",
                    matcher="=",
                    minor="1",
                    patch="2",
                    prerelease=None,
                ),
            ],
        )

        c_pinned = c.resolved()
        self.assertEqual(c_pinned.package, "dbt-labs-test/a")
        self.assertEqual(c_pinned.version, "0.1.2")
        self.assertEqual(c_pinned.source_type(), "hub")

    def test_resolve_missing_package(self):
        a = RegistryUnpinnedPackage.from_contract(
            RegistryPackage(package="dbt-labs-test/b", version="0.1.2")
        )
        with self.assertRaises(dbt.exceptions.DependencyError) as exc:
            a.resolved()

        msg = "Package dbt-labs-test/b was not found in the package index"
        self.assertIn(msg, str(exc.exception))

    def test_resolve_missing_version(self):
        a = RegistryUnpinnedPackage.from_contract(
            RegistryPackage(package="dbt-labs-test/a", version="0.1.4")
        )

        with self.assertRaises(dbt.exceptions.DependencyError) as exc:
            a.resolved()

        # Check that key parts of the error message are present and avoid spacing failures
        error_msg = str(exc.exception)
        assert (
            "Could not find a matching compatible version for package dbt-labs-test/a" in error_msg
        )
        assert "Requested range: =0.1.4, =0.1.4" in error_msg
        assert "Compatible versions: ['0.1.2', '0.1.3']" in error_msg

    def test_resolve_conflict(self):
        a_contract = RegistryPackage(package="dbt-labs-test/a", version="0.1.2")
        b_contract = RegistryPackage(package="dbt-labs-test/a", version="0.1.3")
        a = RegistryUnpinnedPackage.from_contract(a_contract)
        b = RegistryUnpinnedPackage.from_contract(b_contract)
        c = a.incorporate(b)

        with self.assertRaises(dbt.exceptions.DependencyError) as exc:
            c.resolved()
        msg = (
            "Version error for package dbt-labs-test/a: Could not "
            "find a satisfactory version from options: ['=0.1.2', '=0.1.3']"
        )
        self.assertIn(msg, str(exc.exception))

    def test_dependency_error_inherits_from_dbt_runtime_error(self):
        """
        Test that DependencyError is a DbtRuntimeError (not plain Exception).

        Related to issue #12049 - DependencyError MUST inherit from DbtRuntimeError
        so that the CLI's exception handler (core/dbt/cli/requires.py line 186)
        catches it WITHOUT printing Python stack traces.

        The CLI has two exception handlers:
        - Line 186-188: Catches DbtBaseException -> NO stack trace (desired)
        - Line 189-192: Catches BaseException -> SHOWS stack trace (undesired)

        If DependencyError inherits from Exception, it's caught by the BaseException
        handler which prints full stack traces. If it inherits from DbtRuntimeError,
        it's caught by the DbtBaseException handler which shows clean error messages.

        This is an implementation test (checking inheritance) because the actual
        behavior (stack trace vs no stack trace) happens in the CLI layer, not here.
        """
        from dbt_common.exceptions import DbtRuntimeError

        # This is the critical assertion - DependencyError MUST be a DbtRuntimeError
        self.assertTrue(
            issubclass(dbt.exceptions.DependencyError, DbtRuntimeError),
            "DependencyError must inherit from DbtRuntimeError (not plain Exception) "
            "to avoid stack traces in CLI output. See issue #12049.",
        )

        # Verify this applies to actual raised exceptions from version conflicts
        a_contract = RegistryPackage(package="dbt-labs-test/a", version="0.1.2")
        b_contract = RegistryPackage(package="dbt-labs-test/a", version="0.1.3")
        a = RegistryUnpinnedPackage.from_contract(a_contract)
        b = RegistryUnpinnedPackage.from_contract(b_contract)
        c = a.incorporate(b)

        with self.assertRaises(dbt.exceptions.DependencyError) as exc:
            c.resolved()

        # The raised exception instance must be a DbtRuntimeError
        self.assertIsInstance(
            exc.exception,
            DbtRuntimeError,
            "Raised DependencyError must be instance of DbtRuntimeError",
        )

        # Verify the error message is still clean and user-friendly
        error_msg = str(exc.exception)
        self.assertIn("Version error for package dbt-labs-test/a", error_msg)
        self.assertIn("Could not find a satisfactory version", error_msg)

    def test_resolve_ranges(self):
        a_contract = RegistryPackage(package="dbt-labs-test/a", version="0.1.2")
        b_contract = RegistryPackage(package="dbt-labs-test/a", version="<0.1.4")
        a = RegistryUnpinnedPackage.from_contract(a_contract)
        b = RegistryUnpinnedPackage.from_contract(b_contract)
        c = a.incorporate(b)

        self.assertEqual(c.package, "dbt-labs-test/a")
        self.assertEqual(
            c.versions,
            [
                VersionSpecifier(
                    build=None,
                    major="0",
                    matcher="=",
                    minor="1",
                    patch="2",
                    prerelease=None,
                ),
                VersionSpecifier(
                    build=None,
                    major="0",
                    matcher="<",
                    minor="1",
                    patch="4",
                    prerelease=None,
                ),
            ],
        )

        c_pinned = c.resolved()
        self.assertEqual(c_pinned.package, "dbt-labs-test/a")
        self.assertEqual(c_pinned.version, "0.1.2")
        self.assertEqual(c_pinned.source_type(), "hub")

    def test_resolve_ranges_install_prerelease_default_false(self):
        a_contract = RegistryPackage(package="dbt-labs-test/a", version=">0.1.2")
        b_contract = RegistryPackage(package="dbt-labs-test/a", version="<0.1.5")
        a = RegistryUnpinnedPackage.from_contract(a_contract)
        b = RegistryUnpinnedPackage.from_contract(b_contract)
        c = a.incorporate(b)

        self.assertEqual(c.package, "dbt-labs-test/a")
        self.assertEqual(
            c.versions,
            [
                VersionSpecifier(
                    build=None,
                    major="0",
                    matcher=">",
                    minor="1",
                    patch="2",
                    prerelease=None,
                ),
                VersionSpecifier(
                    build=None,
                    major="0",
                    matcher="<",
                    minor="1",
                    patch="5",
                    prerelease=None,
                ),
            ],
        )

        c_pinned = c.resolved()
        self.assertEqual(c_pinned.package, "dbt-labs-test/a")
        self.assertEqual(c_pinned.version, "0.1.3")
        self.assertEqual(c_pinned.source_type(), "hub")

    def test_resolve_ranges_install_prerelease_true(self):
        a_contract = RegistryPackage(
            package="dbt-labs-test/a", version=">0.1.2", install_prerelease=True
        )
        b_contract = RegistryPackage(package="dbt-labs-test/a", version="<0.1.5")
        a = RegistryUnpinnedPackage.from_contract(a_contract)
        b = RegistryUnpinnedPackage.from_contract(b_contract)
        c = a.incorporate(b)

        self.assertEqual(c.package, "dbt-labs-test/a")
        self.assertEqual(
            c.versions,
            [
                VersionSpecifier(
                    build=None,
                    major="0",
                    matcher=">",
                    minor="1",
                    patch="2",
                    prerelease=None,
                ),
                VersionSpecifier(
                    build=None,
                    major="0",
                    matcher="<",
                    minor="1",
                    patch="5",
                    prerelease=None,
                ),
            ],
        )

        c_pinned = c.resolved()
        self.assertEqual(c_pinned.package, "dbt-labs-test/a")
        self.assertEqual(c_pinned.version, "0.1.4a1")
        self.assertEqual(c_pinned.source_type(), "hub")

    def test_get_version_latest_prelease_true(self):
        a_contract = RegistryPackage(
            package="dbt-labs-test/a", version=">0.1.0", install_prerelease=True
        )
        b_contract = RegistryPackage(package="dbt-labs-test/a", version="<0.1.4")
        a = RegistryUnpinnedPackage.from_contract(a_contract)
        b = RegistryUnpinnedPackage.from_contract(b_contract)
        c = a.incorporate(b)

        self.assertEqual(c.package, "dbt-labs-test/a")
        self.assertEqual(
            c.versions,
            [
                VersionSpecifier(
                    build=None,
                    major="0",
                    matcher=">",
                    minor="1",
                    patch="0",
                    prerelease=None,
                ),
                VersionSpecifier(
                    build=None,
                    major="0",
                    matcher="<",
                    minor="1",
                    patch="4",
                    prerelease=None,
                ),
            ],
        )

        c_pinned = c.resolved()
        self.assertEqual(c_pinned.package, "dbt-labs-test/a")
        self.assertEqual(c_pinned.version, "0.1.3")
        self.assertEqual(c_pinned.get_version_latest(), "0.1.4a1")
        self.assertEqual(c_pinned.source_type(), "hub")

    def test_get_version_latest_prelease_false(self):
        a_contract = RegistryPackage(
            package="dbt-labs-test/a", version=">0.1.0", install_prerelease=False
        )
        b_contract = RegistryPackage(package="dbt-labs-test/a", version="<0.1.4")
        a = RegistryUnpinnedPackage.from_contract(a_contract)
        b = RegistryUnpinnedPackage.from_contract(b_contract)
        c = a.incorporate(b)

        self.assertEqual(c.package, "dbt-labs-test/a")
        self.assertEqual(
            c.versions,
            [
                VersionSpecifier(
                    build=None,
                    major="0",
                    matcher=">",
                    minor="1",
                    patch="0",
                    prerelease=None,
                ),
                VersionSpecifier(
                    build=None,
                    major="0",
                    matcher="<",
                    minor="1",
                    patch="4",
                    prerelease=None,
                ),
            ],
        )

        c_pinned = c.resolved()
        self.assertEqual(c_pinned.package, "dbt-labs-test/a")
        self.assertEqual(c_pinned.version, "0.1.3")
        self.assertEqual(c_pinned.get_version_latest(), "0.1.3")
        self.assertEqual(c_pinned.source_type(), "hub")

    def test_get_version_prerelease_explicitly_requested(self):
        a_contract = RegistryPackage(
            package="dbt-labs-test/a", version="0.1.4a1", install_prerelease=None
        )

        a = RegistryUnpinnedPackage.from_contract(a_contract)

        self.assertEqual(a.package, "dbt-labs-test/a")
        self.assertEqual(
            a.versions,
            [
                VersionSpecifier(
                    build=None,
                    major="0",
                    matcher="=",
                    minor="1",
                    patch="4",
                    prerelease="a1",
                ),
            ],
        )

        a_pinned = a.resolved()
        self.assertEqual(a_pinned.package, "dbt-labs-test/a")
        self.assertEqual(a_pinned.version, "0.1.4a1")
        self.assertEqual(a_pinned.get_version_latest(), "0.1.4a1")
        self.assertEqual(a_pinned.source_type(), "hub")


class MockRegistry:
    def __init__(self, packages):
        self.packages = packages

    def index_cached(self, registry_base_url=None):
        return sorted(self.packages)

    def package(self, package_name, registry_base_url=None):
        try:
            pkg = self.packages[package_name]
        except KeyError:
            return []
        return pkg

    def get_compatible_versions(self, package_name, dbt_version, should_version_check):
        packages = self.package(package_name)
        return [
            pkg_version
            for pkg_version, info in packages.items()
            if is_compatible_version(info, dbt_version)
        ]

    def package_version(self, name, version):
        try:
            return self.packages[name][version]
        except KeyError:
            return None


class TestPackageSpec(unittest.TestCase):
    def setUp(self):
        dbt_version = get_installed_version()
        next_version = deepcopy(dbt_version)
        next_version.minor = str(int(next_version.minor) + 1)
        next_version.prerelease = None
        require_next_version = ">" + next_version.to_version_string()

        self.patcher = mock.patch("dbt.deps.registry.registry")
        self.registry = self.patcher.start()
        self.mock_registry = MockRegistry(
            packages={
                "dbt-labs-test/a": {
                    "0.1.2": {
                        "id": "dbt-labs-test/a/0.1.2",
                        "name": "a",
                        "version": "0.1.2",
                        "packages": [],
                        "_source": {
                            "blahblah": "asdfas",
                        },
                        "downloads": {
                            "tarball": "https://example.com/invalid-url!",
                            "extra": "field",
                        },
                        "newfield": ["another", "value"],
                    },
                    "0.1.3": {
                        "id": "dbt-labs-test/a/0.1.3",
                        "name": "a",
                        "version": "0.1.3",
                        "packages": [],
                        "_source": {
                            "blahblah": "asdfas",
                        },
                        "downloads": {
                            "tarball": "https://example.com/invalid-url!",
                            "extra": "field",
                        },
                        "newfield": ["another", "value"],
                    },
                    "0.1.4a1": {
                        "id": "dbt-labs-test/a/0.1.3a1",
                        "name": "a",
                        "version": "0.1.4a1",
                        "packages": [],
                        "_source": {
                            "blahblah": "asdfas",
                        },
                        "downloads": {
                            "tarball": "https://example.com/invalid-url!",
                            "extra": "field",
                        },
                        "newfield": ["another", "value"],
                    },
                    "0.2.0": {
                        "id": "dbt-labs-test/a/0.2.0",
                        "name": "a",
                        "version": "0.2.0",
                        "packages": [],
                        "_source": {
                            "blahblah": "asdfas",
                        },
                        # this one shouldn't be picked!
                        "require_dbt_version": require_next_version,
                        "downloads": {
                            "tarball": "https://example.com/invalid-url!",
                            "extra": "field",
                        },
                        "newfield": ["another", "value"],
                    },
                },
                "dbt-labs-test/b": {
                    "0.2.1": {
                        "id": "dbt-labs-test/b/0.2.1",
                        "name": "b",
                        "version": "0.2.1",
                        "packages": [{"package": "dbt-labs-test/a", "version": ">=0.1.3"}],
                        "_source": {
                            "blahblah": "asdfas",
                        },
                        "downloads": {
                            "tarball": "https://example.com/invalid-url!",
                            "extra": "field",
                        },
                        "newfield": ["another", "value"],
                    },
                },
            }
        )

        self.registry.index_cached.side_effect = self.mock_registry.index_cached
        self.registry.get_compatible_versions.side_effect = (
            self.mock_registry.get_compatible_versions
        )
        self.registry.package_version.side_effect = self.mock_registry.package_version

    def tearDown(self):
        self.patcher.stop()

    def test_dependency_resolution(self):
        package_config = PackageConfig.from_dict(
            {
                "packages": [
                    {"package": "dbt-labs-test/a", "version": ">0.1.2"},
                    {"package": "dbt-labs-test/b", "version": "0.2.1"},
                ],
            }
        )
        resolved = resolve_packages(
            package_config.packages, mock.MagicMock(project_name="test"), {}
        )
        self.assertEqual(len(resolved), 2)
        self.assertEqual(resolved[0].name, "dbt-labs-test/a")
        self.assertEqual(resolved[0].version, "0.1.3")
        self.assertEqual(resolved[1].name, "dbt-labs-test/b")
        self.assertEqual(resolved[1].version, "0.2.1")

    def test_private_package_resolves(self):
        # Regression sentinel: private: package must dispatch to PrivateUnpinnedPackage,
        # not raise a "git provider integration is missing" stub error.
        import dbt.deps.private_package as _pp_mod
        from dbt.deps.private_package import PrivateUnpinnedPackage
        from dbt.deps.resolver import PackageListing

        old_helper = _pp_mod.PRIVATE_PACKAGE_HELPER
        _pp_mod.PRIVATE_PACKAGE_HELPER = None
        os.environ["DBT_ENV_PRIVATE_GIT_PROVIDER_INFO"] = "[]"
        try:
            package_config = PackageConfig.from_dict(
                {
                    "packages": [
                        {"private": "dbt-labs-test/a", "subdirectory": "foo-bar"},
                    ],
                }
            )
            listing = PackageListing.from_contracts(package_config.packages)
            self.assertEqual(len(listing.packages), 1)
            pkg = next(iter(listing.packages.values()))
            self.assertIsInstance(pkg, PrivateUnpinnedPackage)
            self.assertEqual(pkg.git, "git@github.com:dbt-labs-test/a.git")
        finally:
            del os.environ["DBT_ENV_PRIVATE_GIT_PROVIDER_INFO"]
            _pp_mod.PRIVATE_PACKAGE_HELPER = old_helper

    def test_dependency_resolution_allow_prerelease(self):
        package_config = PackageConfig.from_dict(
            {
                "packages": [
                    {
                        "package": "dbt-labs-test/a",
                        "version": ">0.1.2",
                        "install_prerelease": True,
                    },
                    {"package": "dbt-labs-test/b", "version": "0.2.1"},
                ],
            }
        )
        resolved = resolve_packages(
            package_config.packages, mock.MagicMock(project_name="test"), {}
        )
        self.assertEqual(resolved[0].name, "dbt-labs-test/a")
        self.assertEqual(resolved[0].version, "0.1.4a1")

    def test_validation_error_when_version_is_missing_from_package_config(self):
        packages_data = {"packages": [{"package": "dbt-labs-test/b", "version": None}]}

        with self.assertRaises(ValidationError) as exc:
            PackageConfig.validate(data=packages_data)

        msg = "dbt-labs-test/b is missing the version. When installing from the Hub package index, version is a required property"
        assert msg in str(exc.exception)

    def test_validation_error_when_namespace_is_missing_from_package_config(self):
        packages_data = {"packages": [{"package": "dbt-labs", "version": "1.0.0"}]}

        with self.assertRaises(ValidationError) as exc:
            PackageConfig.validate(data=packages_data)

        msg = "dbt-labs was not found in the package index. Packages on the index require a namespace, e.g dbt-labs/dbt_utils"
        assert msg in str(exc.exception)


class TestCheckForDuplicatePackagesWithBooleans(unittest.TestCase):
    """Unit test for check_for_duplicate_packages method with boolean values.

    This is a regression test for issue #9104 where the method would fail with
    "TypeError: argument of type 'bool' is not iterable" when package entries
    contained boolean fields like warn-unpinned: false.
    """

    def test_check_duplicate_with_warn_unpinned_false(self):
        """Test that check_for_duplicate_packages handles warn-unpinned: false"""
        # Create a mock DepsTask with minimal setup
        mock_args = Namespace(
            add_package={"name": "audit_helper", "version": "0.9.0"}, source="hub"
        )

        # Mock project - we don't need a real one for this test
        with mock.patch("dbt.task.deps.BaseTask.__init__"):
            task = DepsTask.__new__(DepsTask)
            task.args = mock_args

        # Test data: packages.yml with warn-unpinned: false
        packages_yml = {
            "packages": [
                {
                    "git": "https://github.com/dbt-labs/dbt-utils.git",
                    "revision": "1.0.0",
                    "warn-unpinned": False,  # This is the problematic boolean
                },
            ]
        }

        # This should not raise TypeError
        result = task.check_for_duplicate_packages(packages_yml)

        # Verify the result is returned (not None)
        self.assertIsNotNone(result)
        self.assertIn("packages", result)
        # Original package should still be there (no duplicate)
        self.assertEqual(len(result["packages"]), 1)

    def test_check_duplicate_with_warn_unpinned_true(self):
        """Test that check_for_duplicate_packages handles warn-unpinned: true"""
        mock_args = Namespace(
            add_package={"name": "audit_helper", "version": "0.9.0"}, source="hub"
        )

        with mock.patch("dbt.task.deps.BaseTask.__init__"):
            task = DepsTask.__new__(DepsTask)
            task.args = mock_args

        packages_yml = {
            "packages": [
                {
                    "git": "https://github.com/dbt-labs/dbt-utils.git",
                    "revision": "1.0.0",
                    "warn-unpinned": True,  # Another boolean value
                },
            ]
        }

        # This should not raise TypeError
        result = task.check_for_duplicate_packages(packages_yml)
        self.assertIsNotNone(result)
        self.assertEqual(len(result["packages"]), 1)

    def test_check_duplicate_with_subdirectory_and_warn_unpinned(self):
        """Test with multiple non-string values (subdirectory string + warn-unpinned bool)"""
        mock_args = Namespace(
            add_package={"name": "audit_helper", "version": "0.9.0"}, source="hub"
        )

        with mock.patch("dbt.task.deps.BaseTask.__init__"):
            task = DepsTask.__new__(DepsTask)
            task.args = mock_args

        packages_yml = {
            "packages": [
                {
                    "git": "https://github.com/dbt-labs/dbt-utils.git",
                    "revision": "1.0.0",
                    "subdirectory": "some_dir",
                    "warn-unpinned": False,
                },
            ]
        }

        # This should not raise TypeError
        result = task.check_for_duplicate_packages(packages_yml)
        self.assertIsNotNone(result)
        self.assertEqual(len(result["packages"]), 1)

    def test_check_duplicate_detects_git_match(self):
        """Test that duplicate detection still works for git packages"""
        mock_args = Namespace(add_package={"name": "dbt-utils", "version": "1.1.0"}, source="git")

        with mock.patch("dbt.task.deps.BaseTask.__init__"):
            task = DepsTask.__new__(DepsTask)
            task.args = mock_args

        packages_yml = {
            "packages": [
                {
                    "git": "https://github.com/dbt-labs/dbt-utils.git",
                    "revision": "1.0.0",
                    "warn-unpinned": False,
                },
            ]
        }

        # The package name "dbt-utils" should match in the git URL
        with mock.patch("dbt_common.events.functions.fire_event"):
            result = task.check_for_duplicate_packages(packages_yml)

        # The duplicate should be removed
        self.assertIsNotNone(result)
        self.assertEqual(len(result["packages"]), 0)

    def test_check_duplicate_with_hub_package(self):
        """Test with hub package (which don't have warn-unpinned)"""
        mock_args = Namespace(
            add_package={"name": "another_package", "version": "1.0.0"}, source="hub"
        )

        with mock.patch("dbt.task.deps.BaseTask.__init__"):
            task = DepsTask.__new__(DepsTask)
            task.args = mock_args

        packages_yml = {
            "packages": [
                {
                    "package": "dbt-labs/dbt_utils",
                    "version": "1.0.0",
                },
            ]
        }

        # This should work fine
        result = task.check_for_duplicate_packages(packages_yml)
        self.assertIsNotNone(result)
        self.assertEqual(len(result["packages"]), 1)

    def test_check_duplicate_with_mixed_package_types(self):
        """Test with mixed package types (hub + git with warn-unpinned)"""
        mock_args = Namespace(
            add_package={"name": "audit_helper", "version": "0.9.0"}, source="hub"
        )

        with mock.patch("dbt.task.deps.BaseTask.__init__"):
            task = DepsTask.__new__(DepsTask)
            task.args = mock_args

        packages_yml = {
            "packages": [
                {
                    "package": "dbt-labs/dbt_utils",
                    "version": "1.0.0",
                },
                {
                    "git": "https://github.com/example/some-package.git",
                    "revision": "0.5.0",
                    "warn-unpinned": False,
                },
            ]
        }

        # This should not raise TypeError
        result = task.check_for_duplicate_packages(packages_yml)
        self.assertIsNotNone(result)
        # Both packages should remain (no duplicates)
        self.assertEqual(len(result["packages"]), 2)

    def test_check_duplicate_cross_source_hub_removes_git(self):
        """Test that adding a hub package removes a git package with the same name"""
        # When adding "dbt-labs/dbt_utils", it should match git URLs containing "dbt_utils"
        # Note: The full package name "dbt-labs/dbt_utils" is what gets checked
        mock_args = Namespace(
            add_package={"name": "dbt-labs/dbt_utils", "version": "1.0.0"}, source="hub"
        )

        with mock.patch("dbt.task.deps.BaseTask.__init__"):
            task = DepsTask.__new__(DepsTask)
            task.args = mock_args

        packages_yml = {
            "packages": [
                {
                    "git": "https://github.com/dbt-labs/dbt_utils.git",
                    "revision": "1.0.0",
                    "warn-unpinned": False,
                },
            ]
        }

        # The hub package name "dbt-labs/dbt_utils" should match in the git URL
        # because "dbt_utils" appears in both
        with mock.patch("dbt_common.events.functions.fire_event"):
            result = task.check_for_duplicate_packages(packages_yml)

        # The git package should be removed (cross-source duplicate)
        self.assertIsNotNone(result)
        self.assertEqual(len(result["packages"]), 0)

    def test_check_duplicate_multiple_matches(self):
        """Test that all duplicate packages are removed, not just the first one"""
        # When adding "dbt-labs/dbt_utils", it should match any identifier containing "dbt_utils"
        mock_args = Namespace(
            add_package={"name": "dbt-labs/dbt_utils", "version": "1.0.0"}, source="hub"
        )

        with mock.patch("dbt.task.deps.BaseTask.__init__"):
            task = DepsTask.__new__(DepsTask)
            task.args = mock_args

        packages_yml = {
            "packages": [
                {
                    "git": "https://github.com/dbt-labs/dbt_utils.git",
                    "revision": "0.9.0",
                },
                {
                    "git": "https://github.com/fivetran/dbt_amplitude",
                    "revision": "1.0.0",
                },
                {
                    "package": "dbt-labs/dbt_utils",  # Exact match
                    "version": "0.8.0",
                },
            ]
        }

        # Should remove both packages that contain "dbt_utils"
        with mock.patch("dbt_common.events.functions.fire_event"):
            result = task.check_for_duplicate_packages(packages_yml)

        # Only the dbt_amplitude package should remain
        self.assertIsNotNone(result)
        self.assertEqual(len(result["packages"]), 1)
        self.assertIn("dbt_amplitude", result["packages"][0]["git"])

    def test_check_duplicate_underscore_hyphen_matching(self):
        """Test that underscore and hyphen variants match (dbt_utils matches dbt-utils)"""
        # Adding hub package with underscore should match git package with hyphen
        mock_args = Namespace(
            add_package={"name": "dbt-labs/dbt_utils", "version": "1.0.0"}, source="hub"
        )

        with mock.patch("dbt.task.deps.BaseTask.__init__"):
            task = DepsTask.__new__(DepsTask)
            task.args = mock_args

        packages_yml = {
            "packages": [
                {
                    "git": "https://github.com/dbt-labs/dbt-utils.git",  # hyphen in URL
                    "revision": "1.0.0",
                },
            ]
        }

        # Should match because "dbt-utils" variant matches the git URL
        with mock.patch("dbt_common.events.functions.fire_event"):
            result = task.check_for_duplicate_packages(packages_yml)

        self.assertIsNotNone(result)
        self.assertEqual(len(result["packages"]), 0)  # Git package removed

    def test_check_duplicate_no_partial_word_match(self):
        """Test that partial word matches are rejected (dbt-core shouldn't match dbt-core-utils)"""
        mock_args = Namespace(
            add_package={"name": "dbt-labs/dbt-core", "version": "1.0.0"}, source="hub"
        )

        with mock.patch("dbt.task.deps.BaseTask.__init__"):
            task = DepsTask.__new__(DepsTask)
            task.args = mock_args

        packages_yml = {
            "packages": [
                {
                    "git": "https://github.com/dbt-labs/dbt-core-utils.git",
                    "revision": "1.0.0",
                },
                {
                    "package": "other-org/my-dbt-core-fork",
                    "version": "2.0.0",
                },
            ]
        }

        # Should NOT match because "dbt-core" is part of a larger word
        with mock.patch("dbt_common.events.functions.fire_event"):
            result = task.check_for_duplicate_packages(packages_yml)

        # Both packages should remain (no matches)
        self.assertIsNotNone(result)
        self.assertEqual(len(result["packages"]), 2)

    def test_check_duplicate_exact_word_boundary_match(self):
        """Test that exact matches with word boundaries work correctly"""
        mock_args = Namespace(
            add_package={"name": "dbt-labs/dbt-utils", "version": "1.0.0"}, source="hub"
        )

        with mock.patch("dbt.task.deps.BaseTask.__init__"):
            task = DepsTask.__new__(DepsTask)
            task.args = mock_args

        packages_yml = {
            "packages": [
                {
                    "git": "https://github.com/dbt-labs/dbt-utils.git",  # Should match
                    "revision": "1.0.0",
                },
                {
                    "git": "https://github.com/other/dbt-utils-extra.git",  # Should NOT match
                    "revision": "2.0.0",
                },
                {
                    "package": "dbt-labs/dbt_utils",  # Should match (underscore variant)
                    "version": "0.9.0",
                },
            ]
        }

        with mock.patch("dbt_common.events.functions.fire_event"):
            result = task.check_for_duplicate_packages(packages_yml)

        # Only dbt-utils-extra should remain
        self.assertIsNotNone(result)
        self.assertEqual(len(result["packages"]), 1)
        self.assertIn("dbt-utils-extra", result["packages"][0]["git"])


class TestGetInstallationPathTraversal(unittest.TestCase):
    """Regression: a malicious tarball/git package can set its `name` (parsed
    unsanitized from the dependency's own dbt_project.yml or packages.yml) to
    an absolute path or a path containing `..`. Since os.path.join discards
    the first argument when the second is absolute, get_installation_path
    must reject any resolved name that escapes packages_install_path instead
    of silently returning an attacker-controlled destination."""

    class FakePinnedPackage(PinnedPackage):
        def __init__(self, project_name):
            super().__init__()
            self._project_name = project_name

        @property
        def name(self):
            return self._project_name

        def source_type(self):
            return "fake"

        def get_version(self):
            return None

        def _fetch_metadata(self, project, renderer):
            return SimpleNamespace(name=self._project_name)

        def install(self, project, renderer):
            raise NotImplementedError

        def nice_version_name(self):
            return "1.0"

        def to_dict(self):
            return {}

    def _project(self):
        project = mock.Mock()
        project.packages_install_path = "/tmp/proj/dbt_packages"
        return project

    def test_normal_package_name_resolves_inside_install_path(self):
        pkg = self.FakePinnedPackage("my_package")
        path = pkg.get_installation_path(self._project(), None)
        self.assertEqual(path, "/tmp/proj/dbt_packages/my_package")

    def test_absolute_path_name_is_rejected(self):
        pkg = self.FakePinnedPackage("/etc/cron.d/evil")
        with self.assertRaises(dbt.exceptions.DependencyError):
            pkg.get_installation_path(self._project(), None)

    def test_parent_traversal_name_is_rejected(self):
        pkg = self.FakePinnedPackage("../../../../etc/cron.d/evil")
        with self.assertRaises(dbt.exceptions.DependencyError):
            pkg.get_installation_path(self._project(), None)

    def test_relative_traversal_within_bounds_is_allowed(self):
        # "./my_package" still resolves inside packages_install_path and
        # should not be rejected.
        pkg = self.FakePinnedPackage("./my_package")
        path = pkg.get_installation_path(self._project(), None)
        self.assertEqual(
            os.path.realpath(path),
            os.path.realpath("/tmp/proj/dbt_packages/my_package"),
        )


class TestInstallPathTraversalHubPackage(unittest.TestCase):
    """Regression: hub packages install via PinnedPackage._install, which builds
    its untar destination from get_project_name directly instead of going
    through get_installation_path. Assert _install also rejects a package
    name that resolves outside packages_install_path, before it ever
    downloads or untars anything."""

    class FakeRegistryPinnedPackage(PinnedPackage):
        def __init__(self, project_name):
            super().__init__()
            self._project_name = project_name
            self.package = project_name
            self.version = "1.0.0"

        @property
        def name(self):
            return self._project_name

        def source_type(self):
            return "hub"

        def get_version(self):
            return self.version

        def _fetch_metadata(self, project, renderer):
            return SimpleNamespace(
                name=self._project_name,
                downloads=SimpleNamespace(tarball="http://example.com/package.tar.gz"),
            )

        def install(self, project, renderer):
            self._install(project, renderer)

        def nice_version_name(self):
            return "1.0.0"

        def to_dict(self):
            return {}

    def _project(self):
        project = mock.Mock()
        project.packages_install_path = "/tmp/proj/dbt_packages"
        return project

    @mock.patch("dbt.deps.base.get_downloads_path", return_value="/tmp/downloads")
    @mock.patch("dbt_common.clients.system.make_directory")
    @mock.patch("dbt_common.clients.system.download")
    @mock.patch("dbt_common.clients.system.untar_package")
    def test_traversal_name_is_rejected_before_download(
        self, untar_package, download, make_directory, get_downloads_path
    ):
        pkg = self.FakeRegistryPinnedPackage("../../../../etc/cron.d/evil")
        with self.assertRaises(dbt.exceptions.DependencyError):
            pkg.install(self._project(), None)
        download.assert_not_called()
        untar_package.assert_not_called()
