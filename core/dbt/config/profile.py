import os
from dataclasses import dataclass
from typing import Any, Dict, Optional, Tuple

from dbt.adapters.contracts.connection import Credentials, HasCredentials
from dbt.clients.yaml_helper import get_yaml_path, load_yaml_text
from dbt.contracts.project import ProfileConfig
from dbt.events.types import MissingProfileTarget
from dbt.exceptions import (
    CompilationError,
    DbtProfileError,
    DbtProjectError,
    DbtRuntimeError,
    ProfileConfigError,
)
from dbt.flags import get_flags
from dbt_common.clients.system import load_file_contents
from dbt_common.dataclass_schema import ValidationError
from dbt_common.events.functions import fire_event
from dbt_common.exceptions import DbtValidationError

from .renderer import ProfileRenderer

DEFAULT_THREADS = 1

INVALID_PROFILE_MESSAGE = """
dbt encountered an error while trying to read your profiles.yml file.

{error_string}
"""


def read_profile(profiles_dir: str) -> Dict[str, Any]:
    path = get_yaml_path(os.path.join(profiles_dir, "profiles"))

    contents = None
    if os.path.isfile(path):
        try:
            contents = load_file_contents(path, strip=False)
            yaml_content = load_yaml_text(contents)
            if not yaml_content:
                msg = f"The profiles.yml file at {path} is empty"
                raise DbtProfileError(INVALID_PROFILE_MESSAGE.format(error_string=msg))
            return yaml_content
        except DbtValidationError as e:
            msg = INVALID_PROFILE_MESSAGE.format(error_string=e)
            raise DbtValidationError(msg) from e

    return {}


# The Profile class is included in RuntimeConfig, so any attribute
# additions must also be set where the RuntimeConfig class is created
# `init=False` is a workaround for https://bugs.python.org/issue45081
@dataclass(init=False)
class Profile(HasCredentials):
    profile_name: str
    target_name: str
    threads: int
    credentials: Credentials
    profile_env_vars: Dict[str, Any]
    log_cache_events: bool

    def __init__(
        self,
        profile_name: str,
        target_name: str,
        threads: int,
        credentials: Credentials,
    ) -> None:
        """Explicitly defining `__init__` to work around bug in Python 3.9.7
        https://bugs.python.org/issue45081
        """
        self.profile_name = profile_name
        self.target_name = target_name
        self.threads = threads
        self.credentials = credentials
        self.profile_env_vars = {}  # never available on init
        self.log_cache_events = (
            get_flags().LOG_CACHE_EVENTS
        )  # never available on init, set for adapter instantiation via AdapterRequiredConfig

    def to_profile_info(self, serialize_credentials: bool = False) -> Dict[str, Any]:
        """Unlike to_project_config, this dict is not a mirror of any existing
        on-disk data structure. It's used when creating a new profile from an
        existing one.

        :param serialize_credentials bool: If True, serialize the credentials.
            Otherwise, the Credentials object will be copied.
        :returns dict: The serialized profile.
        """
        result = {
            "profile_name": self.profile_name,
            "target_name": self.target_name,
            "threads": self.threads,
            "credentials": self.credentials,
        }
        if serialize_credentials:
            result["credentials"] = self.credentials.to_dict(omit_none=True)
        return result

    def to_target_dict(self) -> Dict[str, Any]:
        target = dict(self.credentials.connection_info(with_aliases=True))
        target.update(
            {
                "type": self.credentials.type,
                "threads": self.threads,
                "name": self.target_name,
                "target_name": self.target_name,
                "profile_name": self.profile_name,
            }
        )
        return target

    def __eq__(self, other: object) -> bool:
        if not (isinstance(other, self.__class__) and isinstance(self, other.__class__)):
            return NotImplemented
        return self.to_profile_info() == other.to_profile_info()

    def validate(self):
        try:
            if self.credentials:
                dct = self.credentials.to_dict(omit_none=True)
                self.credentials.validate(dct)
            dct = self.to_profile_info(serialize_credentials=True)
            ProfileConfig.validate(dct)
        except ValidationError as exc:
            raise ProfileConfigError(exc) from exc

    @staticmethod
    def _credentials_from_profile(
        profile: Dict[str, Any], profile_name: str, target_name: str
    ) -> Credentials:
        # avoid an import cycle
        from dbt.adapters.factory import load_plugin

        # credentials carry their 'type' in their actual type, not their
        # attributes. We do want this in order to pick our Credentials class.
        if "type" not in profile:
            raise DbtProfileError(
                'required field "type" not found in profile {} and target {}'.format(
                    profile_name, target_name
                )
            )

        typename = profile.pop("type")
        try:
            cls = load_plugin(typename)
            data = cls.translate_aliases(profile)
            cls.validate(data)
            credentials = cls.from_dict(data)
        except (DbtRuntimeError, ValidationError) as e:
            msg = str(e) if isinstance(e, DbtRuntimeError) else e.message
            raise DbtProfileError(
                'Credentials in profile "{}", target "{}" invalid: {}'.format(
                    profile_name, target_name, msg
                )
            ) from e

        return credentials

    @staticmethod
    def pick_profile_name(
        args_profile_name: Optional[str],
        project_profile_name: Optional[str] = None,
    ) -> str:
        # TODO: Duplicating this method as direct copy of the implementation in dbt.cli.resolvers
        # dbt.cli.resolvers implementation can't be used because it causes a circular dependency.
        # This should be removed and use a safe default access on the Flags module when
        # https://github.com/dbt-labs/dbt-core/issues/6259 is closed.
        def default_profiles_dir():
            from pathlib import Path

            return Path.cwd() if (Path.cwd() / "profiles.yml").exists() else Path.home() / ".dbt"

        profile_name = project_profile_name
        if args_profile_name is not None:
            profile_name = args_profile_name
        if profile_name is None:
            NO_SUPPLIED_PROFILE_ERROR = """\
dbt cannot run because no profile was specified for this dbt project.
To specify a profile for this project, add a line like the this to
your dbt_project.yml file:

profile: [profile name]

Here, [profile name] should be replaced with a profile name
defined in your profiles.yml file. You can find profiles.yml here:

{profiles_file}/profiles.yml
""".format(
                profiles_file=default_profiles_dir()
            )
            raise DbtProjectError(NO_SUPPLIED_PROFILE_ERROR)
        return profile_name

    @staticmethod
    def _get_profile_data(
        profile: Dict[str, Any], profile_name: str, target_name: str
    ) -> Dict[str, Any]:
        if "outputs" not in profile:
            raise DbtProfileError("outputs not specified in profile '{}'".format(profile_name))
        outputs = profile["outputs"]

        if target_name not in outputs:
            outputs = "\n".join(" - {}".format(output) for output in outputs)
            msg = (
                "The profile '{}' does not have a target named '{}'. The "
                "valid target names for this profile are:\n{}".format(
                    profile_name, target_name, outputs
                )
            )
            raise DbtProfileError(msg, result_type="invalid_target")
        profile_data = outputs[target_name]

        if not isinstance(profile_data, dict):
            msg = (
                f"output '{target_name}' of profile '{profile_name}' is "
                f"misconfigured in profiles.yml"
            )
            raise DbtProfileError(msg, result_type="invalid_target")

        return profile_data

    @classmethod
    def from_credentials(
        cls,
        credentials: Credentials,
        threads: int,
        profile_name: str,
        target_name: str,
    ) -> "Profile":
        """Create a profile from an existing set of Credentials and the
        remaining information.

        :param credentials: The credentials dict for this profile.
        :param threads: The number of threads to use for connections.
        :param profile_name: The profile name used for this profile.
        :param target_name: The target name used for this profile.
        :raises DbtProfileError: If the profile is invalid.
        :returns: The new Profile object.
        """

        profile = cls(
            profile_name=profile_name,
            target_name=target_name,
            threads=threads,
            credentials=credentials,
        )
        profile.validate()
        return profile

    @classmethod
    def render_profile(
        cls,
        raw_profile: Dict[str, Any],
        profile_name: str,
        target_override: Optional[str],
        renderer: ProfileRenderer,
    ) -> Tuple[str, Dict[str, Any]]:
        """This is a containment zone for the hateful way we're rendering
        profiles.
        """
        # rendering profiles is a bit complex. Two constraints cause trouble:
        # 1) users should be able to use environment/cli variables to specify
        #    the target in their profile.
        # 2) Missing environment/cli variables in profiles/targets that don't
        #    end up getting selected should not cause errors.
        # so first we'll just render the target name, then we use that rendered
        # name to extract a profile that we can render.
        if target_override is not None:
            target_name = target_override
        elif "target" in raw_profile:
            # render the target if it was parsed from yaml
            target_name = renderer.render_value(raw_profile["target"])
        else:
            target_name = "default"
            fire_event(MissingProfileTarget(profile_name=profile_name, target_name=target_name))

        raw_profile_data = cls._get_profile_data(raw_profile, profile_name, target_name)

        try:
            profile_data = renderer.render_data(raw_profile_data)
        except CompilationError as exc:
            raise DbtProfileError(str(exc)) from exc
        return target_name, profile_data

    @classmethod
    def from_raw_profile_info(
        cls,
        raw_profile: Dict[str, Any],
        profile_name: str,
        renderer: ProfileRenderer,
        target_override: Optional[str] = None,
        threads_override: Optional[int] = None,
    ) -> "Profile":
        """Create a profile from its raw profile information.

         (this is an intermediate step, mostly useful for unit testing)

        :param raw_profile: The profile data for a single profile, from
            disk as yaml and its values rendered with jinja.
        :param profile_name: The profile name used.
        :param renderer: The config renderer.
        :param target_override: The target to use, if provided on
            the command line.
        :param threads_override: The thread count to use, if
            provided on the command line.
        :raises DbtProfileError: If the profile is invalid or missing, or the
            target could not be found
        :returns: The new Profile object.
        """
        # TODO: should it be, and the values coerced to bool?
        target_name, profile_data = cls.render_profile(
            raw_profile, profile_name, target_override, renderer
        )

        # valid connections never include the number of threads, but it's
        # stored on a per-connection level in the raw configs
        threads = profile_data.pop("threads", DEFAULT_THREADS)
        if threads_override is not None:
            threads = threads_override

        credentials: Credentials = cls._credentials_from_profile(
            profile_data, profile_name, target_name
        )

        return cls.from_credentials(
            credentials=credentials,
            profile_name=profile_name,
            target_name=target_name,
            threads=threads,
        )

    @classmethod
    def from_raw_profiles(
        cls,
        raw_profiles: Dict[str, Any],
        profile_name: str,
        renderer: ProfileRenderer,
        target_override: Optional[str] = None,
        threads_override: Optional[int] = None,
    ) -> "Profile":
        """
        :param raw_profiles: The profile data, from disk as yaml.
        :param profile_name: The profile name to use.
        :param renderer: The config renderer.
        :param target_override: The target to use, if provided on the command
            line.
        :param threads_override: The thread count to use, if provided on the
            command line.
        :raises DbtProjectError: If there is no profile name specified in the
            project or the command line arguments
        :raises DbtProfileError: If the profile is invalid or missing, or the
            target could not be found
        :returns: The new Profile object.
        """
        if profile_name not in raw_profiles:
            raise DbtProjectError("Could not find profile named '{}'".format(profile_name))

        # First, we've already got our final decision on profile name, and we
        # don't render keys, so we can pluck that out
        raw_profile = raw_profiles[profile_name]
        if not raw_profile:
            msg = f"Profile {profile_name} in profiles.yml is empty"
            raise DbtProfileError(INVALID_PROFILE_MESSAGE.format(error_string=msg))

        return cls.from_raw_profile_info(
            raw_profile=raw_profile,
            profile_name=profile_name,
            renderer=renderer,
            target_override=target_override,
            threads_override=threads_override,
        )

    @classmethod
    def render(
        cls,
        renderer: ProfileRenderer,
        project_profile_name: Optional[str],
        profile_name_override: Optional[str] = None,
        target_override: Optional[str] = None,
        threads_override: Optional[int] = None,
    ) -> "Profile":
        """Given the raw profiles as read from disk and the name of the desired
        profile if specified, return the profile component of the runtime
        config.

        :param args argparse.Namespace: The arguments as parsed from the cli.
        :param project_profile_name Optional[str]: The profile name, if
            specified in a project.
        :raises DbtProjectError: If there is no profile name specified in the
            project or the command line arguments, or if the specified profile
            is not found
        :raises DbtProfileError: If the profile is invalid or missing, or the
            target could not be found.
        :returns Profile: The new Profile object.
        """
        flags = get_flags()
        raw_profiles = read_profile(flags.PROFILES_DIR)
        profile_name = cls.pick_profile_name(profile_name_override, project_profile_name)
        return cls.from_raw_profiles(
            raw_profiles=raw_profiles,
            profile_name=profile_name,
            renderer=renderer,
            target_override=target_override,
            threads_override=threads_override,
        )
