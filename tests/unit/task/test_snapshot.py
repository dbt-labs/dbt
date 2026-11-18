from dbt.task.snapshot import (
    SNAPSHOT_UNIQUE_KEY_SUGGESTION,
    _add_snapshot_unique_key_suggestion,
)


class FakeDbtException(Exception):
    def __init__(self, message: str):
        self.msg = message
        super().__init__(message)


def test_adds_unique_key_suggestion_for_duplicate_row_error():
    exc = Exception("Duplicate row detected during DML action")

    _add_snapshot_unique_key_suggestion(exc)

    assert SNAPSHOT_UNIQUE_KEY_SUGGESTION in str(exc)


def test_does_not_add_suggestion_for_unrelated_error():
    exc = Exception("connection timeout")

    _add_snapshot_unique_key_suggestion(exc)

    assert SNAPSHOT_UNIQUE_KEY_SUGGESTION not in str(exc)


def test_does_not_duplicate_unique_key_suggestion():
    exc = Exception(
        "Duplicate row detected during DML action\n\n" f"{SNAPSHOT_UNIQUE_KEY_SUGGESTION}"
    )

    _add_snapshot_unique_key_suggestion(exc)

    assert str(exc).count(SNAPSHOT_UNIQUE_KEY_SUGGESTION) == 1


def test_adds_unique_key_suggestion_to_msg_attribute():
    exc = FakeDbtException("UPDATE/MERGE must match at most one source row")

    _add_snapshot_unique_key_suggestion(exc)

    assert SNAPSHOT_UNIQUE_KEY_SUGGESTION in exc.msg
