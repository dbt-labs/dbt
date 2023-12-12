# coding=utf-8
import importlib
import os
import platform
import sys

from collections import namedtuple
from enum import Flag
from pathlib import Path
from typing import Optional, Dict, Any, List, Tuple

from dbt.common.events.functions import fire_event
from dbt.common.events.types import (
    OpenCommand,
    DebugCmdOut,
    DebugCmdResult,
)
import dbt.clients.system
import dbt.exceptions
import dbt.common.exceptions
from dbt.clients.adapter import get_adapter, register_adapter
from dbt.config import PartialProject, Project, Profile
from dbt.config.renderer import DbtProjectYamlRenderer, ProfileRenderer
from dbt.contracts.results import RunStatus
from dbt.clients.yaml_helper import load_yaml_text
from dbt.links import ProfileConfigDocs
from dbt.common.ui import green, red
from dbt.common.events.format import pluralize
from dbt.version import get_installed_version

from dbt.task.base import BaseTask, get_nearest_project_dir

ONLY_PROFILE_MESSAGE = """
A `dbt_project.yml` file was not found in this directory.
Using the only profile `{}`.
""".lstrip()

MULTIPLE_PROFILE_MESSAGE = """
A `dbt_project.yml` file was not found in this directory.
dbt found the following profiles:
{}

To debug one of these profiles, run:
dbt debug --profile [profile-name]
""".lstrip()

COULD_NOT_CONNECT_MESSAGE = """
dbt was unable to connect to the specified database.
The database returned the following error:

  >{err}

Check your database credentials and try again. For more information, visit:
{url}
""".lstrip()

MISSING_PROFILE_MESSAGE = """
dbt looked for a profiles.yml file in {path}, but did
not find one. For more information on configuring your profile, consult the
documentation:

{url}
""".lstrip()

FILE_NOT_FOUND = "file not found"


SubtaskStatus = namedtuple(
    "SubtaskStatus", ["log_msg", "run_status", "details", "summary_message"]
)


class DebugRunStatus(Flag):
    SUCCESS = True
    FAIL = False


class DebugTask(BaseTask):
    def __init__(self, args, config) -> None:
        super().__init__(args, config)
        self.profiles_dir = args.PROFILES_DIR
        self.profile_path = os.path.join(self.profiles_dir, "profiles.yml")
        try:
            self.project_dir = get_nearest_project_dir(self.args.project_dir)
        except dbt.common.exceptions.DbtBaseException:
            # we probably couldn't find a project directory. Set project dir
            # to whatever was given, or default to the current directory.
            if args.project_dir:
                self.project_dir = args.project_dir
            else:
                self.project_dir = Path.cwd()
        self.project_path = os.path.join(self.project_dir, "dbt_project.yml")
        self.cli_vars: Dict[str, Any] = args.vars

        # set by _load_*
        self.profile: Optional[Profile] = None
        self.raw_profile_data: Optional[Dict[str, Any]] = None
        self.profile_name: Optional[str] = None
        self.project: Optional[Project] = None

    @property
    def project_profile(self):
        if self.project is None:
            return None
        return self.project.profile_name

    def run(self) -> bool:
        # WARN: this is a legacy workflow that is not compatible with other runtime flags
        if self.args.config_dir:
            fire_event(
                OpenCommand(
                    open_cmd=dbt.clients.system.open_dir_cmd(), profiles_dir=str(self.profiles_dir)
                )
            )
            return DebugRunStatus.SUCCESS.value

        version: str = get_installed_version().to_version_string(skip_matcher=True)
        fire_event(DebugCmdOut(msg="dbt version: {}".format(version)))
        fire_event(DebugCmdOut(msg="python version: {}".format(sys.version.split()[0])))
        fire_event(DebugCmdOut(msg="python path: {}".format(sys.executable)))
        fire_event(DebugCmdOut(msg="os info: {}".format(platform.platform())))

        # Load profile if possible, then load adapter info (which requires the profile)
        load_profile_status: SubtaskStatus = self._load_profile()
        fire_event(DebugCmdOut(msg="Using profiles dir at {}".format(self.profiles_dir)))
        fire_event(DebugCmdOut(msg="Using profiles.yml file at {}".format(self.profile_path)))
        fire_event(DebugCmdOut(msg="Using dbt_project.yml file at {}".format(self.project_path)))
        if load_profile_status.run_status == RunStatus.Success:
            if self.profile is None:
                raise dbt.common.exceptions.DbtInternalError(
                    "Profile should not be None if loading profile completed"
                )
            else:
                adapter_type: str = self.profile.credentials.type

            adapter_version: str = self._read_adapter_version(
                f"dbt.adapters.{adapter_type}.__version__"
            )
            fire_event(DebugCmdOut(msg="adapter type: {}".format(adapter_type)))
            fire_event(DebugCmdOut(msg="adapter version: {}".format(adapter_version)))

        # Get project loaded to do additional checks
        load_project_status: SubtaskStatus = self._load_project()

        dependencies_statuses: List[SubtaskStatus] = []
        if self.args.connection:
            fire_event(DebugCmdOut(msg="Skipping steps before connection verification"))
        else:
            # this job's status not logged since already accounted for in _load_* commands
            self.test_configuration(load_profile_status.log_msg, load_project_status.log_msg)
            dependencies_statuses = self.test_dependencies()

        # Test connection
        connection_status = self.test_connection()

        # Log messages from any fails
        all_statuses: List[SubtaskStatus] = [
            load_profile_status,
            load_project_status,
            *dependencies_statuses,
            connection_status,
        ]
        all_failing_statuses: List[SubtaskStatus] = list(
            filter(lambda status: status.run_status == RunStatus.Error, all_statuses)
        )

        failure_count: int = len(all_failing_statuses)
        if failure_count > 0:
            fire_event(DebugCmdResult(msg=red(f"{(pluralize(failure_count, 'check'))} failed:")))
            for status in all_failing_statuses:
                fire_event(DebugCmdResult(msg=f"{status.summary_message}\n"))
            return DebugRunStatus.FAIL.value
        else:
            fire_event(DebugCmdResult(msg=green("All checks passed!")))
            return DebugRunStatus.SUCCESS.value

    # ==============================
    # Override for elsewhere in core
    # ==============================

    def interpret_results(self, results):
        return results

    # ===============
    # Loading profile
    # ===============

    def _load_profile(self) -> SubtaskStatus:
        """
        Side effects: load self.profile
                      load self.target_name
                      load self.raw_profile_data
        """
        if not os.path.exists(self.profile_path):
            return SubtaskStatus(
                log_msg=red("ERROR not found"),
                run_status=RunStatus.Error,
                details=FILE_NOT_FOUND,
                summary_message=MISSING_PROFILE_MESSAGE.format(
                    path=self.profile_path, url=ProfileConfigDocs
                ),
            )

        raw_profile_data = load_yaml_text(dbt.clients.system.load_file_contents(self.profile_path))
        if isinstance(raw_profile_data, dict):
            self.raw_profile_data = raw_profile_data

        profile_errors = []
        profile_names, summary_message = self._choose_profile_names()
        renderer = ProfileRenderer(self.cli_vars)
        for profile_name in profile_names:
            try:
                profile: Profile = Profile.render(
                    renderer,
                    profile_name,
                    self.args.profile,
                    self.args.target,
                    # TODO: Generalize safe access to flags.THREADS:
                    # https://github.com/dbt-labs/dbt-core/issues/6259
                    getattr(self.args, "threads", None),
                )
            except dbt.common.exceptions.DbtConfigError as exc:
                profile_errors.append(str(exc))
            else:
                if len(profile_names) == 1:
                    # if a profile was specified, set it on the task
                    self.target_name = self._choose_target_name(profile_name)
                    self.profile = profile

        if profile_errors:
            details = "\n\n".join(profile_errors)
            return SubtaskStatus(
                log_msg=red("ERROR invalid"),
                run_status=RunStatus.Error,
                details=details,
                summary_message=(
                    summary_message + f"Profile loading failed for the following reason:"
                    f"\n{details}"
                    f"\n"
                ),
            )
        else:
            return SubtaskStatus(
                log_msg=green("OK found and valid"),
                run_status=RunStatus.Success,
                details="",
                summary_message="Profile is valid",
            )

    def _choose_profile_names(self) -> Tuple[List[str], str]:
        project_profile: Optional[str] = None
        if os.path.exists(self.project_path):
            try:
                partial = PartialProject.from_project_root(
                    os.path.dirname(self.project_path),
                    verify_version=bool(self.args.VERSION_CHECK),
                )
                renderer = DbtProjectYamlRenderer(None, self.cli_vars)
                project_profile = partial.render_profile_name(renderer)
            except dbt.exceptions.DbtProjectError:
                pass

        args_profile: Optional[str] = getattr(self.args, "profile", None)

        try:
            return [Profile.pick_profile_name(args_profile, project_profile)], ""
        except dbt.common.exceptions.DbtConfigError:
            pass
        # try to guess

        profiles = []
        if self.raw_profile_data:
            profiles = [k for k in self.raw_profile_data if k != "config"]
            if project_profile is None:
                summary_message = "Could not load dbt_project.yml\n"
            elif len(profiles) == 0:
                summary_message = "The profiles.yml has no profiles\n"
            elif len(profiles) == 1:
                summary_message = ONLY_PROFILE_MESSAGE.format(profiles[0])
            else:
                summary_message = MULTIPLE_PROFILE_MESSAGE.format(
                    "\n".join(" - {}".format(o) for o in profiles)
                )
        return profiles, summary_message

    def _read_adapter_version(self, module) -> str:
        """read the version out of a standard adapter file"""
        try:
            version = importlib.import_module(module).version
        except ModuleNotFoundError:
            version = red("ERROR not found")
        except Exception as exc:
            version = red("ERROR {}".format(exc))
            raise dbt.exceptions.DbtInternalError(
                f"Error when reading adapter version from {module}: {exc}"
            )

        return version

    def _choose_target_name(self, profile_name: str):
        has_raw_profile = (
            self.raw_profile_data is not None and profile_name in self.raw_profile_data
        )

        if not has_raw_profile:
            return None

        # mypy appeasement, we checked just above
        assert self.raw_profile_data is not None
        raw_profile = self.raw_profile_data[profile_name]

        renderer = ProfileRenderer(self.cli_vars)

        target_name, _ = Profile.render_profile(
            raw_profile=raw_profile,
            profile_name=profile_name,
            target_override=getattr(self.args, "target", None),
            renderer=renderer,
        )
        return target_name

    # ===============
    # Loading project
    # ===============

    def _load_project(self) -> SubtaskStatus:
        """
        Side effect: load self.project
        """
        if not os.path.exists(self.project_path):
            return SubtaskStatus(
                log_msg=red("ERROR not found"),
                run_status=RunStatus.Error,
                details=FILE_NOT_FOUND,
                summary_message=(
                    f"Project loading failed for the following reason:"
                    f"\n project path <{self.project_path}> not found"
                ),
            )

        renderer = DbtProjectYamlRenderer(self.profile, self.cli_vars)

        try:
            self.project = Project.from_project_root(
                str(self.project_dir),
                renderer,
                verify_version=self.args.VERSION_CHECK,
            )
        except dbt.common.exceptions.DbtConfigError as exc:
            return SubtaskStatus(
                log_msg=red("ERROR invalid"),
                run_status=RunStatus.Error,
                details=str(exc),
                summary_message=(
                    f"Project loading failed for the following reason:" f"\n{str(exc)}" f"\n"
                ),
            )
        else:
            return SubtaskStatus(
                log_msg=green("OK found and valid"),
                run_status=RunStatus.Success,
                details="",
                summary_message="Project is valid",
            )

    def _profile_found(self) -> str:
        if not self.raw_profile_data:
            return red("ERROR not found")
        assert self.raw_profile_data is not None
        if self.profile_name in self.raw_profile_data:
            return green("OK found")
        else:
            return red("ERROR not found")

    def _target_found(self) -> str:
        requirements = self.raw_profile_data and self.profile_name and self.target_name
        if not requirements:
            return red("ERROR not found")
        # mypy appeasement, we checked just above
        assert self.raw_profile_data is not None
        assert self.profile_name is not None
        assert self.target_name is not None
        if self.profile_name not in self.raw_profile_data:
            return red("ERROR not found")
        profiles = self.raw_profile_data[self.profile_name]["outputs"]
        if self.target_name not in profiles:
            return red("ERROR not found")
        else:
            return green("OK found")

    # ============
    # Config tests
    # ============

    def test_git(self) -> SubtaskStatus:
        try:
            dbt.clients.system.run_cmd(os.getcwd(), ["git", "--help"])
        except dbt.exceptions.ExecutableError as exc:
            return SubtaskStatus(
                log_msg=red("ERROR"),
                run_status=RunStatus.Error,
                details="git error",
                summary_message="Error from git --help: {!s}".format(exc),
            )
        else:
            return SubtaskStatus(
                log_msg=green("OK found"),
                run_status=RunStatus.Success,
                details="",
                summary_message="git is installed and on the path",
            )

    def test_dependencies(self) -> List[SubtaskStatus]:
        fire_event(DebugCmdOut(msg="Required dependencies:"))

        git_test_status = self.test_git()
        fire_event(DebugCmdResult(msg=f" - git [{git_test_status.log_msg}]\n"))

        return [git_test_status]

    def test_configuration(self, profile_status_msg, project_status_msg):
        fire_event(DebugCmdOut(msg="Configuration:"))
        fire_event(DebugCmdOut(msg=f"  profiles.yml file [{profile_status_msg}]"))
        fire_event(DebugCmdOut(msg=f"  dbt_project.yml file [{project_status_msg}]"))

        # skip profile stuff if we can't find a profile name
        if self.profile_name is not None:
            fire_event(
                DebugCmdOut(
                    msg="  profile: {} [{}]\n".format(self.profile_name, self._profile_found())
                )
            )
            fire_event(
                DebugCmdOut(
                    msg="  target: {} [{}]\n".format(self.target_name, self._target_found())
                )
            )

    # ===============
    # Connection test
    # ===============

    @staticmethod
    def attempt_connection(profile) -> Optional[str]:
        """Return a string containing the error message, or None if there was no error."""
        register_adapter(profile)
        adapter = get_adapter(profile)
        try:
            with adapter.connection_named("debug"):
                # is defined in adapter class
                adapter.debug_query()
        except Exception as exc:
            return COULD_NOT_CONNECT_MESSAGE.format(
                err=str(exc),
                url=ProfileConfigDocs,
            )
        return None

    def test_connection(self) -> SubtaskStatus:
        if self.profile is None:
            fire_event(DebugCmdOut(msg="Connection test skipped since no profile was found"))
            return SubtaskStatus(
                log_msg=red("SKIPPED"),
                run_status=RunStatus.Skipped,
                details="No profile found",
                summary_message="Connection test skipped since no profile was found",
            )

        fire_event(DebugCmdOut(msg="Connection:"))
        for k, v in self.profile.credentials.connection_info():
            fire_event(DebugCmdOut(msg=f"  {k}: {v}"))

        connection_result = self.attempt_connection(self.profile)
        if connection_result is None:
            status = SubtaskStatus(
                log_msg=green("OK connection ok"),
                run_status=RunStatus.Success,
                details="",
                summary_message="Connection test passed",
            )
        else:
            status = SubtaskStatus(
                log_msg=red("ERROR"),
                run_status=RunStatus.Error,
                details="Failure in connecting to db",
                summary_message=connection_result,
            )
        fire_event(DebugCmdOut(msg=f"  Connection test: [{status.log_msg}]\n"))
        return status

    @classmethod
    def validate_connection(cls, target_dict):
        """Validate a connection dictionary. On error, raises a DbtConfigError."""
        target_name = "test"
        # make a fake profile that we can parse
        profile_data = {
            "outputs": {
                target_name: target_dict,
            },
        }
        # this will raise a DbtConfigError on failure
        profile = Profile.from_raw_profile_info(
            raw_profile=profile_data,
            profile_name="",
            target_override=target_name,
            renderer=ProfileRenderer({}),
        )
        result = cls.attempt_connection(profile)
        if result is not None:
            raise dbt.exceptions.DbtProfileError(result, result_type="connection_failure")
