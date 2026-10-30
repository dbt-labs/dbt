from dbt.tracking import track_deprecated_version_invocation
from dbt.version import get_installed_version
from dbt_common.events.base_types import EventLevel
from dbt_common.events.functions import fire_event
from dbt_common.events.types import Note
from dbt_common.ui import warning_tag

# dbt Core 1.10 and older no longer receive patches. See
# https://github.com/dbt-labs/dbt-core/issues/15694
LAST_SUPPORTED_MINOR_VERSION = 11

WARN_MSG = (
    "This version of dbt is deprecated and no longer receives regular patches, including for "
    "known bugs. Please upgrade to a newer supported version of dbt for new projects: "
    "https://docs.getdbt.com/docs/dbt-versions?utm_source=dbt-cli"
)
INFO_MSG = (
    "This version of dbt is deprecated and no longer receives regular patches, including for "
    "known bugs. We recommend upgrading to a newer supported version of dbt: "
    "https://docs.getdbt.com/docs/dbt-versions"
)


def is_deprecated_version() -> bool:
    installed = get_installed_version()
    if installed.major is None or installed.minor is None:
        return False
    return (int(installed.major), int(installed.minor)) < (1, LAST_SUPPORTED_MINOR_VERSION)


def check_deprecated_version(is_warn: bool = False) -> None:
    """Notify and record telemetry if the installed dbt version is deprecated.

    Callers that gate a user-facing entry point (first install, `dbt init`, `--version`)
    should pass is_warn=True per the WARNING rows in
    https://github.com/dbt-labs/dbt-core/issues/15694; everything else (regular
    invocations) stays at INFO.
    """
    if not is_deprecated_version():
        return

    if is_warn:
        fire_event(Note(msg=warning_tag(WARN_MSG)), level=EventLevel.WARN)
    else:
        fire_event(Note(msg=INFO_MSG))

    track_deprecated_version_invocation()
