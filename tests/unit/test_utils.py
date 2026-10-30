import os
import unittest

import pytest

import dbt.exceptions
import dbt.utils


class TestMultiDict(unittest.TestCase):
    def test_one_member(self):
        dct = {"a": 1, "b": 2, "c": 3}
        md = dbt.utils.MultiDict([dct])
        assert len(md) == 3
        for key in "abc":
            assert key in md
        assert md["a"] == 1
        assert md["b"] == 2
        assert md["c"] == 3

    def test_two_members_no_overlap(self):
        first = {"a": 1, "b": 2, "c": 3}
        second = {"d": 1, "e": 2, "f": 3}
        md = dbt.utils.MultiDict([first, second])
        assert len(md) == 6
        for key in "abcdef":
            assert key in md
        assert md["a"] == 1
        assert md["b"] == 2
        assert md["c"] == 3
        assert md["d"] == 1
        assert md["e"] == 2
        assert md["f"] == 3

    def test_two_members_overlap(self):
        first = {"a": 1, "b": 2, "c": 3}
        second = {"c": 1, "d": 2, "e": 3}
        md = dbt.utils.MultiDict([first, second])
        assert len(md) == 5
        for key in "abcde":
            assert key in md
        assert md["a"] == 1
        assert md["b"] == 2
        assert md["c"] == 1
        assert md["d"] == 2
        assert md["e"] == 3


class TestHumanizeExecutionTime(unittest.TestCase):
    def test_humanzing_execution_time_with_integer(self):

        result = dbt.utils.humanize_execution_time(execution_time=9460)

        assert result == " in 2 hours 37 minutes and 40.00 seconds"

    def test_humanzing_execution_time_with_two_decimal_place_float(self):

        result = dbt.utils.humanize_execution_time(execution_time=0.32)

        assert result == " in 0 hours 0 minutes and 0.32 seconds"

    def test_humanzing_execution_time_with_four_decimal_place_float(self):

        result = dbt.utils.humanize_execution_time(execution_time=0.3254)

        assert result == " in 0 hours 0 minutes and 0.33 seconds"


class CustomError(Exception):
    pass


class TestResolvePathWithinBase:
    def test_relative_path_inside_base(self, tmp_path):
        base = str(tmp_path)
        result = dbt.utils.resolve_path_within_base(
            "sub/dir", base, "some-field", error_cls=CustomError
        )
        assert result == os.path.join(base, "sub/dir")

    def test_absolute_path_inside_base_is_fine(self, tmp_path):
        base = str(tmp_path)
        inside = os.path.join(base, "sub/dir")
        result = dbt.utils.resolve_path_within_base(
            inside, base, "some-field", error_cls=CustomError
        )
        assert result == inside

    def test_relative_traversal_is_rejected(self, tmp_path):
        base = str(tmp_path)
        with pytest.raises(CustomError, match="must resolve to a path inside"):
            dbt.utils.resolve_path_within_base(
                "../escaped", base, "some-field", error_cls=CustomError
            )

    def test_absolute_path_outside_base_is_rejected(self, tmp_path):
        base = str(tmp_path / "project")
        outside = str(tmp_path / "other")
        with pytest.raises(CustomError, match="must resolve to a path inside"):
            dbt.utils.resolve_path_within_base(outside, base, "some-field", error_cls=CustomError)

    def test_symlink_escape_is_rejected(self, tmp_path):
        base = tmp_path / "project"
        base.mkdir()
        outside = tmp_path / "outside"
        outside.mkdir()
        link = base / "escape_link"
        link.symlink_to(outside)
        with pytest.raises(CustomError, match="must resolve to a path inside"):
            dbt.utils.resolve_path_within_base(
                "escape_link/payload", str(base), "some-field", error_cls=CustomError
            )

    def test_extended_length_prefix_is_ignored(self, tmp_path, monkeypatch):
        base = str(tmp_path)
        real = os.path.realpath

        def fake_realpath(p):
            resolved = real(p)
            return "\\\\?\\" + resolved if p != base else resolved

        monkeypatch.setattr(os.path, "realpath", fake_realpath)
        result = dbt.utils.resolve_path_within_base(
            "sub/file.sql", base, "some-field", error_cls=CustomError
        )
        assert result == os.path.join(base, "sub/file.sql")
