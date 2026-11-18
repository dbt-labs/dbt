from unittest.mock import Mock, patch

from dbt.profiler import profiler


def test_profiler_records_stats_to_requested_outfile():
    profile = Mock()
    stats = Mock()

    with (
        patch("dbt.profiler.Profile", return_value=profile),
        patch("dbt.profiler.Stats", return_value=stats),
    ):
        with profiler("timing.prof"):
            pass

    profile.enable.assert_called_once_with()
    profile.disable.assert_called_once_with()
    stats.sort_stats.assert_called_once_with("tottime")
    stats.dump_stats.assert_called_once_with("timing.prof")
