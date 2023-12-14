import copy
import os
from pathlib import Path
import re
import shutil
from typing import Optional

import yaml
import click

import dbt.config
import dbt.clients.system
from dbt.config.profile import read_profile
from dbt.exceptions import DbtRuntimeError, NoAdaptersAvailableError
from dbt.flags import get_flags
from dbt.version import _get_adapter_plugin_names
from dbt.adapters.factory import load_plugin, get_include_paths

from dbt.contracts.util import Identifier as ProjectName

from dbt.events.functions import fire_event
from dbt.events.types import (
    StarterProjectPath,
    ConfigFolderDirectory,
    NoSampleProfileFound,
    ProfileWrittenWithSample,
    ProfileWrittenWithTargetTemplateYAML,
    ProfileWrittenWithProjectTemplateYAML,
    SettingUpProfile,
    InvalidProfileTemplateYAML,
    ProjectCreated,
)

from dbt.include.starter_project import PACKAGE_PATH as starter_project_directory

from dbt.include.global_project import PROJECT_NAME as GLOBAL_PROJECT_NAME

from dbt.task.base import BaseTask, move_to_nearest_project_dir

DOCS_URL = "https://docs.getdbt.com/docs/configure-your-profile"
SLACK_URL = "https://community.getdbt.com/"

# This file is not needed for the starter project but exists for finding the resource path
IGNORE_FILES = ["__init__.py", "__pycache__"]


# https://click.palletsprojects.com/en/8.0.x/api/#types
# click v7.0 has UNPROCESSED, STRING, INT, FLOAT, BOOL, and UUID available.
click_type_mapping = {
    "string": click.STRING,
    "int": click.INT,
    "float": click.FLOAT,
    "bool": click.BOOL,
    None: None,
}


class InitTask(BaseTask):
    def copy_starter_repo(self, project_name):
        fire_event(StarterProjectPath(dir=starter_project_directory))
        os.chdir(os.path.dirname(self.args.project_dir))
        shutil.copytree(
            starter_project_directory,
            project_name,
            ignore=shutil.ignore_patterns(*IGNORE_FILES),
            dirs_exist_ok=True,
        )
        os.chdir(project_name)

    def create_profiles_dir(self, profiles_dir: str) -> bool:
        """Create the user's profiles directory if it doesn't already exist."""
        profiles_path = Path(profiles_dir)
        if not profiles_path.exists():
            fire_event(ConfigFolderDirectory(dir=profiles_dir))
            dbt.clients.system.make_directory(profiles_dir)
            return True
        return False

    def create_profile_from_sample(self, adapter: str, profile_name: str):
        """Create a profile entry using the adapter's sample_profiles.yml

        Renames the profile in sample_profiles.yml to match that of the project."""
        # Line below raises an exception if the specified adapter is not found
        load_plugin(adapter)
        adapter_path = get_include_paths(adapter)[0]
        sample_profiles_path = adapter_path / "sample_profiles.yml"

        if not sample_profiles_path.exists():
            fire_event(NoSampleProfileFound(adapter=adapter))
        else:
            with open(sample_profiles_path, "r") as f:
                sample_profile = f.read()
            sample_profile_name = list(yaml.safe_load(sample_profile).keys())[0]
            # Use a regex to replace the name of the sample_profile with
            # that of the project without losing any comments from the sample
            sample_profile = re.sub(f"^{sample_profile_name}:", f"{profile_name}:", sample_profile)
            profiles_filepath = Path(get_flags().PROFILES_DIR) / Path("profiles.yml")
            if profiles_filepath.exists():
                with open(profiles_filepath, "a") as f:
                    f.write("\n" + sample_profile)
            else:
                with open(profiles_filepath, "w") as f:
                    f.write(sample_profile)
                fire_event(
                    ProfileWrittenWithSample(name=profile_name, path=str(profiles_filepath))
                )

    def generate_target_from_input(self, profile_template: dict, target: dict = {}) -> dict:
        """Generate a target configuration from profile_template and user input."""
        profile_template_local = copy.deepcopy(profile_template)
        for key, value in profile_template_local.items():
            if key.startswith("_choose"):
                choice_type = key[8:].replace("_", " ")
                option_list = list(value.keys())
                prompt_msg = (
                    "\n".join([f"[{n+1}] {v}" for n, v in enumerate(option_list)])
                    + f"\nDesired {choice_type} option (enter a number)"
                )
                numeric_choice = click.prompt(prompt_msg, type=click.INT)
                choice = option_list[numeric_choice - 1]
                # Complete the chosen option's values in a recursive call
                target = self.generate_target_from_input(
                    profile_template_local[key][choice], target
                )
            else:
                if key.startswith("_fixed"):
                    # _fixed prefixed keys are not presented to the user
                    target[key[7:]] = value
                else:
                    hide_input = value.get("hide_input", False)
                    default = value.get("default", None)
                    hint = value.get("hint", None)
                    type = click_type_mapping[value.get("type", None)]
                    text = key + (f" ({hint})" if hint else "")
                    target[key] = click.prompt(
                        text, default=default, hide_input=hide_input, type=type
                    )
        return target

    def get_profile_name_from_current_project(self) -> str:
        """Reads dbt_project.yml in the current directory to retrieve the
        profile name.
        """
        with open("dbt_project.yml") as f:
            dbt_project = yaml.safe_load(f)
        return dbt_project["profile"]

    def write_profile(self, profile: dict, profile_name: str):
        """Given a profile, write it to the current project's profiles.yml.
        This will overwrite any profile with a matching name."""
        # Create the profile directory if it doesn't exist
        profiles_filepath = Path(get_flags().PROFILES_DIR) / Path("profiles.yml")

        profiles = {profile_name: profile}

        if profiles_filepath.exists():
            with open(profiles_filepath, "r") as f:
                profiles = yaml.safe_load(f) or {}
                profiles[profile_name] = profile

        # Write the profiles dictionary to a brand-new or pre-existing file
        with open(profiles_filepath, "w") as f:
            yaml.dump(profiles, f)

    def create_profile_from_profile_template(self, profile_template: dict, profile_name: str):
        """Create and write a profile using the supplied profile_template."""
        initial_target = profile_template.get("fixed", {})
        prompts = profile_template.get("prompts", {})
        target = self.generate_target_from_input(prompts, initial_target)
        target_name = target.pop("target", "dev")
        profile = {"outputs": {target_name: target}, "target": target_name}
        self.write_profile(profile, profile_name)

    def create_profile_from_target(self, adapter: str, profile_name: str):
        """Create a profile without defaults using target's profile_template.yml if available, or
        sample_profiles.yml as a fallback."""
        # Line below raises an exception if the specified adapter is not found
        load_plugin(adapter)
        adapter_path = get_include_paths(adapter)[0]
        profile_template_path = adapter_path / "profile_template.yml"

        if profile_template_path.exists():
            with open(profile_template_path) as f:
                profile_template = yaml.safe_load(f)
            self.create_profile_from_profile_template(profile_template, profile_name)
            profiles_filepath = Path(get_flags().PROFILES_DIR) / Path("profiles.yml")
            fire_event(
                ProfileWrittenWithTargetTemplateYAML(
                    name=profile_name, path=str(profiles_filepath)
                )
            )
        else:
            # For adapters without a profile_template.yml defined, fallback on
            # sample_profiles.yml
            self.create_profile_from_sample(adapter, profile_name)

    def check_if_profile_exists(self, profile_name: str) -> bool:
        """
        Validate that the specified profile exists. Can't use the regular profile validation
        routine because it assumes the project file exists
        """
        profiles_dir = get_flags().PROFILES_DIR
        raw_profiles = read_profile(profiles_dir)
        return profile_name in raw_profiles

    def check_if_can_write_profile(self, profile_name: Optional[str] = None) -> bool:
        """Using either a provided profile name or that specified in dbt_project.yml,
        check if the profile already exists in profiles.yml, and if so ask the
        user whether to proceed and overwrite it."""
        profiles_file = Path(get_flags().PROFILES_DIR) / Path("profiles.yml")
        if not profiles_file.exists():
            return True
        profile_name = profile_name or self.get_profile_name_from_current_project()
        with open(profiles_file, "r") as f:
            profiles = yaml.safe_load(f) or {}
        if profile_name in profiles.keys():
            response = click.confirm(
                f"The profile {profile_name} already exists in "
                f"{profiles_file}. Continue and overwrite it?"
            )
            return response
        else:
            return True

    def create_profile_using_project_profile_template(self, profile_name):
        """Create a profile using the project's profile_template.yml"""
        with open("profile_template.yml") as f:
            profile_template = yaml.safe_load(f)
        self.create_profile_from_profile_template(profile_template, profile_name)
        profiles_filepath = Path(get_flags().PROFILES_DIR) / Path("profiles.yml")
        fire_event(
            ProfileWrittenWithProjectTemplateYAML(name=profile_name, path=str(profiles_filepath))
        )

    def ask_for_adapter_choice(self) -> str:
        """Ask the user which adapter (database) they'd like to use."""
        available_adapters = list(_get_adapter_plugin_names())

        if not available_adapters:
            raise NoAdaptersAvailableError()

        prompt_msg = (
            "Which database would you like to use?\n"
            + "\n".join([f"[{n+1}] {v}" for n, v in enumerate(available_adapters)])
            + "\n\n(Don't see the one you want? https://docs.getdbt.com/docs/available-adapters)"
            + "\n\nEnter a number"
        )
        numeric_choice = click.prompt(prompt_msg, type=click.INT)
        return available_adapters[numeric_choice - 1]

    def setup_profile(self, profile_name: str) -> None:
        """Set up a new profile for a project"""
        fire_event(SettingUpProfile())
        if not self.check_if_can_write_profile(profile_name=profile_name):
            return
        # If a profile_template.yml exists in the project root, that effectively
        # overrides the profile_template.yml for the given target.
        profile_template_path = Path("profile_template.yml")
        if profile_template_path.exists():
            try:
                # This relies on a valid profile_template.yml from the user,
                # so use a try: except to fall back to the default on failure
                self.create_profile_using_project_profile_template(profile_name)
                return
            except Exception:
                fire_event(InvalidProfileTemplateYAML())
        adapter = self.ask_for_adapter_choice()
        self.create_profile_from_target(adapter, profile_name=profile_name)

    def check_valid_project_name(self) -> None:
        name = os.path.basename(self.args.project_dir)
        internal_package_names = {GLOBAL_PROJECT_NAME}
        available_adapters = list(_get_adapter_plugin_names())
        for adapter_name in available_adapters:
            internal_package_names.update(f"dbt_{adapter_name}")
        if not ProjectName.is_valid(name) or name in internal_package_names:
            raise DbtRuntimeError(
                msg="{} is not a valid project name.\nEnter a name for your project (letters, digits, underscore)".format(
                    name
                )
            )

    def create_new_project(self, project_name: str, profile_name: str):
        self.copy_starter_repo(project_name)
        with open("dbt_project.yml", "r") as f:
            content = f"{f.read()}".format(project_name=project_name, profile_name=profile_name)
        with open("dbt_project.yml", "w") as f:
            f.write(content)
        fire_event(
            ProjectCreated(
                project_name=project_name,
                docs_url=DOCS_URL,
                slack_url=SLACK_URL,
            )
        )

    def check_if_project_directory_empty(self) -> bool:
        """Check if the project directory is empty"""
        if os.listdir(self.args.project_dir):
            return False
        else:
            return True

    def run(self):
        """Entry point for the init task."""
        profiles_dir = get_flags().PROFILES_DIR
        self.create_profiles_dir(profiles_dir)
        project_name = Path(self.args.project_dir).name

        try:
            move_to_nearest_project_dir(self.args.project_dir)
            in_project = True
        except DbtRuntimeError:
            in_project = False

        if in_project:
            # If --profile was specified, it means use an existing profile, which is not
            # applicable to this case
            if self.args.profile:
                raise DbtRuntimeError(
                    msg="Can not init existing project with specified profile, edit dbt_project.yml instead"
                )

            # When dbt init is run inside an existing project,
            # just setup the user's profile.
            if not self.args.skip_profile_setup:
                profile_name = self.get_profile_name_from_current_project()
                self.setup_profile(profile_name)
        else:
            # When dbt init is run outside of an existing project,
            # create a new project and set up the user's profile.
            self.check_valid_project_name()

            if not self.check_if_project_directory_empty():
                raise DbtRuntimeError(
                    msg=f"{project_name} directory not empty; eg: mkdir project_name; cd project_name; dbt init"
                )

            # If the user specified an existing profile to use, use it instead of generating a new one
            user_profile_name = self.args.profile
            if user_profile_name:
                if not self.args.project_name and not self.check_if_profile_exists(
                    user_profile_name
                ):
                    raise DbtRuntimeError(
                        msg="Could not find profile named '{}'".format(user_profile_name)
                    )
                self.create_new_project(project_name, user_profile_name)
            else:
                profile_name = project_name
                # Create the profile after creating the project to avoid leaving a random profile
                # if the former fails.
                self.create_new_project(project_name, profile_name)

                # Ask for adapter only if skip_profile_setup flag is not provided
                if not self.args.skip_profile_setup:
                    self.setup_profile(profile_name)
