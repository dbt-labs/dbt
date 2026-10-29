from unittest import mock

import pytest

from dbt.clients import git
from dbt.exceptions import (
    CommandResultError,
    DbtRuntimeError,
    GitCheckoutError,
    GitCloningError,
    UnknownGitCloningProblemError,
)


@pytest.fixture(autouse=True)
def no_ambient_git_config(monkeypatch):
    monkeypatch.delenv("GIT_CONFIG_COUNT", raising=False)


class TestSplitCredentials:
    def test_token_only_is_username_with_empty_password(self):
        url, env = git._split_credentials("https://ghp_abc123@github.com/dbt-labs/repo.git")

        assert url == "https://github.com/dbt-labs/repo.git"
        assert env == {
            "GIT_CONFIG_COUNT": "2",
            "GIT_CONFIG_KEY_0": "credential.https://github.com.helper",
            "GIT_CONFIG_VALUE_0": "",
            "GIT_CONFIG_KEY_1": "credential.https://github.com.helper",
            "GIT_CONFIG_VALUE_1": git._CREDENTIAL_HELPER,
            "DBT_GIT_CREDENTIAL_USERNAME": "ghp_abc123",
            "DBT_GIT_CREDENTIAL_PASSWORD": "",
        }

    def test_user_and_password(self):
        url, env = git._split_credentials("https://user:s3cret@dev.azure.com/org/proj/_git/repo")

        assert url == "https://dev.azure.com/org/proj/_git/repo"
        assert env["GIT_CONFIG_KEY_1"] == "credential.https://dev.azure.com.helper"
        assert env["DBT_GIT_CREDENTIAL_USERNAME"] == "user"
        assert env["DBT_GIT_CREDENTIAL_PASSWORD"] == "s3cret"

    def test_percent_encoded_credentials_are_decoded(self):
        _, env = git._split_credentials("https://us%40er:p%2Fw%3A@example.com/repo.git")

        assert env["DBT_GIT_CREDENTIAL_USERNAME"] == "us@er"
        assert env["DBT_GIT_CREDENTIAL_PASSWORD"] == "p/w:"

    def test_port_is_kept_in_url_and_scope(self):
        url, env = git._split_credentials("https://tok@git.example.com:8443/team/repo.git")

        assert url == "https://git.example.com:8443/team/repo.git"
        assert env["GIT_CONFIG_KEY_1"] == "credential.https://git.example.com:8443.helper"

    def test_http_scheme_is_supported(self):
        url, env = git._split_credentials("http://tok@git.example.com/repo.git")

        assert url == "http://git.example.com/repo.git"
        assert env["GIT_CONFIG_KEY_1"] == "credential.http://git.example.com.helper"

    def test_helper_list_is_reset_before_ours(self):
        _, env = git._split_credentials("https://tok@github.com/dbt-labs/repo.git")

        assert env["GIT_CONFIG_VALUE_0"] == ""
        assert env["GIT_CONFIG_VALUE_1"] == git._CREDENTIAL_HELPER

    @pytest.mark.parametrize(
        "repo",
        [
            "https://github.com/dbt-labs/repo.git",
            "git@github.com:dbt-labs/repo.git",
            "ssh://git@github.com/dbt-labs/repo.git",
            "/local/path/to/repo",
        ],
    )
    def test_urls_without_http_credentials_are_unchanged(self, repo):
        assert git._split_credentials(repo) == (repo, {})

    @pytest.mark.parametrize("encoded", ["%0A", "%0D", "%00"])
    def test_credentials_that_would_corrupt_the_helper_protocol_are_left_alone(self, encoded):
        repo = f"https://tok{encoded}host=evil.com@github.com/dbt-labs/repo.git"

        assert git._split_credentials(repo) == (repo, {})

    def test_appends_to_existing_git_config_env(self, monkeypatch):
        monkeypatch.setenv("GIT_CONFIG_COUNT", "2")

        _, env = git._split_credentials("https://tok@github.com/dbt-labs/repo.git")

        assert env["GIT_CONFIG_COUNT"] == "4"
        assert env["GIT_CONFIG_KEY_2"] == "credential.https://github.com.helper"
        assert env["GIT_CONFIG_VALUE_3"] == git._CREDENTIAL_HELPER
        assert not {"GIT_CONFIG_KEY_0", "GIT_CONFIG_KEY_1"} & set(env)

    def test_invalid_existing_git_config_count_is_ignored(self, monkeypatch):
        monkeypatch.setenv("GIT_CONFIG_COUNT", "not-a-number")

        _, env = git._split_credentials("https://tok@github.com/dbt-labs/repo.git")

        assert env["GIT_CONFIG_COUNT"] == "2"
        assert env["GIT_CONFIG_KEY_0"] == "credential.https://github.com.helper"


class TestCredentialsDoNotReachCommandLine:
    @mock.patch("dbt.clients.git.run_cmd")
    def test_clone_uses_clean_url_and_credential_env(self, run_cmd):
        run_cmd.return_value = (b"", b"")

        git.clone("https://ghp_abc123@github.com/dbt-labs/repo.git", "/cwd", dirname="repo")

        cmd = run_cmd.call_args.args[1]
        env = run_cmd.call_args.kwargs["env"]
        assert "https://github.com/dbt-labs/repo.git" in cmd
        assert not any("ghp_abc123" in arg for arg in cmd)
        assert env["LC_ALL"] == "C"
        assert env["DBT_GIT_CREDENTIAL_USERNAME"] == "ghp_abc123"
        assert env["GIT_CONFIG_VALUE_1"] == git._CREDENTIAL_HELPER

    @mock.patch("dbt.clients.git.run_cmd")
    def test_clone_without_credentials_adds_no_git_config(self, run_cmd):
        run_cmd.return_value = (b"", b"")

        git.clone("https://github.com/dbt-labs/repo.git", "/cwd")

        assert run_cmd.call_args.kwargs["env"] == {"LC_ALL": "C"}

    @mock.patch("dbt.clients.git.list_tags", return_value=[])
    @mock.patch("dbt.clients.git.run_cmd")
    def test_fetch_gets_credential_env(self, run_cmd, _list_tags):
        run_cmd.return_value = (b"", b"")

        git._checkout("/cwd", "https://ghp_abc123@github.com/dbt-labs/repo.git", "main")

        fetches = [c for c in run_cmd.call_args_list if c.args[1][:2] == ["git", "fetch"]]
        assert len(fetches) == 1
        assert fetches[0].kwargs["env"]["DBT_GIT_CREDENTIAL_USERNAME"] == "ghp_abc123"
        assert not any("ghp_abc123" in " ".join(c.args[1]) for c in run_cmd.call_args_list)

    @mock.patch("dbt.clients.git.list_tags", return_value=[])
    @mock.patch("dbt.clients.git.run_cmd")
    def test_reset_gets_credential_env(self, run_cmd, _list_tags):
        # in a blobless subdirectory clone, reset fetches missing blobs from origin
        run_cmd.return_value = (b"", b"")

        git._checkout("/cwd", "https://ghp_abc123@github.com/dbt-labs/repo.git", "main")

        resets = [c for c in run_cmd.call_args_list if c.args[1][:2] == ["git", "reset"]]
        assert len(resets) == 1
        assert resets[0].kwargs["env"]["DBT_GIT_CREDENTIAL_USERNAME"] == "ghp_abc123"
        assert resets[0].kwargs["env"]["LC_ALL"] == "C"

    @mock.patch("dbt.clients.git.run_cmd")
    def test_sparse_checkout_gets_credential_env(self, run_cmd):
        run_cmd.return_value = (b"git version 2.55.0", b"")

        git.clone(
            "https://ghp_abc123@github.com/dbt-labs/repo.git",
            "/cwd",
            dirname="repo",
            subdirectory="sub",
        )

        sparse = [c for c in run_cmd.call_args_list if c.args[1][:2] == ["git", "sparse-checkout"]]
        assert len(sparse) == 1
        assert sparse[0].kwargs["env"]["DBT_GIT_CREDENTIAL_USERNAME"] == "ghp_abc123"


class TestUserValuesAreNotGitOptions:
    OPTION = "--upload-pack=touch /tmp/marker"

    @staticmethod
    def _assert_after_separator(cmd, value):
        assert "--" in cmd
        assert cmd.index("--") < cmd.index(value)

    @mock.patch("dbt.clients.git.run_cmd")
    def test_clone_url_and_dirname(self, run_cmd):
        run_cmd.return_value = (b"", b"")

        git.clone(self.OPTION, "/cwd", dirname=self.OPTION)

        cmd = run_cmd.call_args.args[1]
        self._assert_after_separator(cmd, self.OPTION)

    @mock.patch("dbt.clients.git.run_cmd")
    def test_sparse_checkout_subdirectory(self, run_cmd):
        run_cmd.return_value = (b"git version 2.55.0", b"")

        git.clone(
            "https://github.com/dbt-labs/repo.git", "/cwd", dirname="r", subdirectory=self.OPTION
        )

        sparse = [c for c in run_cmd.call_args_list if c.args[1][:2] == ["git", "sparse-checkout"]]
        self._assert_after_separator(sparse[0].args[1], self.OPTION)

    @mock.patch("dbt.clients.git.list_tags", return_value=[])
    @mock.patch("dbt.clients.git.run_cmd")
    def test_checkout_revision(self, run_cmd, _list_tags):
        run_cmd.return_value = (b"", b"")

        git._checkout("/cwd", "https://github.com/dbt-labs/repo.git", self.OPTION)

        for sub in ("fetch", "remote"):
            cmds = [c.args[1] for c in run_cmd.call_args_list if c.args[1][1] == sub]
            assert cmds
            for cmd in cmds:
                self._assert_after_separator(cmd, self.OPTION)


class TestCredentialsNotInErrors:
    REPO = "https://ghp_abc123@github.com/dbt-labs/repo.git"

    @staticmethod
    def _failure():
        return CommandResultError("/cwd", ["git"], 128, b"", b"fatal: nope")

    @mock.patch("dbt.clients.git.run_cmd")
    def test_clone_error(self, run_cmd):
        run_cmd.side_effect = self._failure()

        with pytest.raises(GitCloningError) as exc:
            git.clone(self.REPO, "/cwd")

        assert "ghp_abc123" not in str(exc.value)
        assert "https://github.com/dbt-labs/repo.git" in str(exc.value)

    @mock.patch("dbt.clients.git.run_cmd")
    def test_checkout_error(self, run_cmd):
        run_cmd.side_effect = self._failure()

        with pytest.raises(GitCheckoutError) as exc:
            git.checkout("/cwd", self.REPO, "main")

        assert "ghp_abc123" not in str(exc.value)

    @mock.patch("dbt.clients.git.clone")
    def test_unknown_clone_problem(self, clone):
        clone.side_effect = self._failure()

        with pytest.raises(UnknownGitCloningProblemError) as exc:
            git.clone_and_checkout(self.REPO, "/cwd")

        assert "ghp_abc123" not in str(exc.value)

    @mock.patch("dbt.clients.git.clone", return_value=(b"", b"unexpected"))
    def test_missing_cloning_into(self, _clone):
        with pytest.raises(DbtRuntimeError) as exc:
            git.clone_and_checkout(self.REPO, "/cwd")

        assert "ghp_abc123" not in str(exc.value)
