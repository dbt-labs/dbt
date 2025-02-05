import os
import platform
import traceback
import uuid
from contextlib import contextmanager
from datetime import datetime
from typing import Optional

import pytz
import requests
from packaging.version import Version
from snowplow_tracker import Emitter, SelfDescribingJson, Subject, Tracker
from snowplow_tracker import __version__ as snowplow_version  # type: ignore
from snowplow_tracker import logger as sp_logger
from snowplow_tracker.events import StructuredEvent

from dbt import version as dbt_version
from dbt.adapters.exceptions import FailedToConnectError
from dbt.clients.yaml_helper import safe_load, yaml  # noqa:F401
from dbt.events.types import (
    DisableTracking,
    FlushEvents,
    FlushEventsFailure,
    MainEncounteredError,
    SendEventFailure,
    SendingEvent,
    TrackingInitializeFailure,
)
from dbt_common.events.base_types import EventMsg
from dbt_common.events.functions import fire_event, get_invocation_id, msg_to_dict
from dbt_common.exceptions import NotImplementedError

sp_logger.setLevel(100)

COLLECTOR_URL = "fishtownanalytics.sinter-collect.com"
COLLECTOR_PROTOCOL = "https"
DBT_INVOCATION_ENV = "DBT_INVOCATION_ENV"

ADAPTER_INFO_SPEC = "iglu:com.dbt/adapter_info/jsonschema/1-0-1"
DEPRECATION_WARN_SPEC = "iglu:com.dbt/deprecation_warn/jsonschema/1-0-0"
BEHAVIOR_CHANGE_WARN_SPEC = "iglu:com.dbt/behavior_change_warn/jsonschema/1-0-0"
EXPERIMENTAL_PARSER = "iglu:com.dbt/experimental_parser/jsonschema/1-0-0"
INVOCATION_ENV_SPEC = "iglu:com.dbt/invocation_env/jsonschema/1-0-0"
INVOCATION_SPEC = "iglu:com.dbt/invocation/jsonschema/1-0-2"
LOAD_ALL_TIMING_SPEC = "iglu:com.dbt/load_all_timing/jsonschema/1-0-3"
PACKAGE_INSTALL_SPEC = "iglu:com.dbt/package_install/jsonschema/1-0-0"
PARTIAL_PARSER = "iglu:com.dbt/partial_parser/jsonschema/1-0-1"
PLATFORM_SPEC = "iglu:com.dbt/platform/jsonschema/1-0-0"
PROJECT_ID_SPEC = "iglu:com.dbt/project_id/jsonschema/1-0-1"
RESOURCE_COUNTS = "iglu:com.dbt/resource_counts/jsonschema/1-0-1"
RPC_REQUEST_SPEC = "iglu:com.dbt/rpc_request/jsonschema/1-0-1"
RUNNABLE_TIMING = "iglu:com.dbt/runnable/jsonschema/1-0-0"
RUN_MODEL_SPEC = "iglu:com.dbt/run_model/jsonschema/1-1-0"
PLUGIN_GET_NODES = "iglu:com.dbt/plugin_get_nodes/jsonschema/1-0-0"

SNOWPLOW_TRACKER_VERSION = Version(snowplow_version)

# workaround in case real snowplow tracker is in the env
# the argument was renamed in https://github.com/snowplow/snowplow-python-tracker/commit/39fd50a3aff98a5efdd5c5c7fb5518fe4761305b
INIT_KW_ARGS = (
    {"buffer_size": 30} if SNOWPLOW_TRACKER_VERSION < Version("0.13.0") else {"batch_size": 30}
)


class TimeoutEmitter(Emitter):
    def __init__(self) -> None:
        super().__init__(
            COLLECTOR_URL,
            protocol=COLLECTOR_PROTOCOL,
            on_failure=self.handle_failure,
            method="post",
            # don't set this.
            byte_limit=None,
            **INIT_KW_ARGS,
        )

    @staticmethod
    def handle_failure(num_ok, unsent):
        # num_ok will always be 0, unsent will always be 1 entry long, because
        # the buffer is length 1, so not much to talk about
        fire_event(DisableTracking())
        disable_tracking()

    def _log_request(self, request, payload):
        sp_logger.info(f"Sending {request} request to {self.endpoint}...")
        sp_logger.debug(f"Payload: {payload}")

    def _log_result(self, request, status_code):
        msg = f"{request} request finished with status code: {status_code}"
        if self.is_good_status_code(status_code):
            sp_logger.info(msg)
        else:
            sp_logger.warning(msg)

    def http_post(self, payload):
        self._log_request("POST", payload)

        r = requests.post(
            self.endpoint,
            data=payload,
            headers={"content-type": "application/json; charset=utf-8"},
            timeout=5.0,
        )

        self._log_result("POST", r.status_code)
        return r

    def http_get(self, payload):
        self._log_request("GET", payload)

        r = requests.get(self.endpoint, params=payload, timeout=5.0)

        self._log_result("GET", r.status_code)
        return r


emitter = TimeoutEmitter()
tracker = Tracker(
    emitters=emitter,
    namespace="cf",
    app_id="dbt",
)


class User:
    def __init__(self, cookie_dir) -> None:
        self.do_not_track = True
        self.cookie_dir = cookie_dir

        self.id = None
        self.invocation_id = get_invocation_id()
        self.run_started_at = datetime.now(tz=pytz.utc)

    def state(self):
        return "do not track" if self.do_not_track else "tracking"

    @property
    def cookie_path(self):
        return os.path.join(self.cookie_dir, ".user.yml")

    def initialize(self):
        self.do_not_track = False

        cookie = self.get_cookie()
        self.id = cookie.get("id")

        subject = Subject()
        subject.set_user_id(self.id)
        tracker.set_subject(subject)

    def disable_tracking(self):
        self.do_not_track = True
        self.id = None
        self.cookie_dir = None
        tracker.set_subject(None)

    def set_cookie(self):
        # If the user points dbt to a profile directory which exists AND
        # contains a profiles.yml file, then we can set a cookie. If the
        # specified folder does not exist, or if there is not a profiles.yml
        # file in this folder, then an inconsistent cookie can be used. This
        # will change in every dbt invocation until the user points to a
        # profile dir file which contains a valid profiles.yml file.
        #
        # See: https://github.com/dbt-labs/dbt-core/issues/1645

        user = {"id": str(uuid.uuid4())}

        cookie_path = os.path.abspath(self.cookie_dir)
        profiles_file = os.path.join(cookie_path, "profiles.yml")
        if os.path.exists(cookie_path) and os.path.exists(profiles_file):
            with open(self.cookie_path, "w") as fh:
                yaml.dump(user, fh)

        return user

    def get_cookie(self):
        if not os.path.isfile(self.cookie_path):
            user = self.set_cookie()
        else:
            with open(self.cookie_path, "r") as fh:
                try:
                    user = safe_load(fh)
                    if user is None:
                        user = self.set_cookie()
                except yaml.reader.ReaderError:
                    user = self.set_cookie()
        return user


active_user: Optional[User] = None


def get_platform_context():
    data = {
        "platform": platform.platform(),
        "python": platform.python_version(),
        "python_version": platform.python_implementation(),
    }

    return SelfDescribingJson(PLATFORM_SPEC, data)


def get_dbt_env_context():
    default = "manual"

    dbt_invocation_env = os.getenv(DBT_INVOCATION_ENV, default)
    if dbt_invocation_env == "":
        dbt_invocation_env = default

    data = {
        "environment": dbt_invocation_env,
    }

    return SelfDescribingJson(INVOCATION_ENV_SPEC, data)


def track(user, *args, **kwargs):
    if user.do_not_track:
        return

    fire_event(SendingEvent(kwargs=str(kwargs)))
    try:
        tracker.track(StructuredEvent(*args, **kwargs))
    except Exception:
        fire_event(SendEventFailure())


def track_project_id(options):
    assert active_user is not None, "Cannot track project_id when active user is None"
    context = [SelfDescribingJson(PROJECT_ID_SPEC, options)]

    track(
        active_user,
        category="dbt",
        action="project_id",
        label=get_invocation_id(),
        context=context,
    )


def track_adapter_info(options):
    assert active_user is not None, "Cannot track adapter_info when active user is None"
    context = [SelfDescribingJson(ADAPTER_INFO_SPEC, options)]

    track(
        active_user,
        category="dbt",
        action="adapter_info",
        label=get_invocation_id(),
        context=context,
    )


def track_invocation_start(invocation_context):
    data = {"progress": "start", "result_type": None, "result": None}
    data.update(invocation_context)
    context = [
        SelfDescribingJson(INVOCATION_SPEC, data),
        get_platform_context(),
        get_dbt_env_context(),
    ]

    track(active_user, category="dbt", action="invocation", label="start", context=context)


def track_project_load(options):
    context = [SelfDescribingJson(LOAD_ALL_TIMING_SPEC, options)]
    assert active_user is not None, "Cannot track project loading time when active user is None"

    track(
        active_user,
        category="dbt",
        action="load_project",
        label=get_invocation_id(),
        context=context,
    )


def track_resource_counts(resource_counts):
    context = [SelfDescribingJson(RESOURCE_COUNTS, resource_counts)]
    assert active_user is not None, "Cannot track resource counts when active user is None"

    track(
        active_user,
        category="dbt",
        action="resource_counts",
        label=get_invocation_id(),
        context=context,
    )


def track_model_run(options):
    context = [SelfDescribingJson(RUN_MODEL_SPEC, options)]
    assert active_user is not None, "Cannot track model runs when active user is None"

    track(
        active_user, category="dbt", action="run_model", label=get_invocation_id(), context=context
    )


def track_rpc_request(options):
    context = [SelfDescribingJson(RPC_REQUEST_SPEC, options)]
    assert active_user is not None, "Cannot track rpc requests when active user is None"

    track(
        active_user,
        category="dbt",
        action="rpc_request",
        label=get_invocation_id(),
        context=context,
    )


def get_base_invocation_context():
    assert (
        active_user is not None
    ), "initialize active user before calling get_base_invocation_context"
    return {
        "project_id": None,
        "user_id": active_user.id,
        "invocation_id": active_user.invocation_id,
        "command": None,
        "options": None,
        "version": str(dbt_version.installed),
        "run_type": "regular",
        "adapter_type": None,
        "adapter_unique_id": None,
    }


def track_package_install(command_name: str, project_hashed_name: Optional[str], options):
    assert active_user is not None, "Cannot track package installs when active user is None"

    invocation_data = get_base_invocation_context()

    invocation_data.update({"project_id": project_hashed_name, "command": command_name})

    context = [
        SelfDescribingJson(INVOCATION_SPEC, invocation_data),
        SelfDescribingJson(PACKAGE_INSTALL_SPEC, options),
    ]

    track(
        active_user,
        category="dbt",
        action="package",
        label=get_invocation_id(),
        property_="install",
        context=context,
    )


def track_deprecation_warn(options):

    assert active_user is not None, "Cannot track deprecation warnings when active user is None"

    context = [SelfDescribingJson(DEPRECATION_WARN_SPEC, options)]

    track(
        active_user,
        category="dbt",
        action="deprecation",
        label=get_invocation_id(),
        property_="warn",
        context=context,
    )


def track_behavior_change_warn(msg: EventMsg) -> None:
    if msg.info.name != "BehaviorChangeEvent" or active_user is None:
        return

    context = [SelfDescribingJson(BEHAVIOR_CHANGE_WARN_SPEC, msg_to_dict(msg))]
    track(
        active_user,
        category="dbt",
        action=msg.info.name,
        label=get_invocation_id(),
        context=context,
    )


def track_invocation_end(invocation_context, result_type=None):
    data = {"progress": "end", "result_type": result_type, "result": None}
    data.update(invocation_context)
    context = [
        SelfDescribingJson(INVOCATION_SPEC, data),
        get_platform_context(),
        get_dbt_env_context(),
    ]

    assert active_user is not None, "Cannot track invocation end when active user is None"

    track(active_user, category="dbt", action="invocation", label="end", context=context)


def track_invalid_invocation(args=None, result_type=None):
    assert active_user is not None, "Cannot track invalid invocations when active user is None"
    invocation_context = get_base_invocation_context()
    invocation_context.update({"command": args.which})
    data = {"progress": "invalid", "result_type": result_type, "result": None}
    data.update(invocation_context)
    context = [
        SelfDescribingJson(INVOCATION_SPEC, data),
        get_platform_context(),
        get_dbt_env_context(),
    ]
    track(active_user, category="dbt", action="invocation", label="invalid", context=context)


def track_experimental_parser_sample(options):
    context = [SelfDescribingJson(EXPERIMENTAL_PARSER, options)]
    assert (
        active_user is not None
    ), "Cannot track experimental parser info when active user is None"

    track(
        active_user,
        category="dbt",
        action="experimental_parser",
        label=get_invocation_id(),
        context=context,
    )


def track_partial_parser(options):
    context = [SelfDescribingJson(PARTIAL_PARSER, options)]
    assert active_user is not None, "Cannot track partial parser info when active user is None"

    track(
        active_user,
        category="dbt",
        action="partial_parser",
        label=get_invocation_id(),
        context=context,
    )


def track_plugin_get_nodes(options):
    context = [SelfDescribingJson(PLUGIN_GET_NODES, options)]
    assert active_user is not None, "Cannot track plugin node info when active user is None"

    track(
        active_user,
        category="dbt",
        action="plugin_get_nodes",
        label=get_invocation_id(),
        context=context,
    )


def track_runnable_timing(options):
    context = [SelfDescribingJson(RUNNABLE_TIMING, options)]
    assert active_user is not None, "Cannot track runnable info when active user is None"
    track(
        active_user,
        category="dbt",
        action="runnable_timing",
        label=get_invocation_id(),
        context=context,
    )


def flush():
    fire_event(FlushEvents())
    try:
        tracker.flush()
    except Exception:
        fire_event(FlushEventsFailure())


def disable_tracking():
    global active_user
    if active_user is not None:
        active_user.disable_tracking()
    else:
        active_user = User(None)


def do_not_track():
    global active_user
    active_user = User(None)


def initialize_from_flags(send_anonymous_usage_stats, profiles_dir):
    global active_user
    if send_anonymous_usage_stats:
        active_user = User(profiles_dir)
        try:
            active_user.initialize()
        except Exception:
            fire_event(TrackingInitializeFailure(exc_info=traceback.format_exc()))
            active_user = User(None)
    else:
        active_user = User(None)


@contextmanager
def track_run(run_command=None):
    invocation_context = get_base_invocation_context()
    invocation_context["command"] = run_command

    track_invocation_start(invocation_context)
    try:
        yield
        track_invocation_end(invocation_context, result_type="ok")
    except (NotImplementedError, FailedToConnectError) as e:
        fire_event(MainEncounteredError(exc=str(e)))
        track_invocation_end(invocation_context, result_type="error")
    except Exception:
        track_invocation_end(invocation_context, result_type="error")
        raise
    finally:
        flush()
