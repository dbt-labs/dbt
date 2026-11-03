import os
from unittest import mock

import pytest

from dbt.task import deps
from dbt.task.deps import (
    CACHEDIR_TAG_FILE_NAME,
    CACHEDIR_TAG_SIGNATURE,
    _has_cachedir_tag,
    _warn_if_removing_unowned_dir,
    _write_cachedir_tag,
)
from dbt_common.events.base_types import WarnLevel


def _write_tag(path, contents):
    with open(os.path.join(path, CACHEDIR_TAG_FILE_NAME), "w") as f:
        f.write(contents)


class TestHasCachedirTag:
    def test_missing_tag(self, tmp_path):
        assert not _has_cachedir_tag(str(tmp_path))

    def test_invalid_signature(self, tmp_path):
        _write_tag(tmp_path, "Signature: 1234")
        assert not _has_cachedir_tag(str(tmp_path))

    def test_empty_tag(self, tmp_path):
        _write_tag(tmp_path, "")
        assert not _has_cachedir_tag(str(tmp_path))

    def test_symlinked_tag(self, tmp_path):
        target = tmp_path / "real.tag"
        target.write_text(CACHEDIR_TAG_SIGNATURE)
        install_dir = tmp_path / "dbt_packages"
        install_dir.mkdir()
        os.symlink(target, install_dir / CACHEDIR_TAG_FILE_NAME)
        assert not _has_cachedir_tag(str(install_dir))

    def test_valid_tag(self, tmp_path):
        _write_tag(tmp_path, CACHEDIR_TAG_SIGNATURE + "\n# comment\n")
        assert _has_cachedir_tag(str(tmp_path))


class TestWarnIfRemovingUnownedDir:
    @pytest.fixture
    def fired(self):
        with mock.patch.object(deps, "fire_event") as fire_event:
            yield fire_event

    def test_warns_when_non_empty_without_tag(self, tmp_path, fired):
        (tmp_path / "something.txt").write_text("x")
        _warn_if_removing_unowned_dir(str(tmp_path))
        fired.assert_called_once()
        event = fired.call_args.args[0]
        assert str(tmp_path) in event.msg
        # Must not be a warning event, or `--warn-error` would make it fatal
        assert not isinstance(event, WarnLevel)

    def test_warns_when_tag_invalid(self, tmp_path, fired):
        _write_tag(tmp_path, "Signature: 1234")
        _warn_if_removing_unowned_dir(str(tmp_path))
        fired.assert_called_once()

    def test_no_warning_when_empty(self, tmp_path, fired):
        _warn_if_removing_unowned_dir(str(tmp_path))
        fired.assert_not_called()

    def test_no_warning_when_valid_tag(self, tmp_path, fired):
        _write_tag(tmp_path, CACHEDIR_TAG_SIGNATURE)
        (tmp_path / "some_package").mkdir()
        _warn_if_removing_unowned_dir(str(tmp_path))
        fired.assert_not_called()

    def test_no_warning_when_path_missing(self, tmp_path, fired):
        _warn_if_removing_unowned_dir(str(tmp_path / "missing"))
        fired.assert_not_called()


def test_write_cachedir_tag_is_recognised(tmp_path):
    _write_cachedir_tag(str(tmp_path))
    assert _has_cachedir_tag(str(tmp_path))
