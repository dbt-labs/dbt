"""Unit tests for ManifestLoader.check_for_model_deprecations (issue #12888).

These exercise the reference-deprecation warnings (I066 UpcomingReferenceDeprecation
/ I067 DeprecatedReference) that fire on a model which references a deprecated
model. They run without a database by calling check_for_model_deprecations
directly against a hand-built Manifest and capturing the events it emits.

make_model has no deprecation_date parameter, so deprecation_date is set on the
constructed nodes after creation (the shared helper is intentionally left alone).
"""

from datetime import datetime, timezone
from unittest import mock
from unittest.mock import patch

from dbt.parser.manifest import ManifestLoader
from tests.unit.utils.manifest import make_manifest, make_model

# is_past_deprecation_date compares against datetime.now().astimezone(), so these
# must be timezone-aware to avoid naive-vs-aware comparison errors.
PAST = datetime(2020, 1, 1, tzinfo=timezone.utc)
FUTURE = datetime(2099, 1, 1, tzinfo=timezone.utc)


def _capture_deprecation_events(manifest):
    """Run check_for_model_deprecations against `manifest`, returning the list of
    events it emitted. The method only touches self.manifest, so a stub loader
    carrying the manifest is sufficient — no DB or full ManifestLoader needed."""
    loader = mock.MagicMock()
    loader.manifest = manifest
    captured = []
    # On 1.11.latest the deprecation warnings are emitted via warn_or_error.
    with patch("dbt.parser.manifest.warn_or_error") as mock_warn:
        mock_warn.side_effect = lambda event, *args, **kwargs: captured.append(event)
        ManifestLoader.check_for_model_deprecations(loader)
    return captured


def _reference_warnings_for(events, model_name):
    return [
        e
        for e in events
        if type(e).__name__ in ("DeprecatedReference", "UpcomingReferenceDeprecation")
        and e.model_name == model_name
    ]


def _make_parent_and_child(child_deprecation_date):
    """A deprecated parent (`my_model`, past its date) and a child that ref()s it
    (`my_dependant_model`). The child's own deprecation_date is the variable."""
    parent = make_model("test", "my_model", "select 1 as id")
    parent.deprecation_date = PAST

    child = make_model(
        "test",
        "my_dependant_model",
        'select * from {{ ref("my_model") }}',
        refs=[parent],
    )
    child.deprecation_date = child_deprecation_date

    # Sanity: the child actually depends on the parent (so the parent->child edge
    # that drives the reference warning exists).
    assert parent.unique_id in child.depends_on_nodes
    # Sanity: the date predicate reflects what we set.
    assert parent.is_past_deprecation_date is True
    return parent, child


def test_deprecated_consumer_of_deprecated_reference_is_not_warned():
    """Case (a): a model that is itself already past its deprecation date should
    NOT be told to migrate off a deprecated dependency (#12888)."""
    parent, child = _make_parent_and_child(child_deprecation_date=PAST)
    assert child.is_past_deprecation_date is True

    events = _capture_deprecation_events(make_manifest(nodes=[parent, child]))

    assert _reference_warnings_for(events, "my_dependant_model") == []


def test_non_deprecated_consumer_still_warned():
    """Case (b): a non-deprecated model consuming a past-deprecated model should
    STILL receive the I067 DeprecatedReference warning (regression guard)."""
    parent, child = _make_parent_and_child(child_deprecation_date=None)
    assert child.is_past_deprecation_date is False

    events = _capture_deprecation_events(make_manifest(nodes=[parent, child]))

    warnings = _reference_warnings_for(events, "my_dependant_model")
    assert len(warnings) == 1
    assert type(warnings[0]).__name__ == "DeprecatedReference"
    assert warnings[0].ref_model_name == "my_model"


def test_future_deprecated_consumer_still_warned():
    """Case (c): a model with a FUTURE deprecation date consuming a past-deprecated
    model should STILL receive the I067 warning (locks the semantics)."""
    parent, child = _make_parent_and_child(child_deprecation_date=FUTURE)
    assert child.is_past_deprecation_date is False

    events = _capture_deprecation_events(make_manifest(nodes=[parent, child]))

    warnings = _reference_warnings_for(events, "my_dependant_model")
    assert len(warnings) == 1
    assert type(warnings[0]).__name__ == "DeprecatedReference"
    assert warnings[0].ref_model_name == "my_model"
