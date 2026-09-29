from dbt_common.events.functions import fire_event
from dbt_common.events.types import Note

# Width of the "HH:MM:SS  " prefix the default stdout logger puts on the first line
# of an event. Continuation lines get no prefix, so they are padded to line up under
# the message text. This message always renders through that logger -- `--version` is
# an eager click option, so it fires before setup_event_logger() applies --log-format.
_LOG_PREFIX_WIDTH = 10

_INSTALL_URL = "https://docs.getdbt.com/docs/local/install-dbt?utm_source=dbt-cli"
_UPGRADE_URL = (
    "https://docs.getdbt.com/docs/dbt-versions/dbt-upgrade/upgrading-to-v2?utm_source=dbt-cli"
)

# dbt v2 is the default experience for new installs. This message points net-new
# installs of dbt Core v1 (very often automated ones) at the v2 install docs, and
# existing v1 projects at the upgrade guide.
V2_AVAILABLE_MSG = "\n".join(
    [
        "dbt v2 is available, and is the default for new installations. This is dbt Core v1.",
        f"{' ' * _LOG_PREFIX_WIDTH}New project - install v2: {_INSTALL_URL}",
        f"{' ' * _LOG_PREFIX_WIDTH}Existing v1 project - upgrade guide: {_UPGRADE_URL}",
    ]
)


def announce_v2_available() -> None:
    """Notify that dbt v2 exists, from user-facing entry points like `--version`."""
    fire_event(Note(msg=V2_AVAILABLE_MSG))
