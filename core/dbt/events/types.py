import json

from dbt.constants import MAXIMUM_SEED_SIZE_NAME, PIN_PACKAGE_URL
from dbt.events.base_types import (
    DebugLevel,
    DynamicLevel,
    ErrorLevel,
    InfoLevel,
    WarnLevel,
)
from dbt_common.events.base_types import EventLevel
from dbt_common.events.format import (
    format_fancy_output_line,
    pluralize,
    timestamp_to_datetime_string,
)
from dbt_common.ui import error_tag, green, line_wrap_message, red, warning_tag, yellow

# Event codes have prefixes which follow this table
#
# | Code |     Description     |
# |:----:|:-------------------:|
# | A    | Pre-project loading |
# | D    | Deprecations        |
# | E    | DB adapter          |
# | I    | Project parsing     |
# | M    | Deps generation     |
# | P    | Artifacts           |
# | Q    | Node execution      |
# | W    | Node testing        |
# | Z    | Misc                |
# | T    | Test only           |
#
# The basic idea is that event codes roughly translate to the natural order of running a dbt task

# =======================================================
# A - Pre-project loading
# =======================================================


class MainReportVersion(InfoLevel):
    def code(self) -> str:
        return "A001"

    def message(self) -> str:
        return f"Running with dbt{self.version}"


class MainReportArgs(DebugLevel):
    def code(self) -> str:
        return "A002"

    def message(self) -> str:
        return f"running dbt with arguments {str(self.args)}"


class MainTrackingUserState(DebugLevel):
    def code(self) -> str:
        return "A003"

    def message(self) -> str:
        return f"Tracking: {self.user_state}"


# Removed A004: MergedFromState


class MissingProfileTarget(InfoLevel):
    def code(self) -> str:
        return "A005"

    def message(self) -> str:
        return f"target not specified in profile '{self.profile_name}', using '{self.target_name}'"


# Skipped A006, A007


class InvalidOptionYAML(ErrorLevel):
    def code(self) -> str:
        return "A008"

    def message(self) -> str:
        return f"The YAML provided in the --{self.option_name} argument is not valid."


class LogDbtProjectError(ErrorLevel):
    def code(self) -> str:
        return "A009"

    def message(self) -> str:
        msg = "Encountered an error while reading the project:"
        if self.exc:
            msg += f"  ERROR: {str(self.exc)}"
        return msg


# Skipped A010


class LogDbtProfileError(ErrorLevel):
    def code(self) -> str:
        return "A011"

    def message(self) -> str:
        msg = "Encountered an error while reading profiles:\n" f"  ERROR: {str(self.exc)}"
        if self.profiles:
            msg += "Defined profiles:\n"
            for profile in self.profiles:
                msg += f" - {profile}"
        else:
            msg += "There are no profiles defined in your profiles.yml file"

        msg += """
For more information on configuring profiles, please consult the dbt docs:

https://docs.getdbt.com/docs/configure-your-profile
"""
        return msg


class StarterProjectPath(DebugLevel):
    def code(self) -> str:
        return "A017"

    def message(self) -> str:
        return f"Starter project path: {self.dir}"


class ConfigFolderDirectory(InfoLevel):
    def code(self) -> str:
        return "A018"

    def message(self) -> str:
        return f"Creating dbt configuration folder at {self.dir}"


class NoSampleProfileFound(InfoLevel):
    def code(self) -> str:
        return "A019"

    def message(self) -> str:
        return f"No sample profile found for {self.adapter}."


class ProfileWrittenWithSample(InfoLevel):
    def code(self) -> str:
        return "A020"

    def message(self) -> str:
        return (
            f"Profile {self.name} written to {self.path} "
            "using target's sample configuration. Once updated, you'll be able to "
            "start developing with dbt."
        )


class ProfileWrittenWithTargetTemplateYAML(InfoLevel):
    def code(self) -> str:
        return "A021"

    def message(self) -> str:
        return (
            f"Profile {self.name} written to {self.path} using target's "
            "profile_template.yml and your supplied values. Run 'dbt debug' to "
            "validate the connection."
        )


class ProfileWrittenWithProjectTemplateYAML(InfoLevel):
    def code(self) -> str:
        return "A022"

    def message(self) -> str:
        return (
            f"Profile {self.name} written to {self.path} using project's "
            "profile_template.yml and your supplied values. Run 'dbt debug' to "
            "validate the connection."
        )


class SettingUpProfile(InfoLevel):
    def code(self) -> str:
        return "A023"

    def message(self) -> str:
        return "Setting up your profile."


class InvalidProfileTemplateYAML(InfoLevel):
    def code(self) -> str:
        return "A024"

    def message(self) -> str:
        return "Invalid profile_template.yml in project."


class ProjectNameAlreadyExists(InfoLevel):
    def code(self) -> str:
        return "A025"

    def message(self) -> str:
        return f"A project called {self.name} already exists here."


class ProjectCreated(InfoLevel):
    def code(self) -> str:
        return "A026"

    def message(self) -> str:
        return f"""
Your new dbt project "{self.project_name}" was created!

For more information on how to configure the profiles.yml file,
please consult the dbt documentation here:

  {self.docs_url}

One more thing:

Need help? Don't hesitate to reach out to us via GitHub issues or on Slack:

  {self.slack_url}

Happy modeling!
"""


# =======================================================
# D - Deprecations
# =======================================================


class DeprecatedModel(WarnLevel):
    def code(self) -> str:
        return "I065"

    def message(self) -> str:
        version = ".v" + self.model_version if self.model_version else ""
        msg = (
            f"Model {self.model_name}{version} has passed its deprecation date of {self.deprecation_date}. "
            "This model should be disabled or removed."
        )
        return warning_tag(msg)


class PackageRedirectDeprecation(WarnLevel):
    def code(self) -> str:
        return "D001"

    def message(self) -> str:
        description = (
            f"The `{self.old_name}` package is deprecated in favor of `{self.new_name}`. Please "
            f"update your `packages.yml` configuration to use `{self.new_name}` instead."
        )
        return line_wrap_message(warning_tag(f"Deprecated functionality\n\n{description}"))


class PackageInstallPathDeprecation(WarnLevel):
    def code(self) -> str:
        return "D002"

    def message(self) -> str:
        description = """\
        The default package install path has changed from `dbt_modules` to `dbt_packages`.
        Please update `clean-targets` in `dbt_project.yml` and check `.gitignore` as well.
        Or, set `packages-install-path: dbt_modules` if you'd like to keep the current value.
        """
        return line_wrap_message(warning_tag(f"Deprecated functionality\n\n{description}"))


class ConfigSourcePathDeprecation(WarnLevel):
    def code(self) -> str:
        return "D003"

    def message(self) -> str:
        description = (
            f"The `{self.deprecated_path}` config has been renamed to `{self.exp_path}`. "
            "Please update your `dbt_project.yml` configuration to reflect this change."
        )
        return line_wrap_message(warning_tag(f"Deprecated functionality\n\n{description}"))


class ConfigDataPathDeprecation(WarnLevel):
    def code(self) -> str:
        return "D004"

    def message(self) -> str:
        description = (
            f"The `{self.deprecated_path}` config has been renamed to `{self.exp_path}`. "
            "Please update your `dbt_project.yml` configuration to reflect this change."
        )
        return line_wrap_message(warning_tag(f"Deprecated functionality\n\n{description}"))


class MetricAttributesRenamed(WarnLevel):
    def code(self) -> str:
        return "D006"

    def message(self) -> str:
        description = (
            "dbt-core v1.3 renamed attributes for metrics:"
            "\n  'sql'              -> 'expression'"
            "\n  'type'             -> 'calculation_method'"
            "\n  'type: expression' -> 'calculation_method: derived'"
            f"\nPlease remove them from the metric definition of metric '{self.metric_name}'"
            "\nRelevant issue here: https://github.com/dbt-labs/dbt-core/issues/5849"
        )

        return warning_tag(f"Deprecated functionality\n\n{description}")


class ExposureNameDeprecation(WarnLevel):
    def code(self) -> str:
        return "D007"

    def message(self) -> str:
        description = (
            "Starting in v1.3, the 'name' of an exposure should contain only letters, "
            "numbers, and underscores. Exposures support a new property, 'label', which may "
            f"contain spaces, capital letters, and special characters. {self.exposure} does not "
            "follow this pattern. Please update the 'name', and use the 'label' property for a "
            "human-friendly title. This will raise an error in a future version of dbt-core."
        )
        return line_wrap_message(warning_tag(f"Deprecated functionality\n\n{description}"))


class InternalDeprecation(WarnLevel):
    def code(self) -> str:
        return "D008"

    def message(self) -> str:
        extra_reason = ""
        if self.reason:
            extra_reason = f"\n{self.reason}"
        msg = (
            f"`{self.name}` is deprecated and will be removed in dbt-core version {self.version}\n\n"
            f"Adapter maintainers can resolve this deprecation by {self.suggested_action}. {extra_reason}"
        )
        return warning_tag(msg)


class EnvironmentVariableRenamed(WarnLevel):
    def code(self) -> str:
        return "D009"

    def message(self) -> str:
        description = (
            f"The environment variable `{self.old_name}` has been renamed as `{self.new_name}`.\n"
            f"If `{self.old_name}` is currently set, its value will be used instead of `{self.new_name}`.\n"
            f"Set `{self.new_name}` and unset `{self.old_name}` to avoid this deprecation warning and "
            "ensure it works properly in a future release."
        )
        return line_wrap_message(warning_tag(f"Deprecated functionality\n\n{description}"))


class ConfigLogPathDeprecation(WarnLevel):
    def code(self) -> str:
        return "D010"

    def message(self) -> str:
        output = "logs"
        cli_flag = "--log-path"
        env_var = "DBT_LOG_PATH"
        description = (
            f"The `{self.deprecated_path}` config in `dbt_project.yml` has been deprecated, "
            f"and will no longer be supported in a future version of dbt-core. "
            f"If you wish to write dbt {output} to a custom directory, please use "
            f"the {cli_flag} CLI flag or {env_var} env var instead."
        )
        return line_wrap_message(warning_tag(f"Deprecated functionality\n\n{description}"))


class ConfigTargetPathDeprecation(WarnLevel):
    def code(self) -> str:
        return "D011"

    def message(self) -> str:
        output = "artifacts"
        cli_flag = "--target-path"
        env_var = "DBT_TARGET_PATH"
        description = (
            f"The `{self.deprecated_path}` config in `dbt_project.yml` has been deprecated, "
            f"and will no longer be supported in a future version of dbt-core. "
            f"If you wish to write dbt {output} to a custom directory, please use "
            f"the {cli_flag} CLI flag or {env_var} env var instead."
        )
        return line_wrap_message(warning_tag(f"Deprecated functionality\n\n{description}"))


# Note: this deprecation has been removed, but we are leaving
# the event class here, because users may have specified it in
# warn_error_options.
class TestsConfigDeprecation(WarnLevel):
    def code(self) -> str:
        return "D012"

    def message(self) -> str:
        description = (
            f"The `{self.deprecated_path}` config has been renamed to `{self.exp_path}`. "
            "Please see https://docs.getdbt.com/docs/build/data-tests#new-data_tests-syntax for more information."
        )
        return line_wrap_message(warning_tag(f"Deprecated functionality\n\n{description}"))


class ProjectFlagsMovedDeprecation(WarnLevel):
    def code(self) -> str:
        return "D013"

    def message(self) -> str:
        description = (
            "User config should be moved from the 'config' key in profiles.yml to the 'flags' "
            "key in dbt_project.yml."
        )
        # Can't use line_wrap_message here because flags.printer_width isn't available yet
        return warning_tag(f"Deprecated functionality\n\n{description}")


class SpacesInResourceNameDeprecation(DynamicLevel):
    def code(self) -> str:
        return "D014"

    def message(self) -> str:
        description = f"Found spaces in the name of `{self.unique_id}`"

        if self.level == EventLevel.ERROR.value:
            description = error_tag(description)
        elif self.level == EventLevel.WARN.value:
            description = warning_tag(description)

        return line_wrap_message(description)


class ResourceNamesWithSpacesDeprecation(WarnLevel):
    def code(self) -> str:
        return "D015"

    def message(self) -> str:
        description = f"Spaces found in {self.count_invalid_names} resource name(s). This is deprecated, and may lead to errors when using dbt."

        if self.show_debug_hint:
            description += " Run again with `--debug` to see them all."

        description += " For more information: https://docs.getdbt.com/reference/global-configs/legacy-behaviors"

        return line_wrap_message(warning_tag(description))


class PackageMaterializationOverrideDeprecation(WarnLevel):
    def code(self) -> str:
        return "D016"

    def message(self) -> str:
        description = f"Installed package '{self.package_name}' is overriding the built-in materialization '{self.materialization_name}'. Overrides of built-in materializations from installed packages will be deprecated in future versions of dbt. For more information: https://docs.getdbt.com/reference/global-configs/legacy-behaviors"

        return line_wrap_message(warning_tag(description))


class SourceFreshnessProjectHooksNotRun(WarnLevel):
    def code(self) -> str:
        return "D017"

    def message(self) -> str:
        description = "In a future version of dbt, the `source freshness` command will start running `on-run-start` and `on-run-end` hooks by default. For more information: https://docs.getdbt.com/reference/global-configs/legacy-behaviors"

        return line_wrap_message(warning_tag(description))


class MFTimespineWithoutYamlConfigurationDeprecation(WarnLevel):
    def code(self) -> str:
        return "D018"

    def message(self) -> str:
        description = "Time spines without YAML configuration are in the process of deprecation. Please add YAML configuration for your 'metricflow_time_spine' model. See documentation on MetricFlow time spines: https://docs.getdbt.com/docs/build/metricflow-time-spine and behavior change documentation: https://docs.getdbt.com/reference/global-configs/behavior-changes."

        return line_wrap_message(warning_tag(description))


class MFCumulativeTypeParamsDeprecation(WarnLevel):
    def code(self) -> str:
        return "D019"

    def message(self) -> str:
        description = "Cumulative fields `type_params.window` and `type_params.grain_to_date` have been moved and will soon be deprecated. Please nest those values under `type_params.cumulative_type_params.window` and `type_params.cumulative_type_params.grain_to_date`. See documentation on behavior changes: https://docs.getdbt.com/reference/global-configs/behavior-changes."

        return line_wrap_message(warning_tag(description))


class MicrobatchMacroOutsideOfBatchesDeprecation(WarnLevel):
    def code(self) -> str:
        return "D020"

    def message(self) -> str:
        description = "The use of a custom microbatch macro outside of batched execution is deprecated. To use it with batched execution, set `flags.require_batched_execution_for_custom_microbatch_strategy` to `True` in `dbt_project.yml`. In the future this will be the default behavior."

        return line_wrap_message(warning_tag(description))


# =======================================================
# I - Project parsing
# =======================================================


class InputFileDiffError(DebugLevel):
    def code(self) -> str:
        return "I001"

    def message(self) -> str:
        return f"Error processing file diff: {self.category}, {self.file_id}"


# Skipping I003, I004, I005, I006, I007


class InvalidValueForField(WarnLevel):
    def code(self) -> str:
        return "I008"

    def message(self) -> str:
        return f"Invalid value ({self.field_value}) for field {self.field_name}"


class ValidationWarning(WarnLevel):
    def code(self) -> str:
        return "I009"

    def message(self) -> str:
        return f"Field {self.field_name} is not valid for {self.resource_type} ({self.node_name})"


class ParsePerfInfoPath(InfoLevel):
    def code(self) -> str:
        return "I010"

    def message(self) -> str:
        return f"Performance info: {self.path}"


# Removed I011: GenericTestFileParse


# Removed I012: MacroFileParse


# Skipping I013


class PartialParsingErrorProcessingFile(DebugLevel):
    def code(self) -> str:
        return "I014"

    def message(self) -> str:
        return f"Partial parsing exception processing file {self.file}"


# Skipped I015


class PartialParsingError(DebugLevel):
    def code(self) -> str:
        return "I016"

    def message(self) -> str:
        return f"PP exception info: {self.exc_info}"


class PartialParsingSkipParsing(InfoLevel):
    def code(self) -> str:
        return "I017"

    def message(self) -> str:
        return "Partial parsing enabled, no changes found, skipping parsing"


# Skipped I018, I019, I020, I021, I022, I023


class UnableToPartialParse(InfoLevel):
    def code(self) -> str:
        return "I024"

    def message(self) -> str:
        return f"Unable to do partial parsing because {self.reason}"


class StateCheckVarsHash(DebugLevel):
    def code(self) -> str:
        return "I025"

    def message(self) -> str:
        return f"checksum: {self.checksum}, vars: {self.vars}, profile: {self.profile}, target: {self.target}, version: {self.version}"


# Skipped I025, I026, I026, I027


class PartialParsingNotEnabled(InfoLevel):
    def code(self) -> str:
        return "I028"

    def message(self) -> str:
        return "Partial parsing not enabled"


class ParsedFileLoadFailed(DebugLevel):
    def code(self) -> str:
        return "I029"

    def message(self) -> str:
        return f"Failed to load parsed file from disk at {self.path}: {self.exc}"


# Skipped I030-I039


class PartialParsingEnabled(InfoLevel):
    def code(self) -> str:
        return "I040"

    def message(self) -> str:
        return (
            f"Partial parsing enabled: "
            f"{self.deleted} files deleted, "
            f"{self.added} files added, "
            f"{self.changed} files changed."
        )


class PartialParsingFile(DebugLevel):
    def code(self) -> str:
        return "I041"

    def message(self) -> str:
        return f"Partial parsing: {self.operation} file: {self.file_id}"


# Skipped I042, I043, I044, I045, I046, I047, I048, I049


class InvalidDisabledTargetInTestNode(DebugLevel):
    def code(self) -> str:
        return "I050"

    def message(self) -> str:
        target_package_string = ""

        if self.target_package != target_package_string:
            target_package_string = f"in package '{self.target_package}' "

        msg = (
            f"{self.resource_type_title} '{self.unique_id}' "
            f"({self.original_file_path}) depends on a {self.target_kind} "
            f"named '{self.target_name}' {target_package_string}which is disabled"
        )

        return warning_tag(msg)


class UnusedResourceConfigPath(WarnLevel):
    def code(self) -> str:
        return "I051"

    def message(self) -> str:
        path_list = "\n".join(f"- {u}" for u in self.unused_config_paths)
        msg = (
            "Configuration paths exist in your dbt_project.yml file which do not "
            "apply to any resources.\n"
            f"There are {len(self.unused_config_paths)} unused configuration paths:\n{path_list}"
        )
        return warning_tag(msg)


class SeedIncreased(WarnLevel):
    def code(self) -> str:
        return "I052"

    def message(self) -> str:
        msg = (
            f"Found a seed ({self.package_name}.{self.name}) "
            f">{MAXIMUM_SEED_SIZE_NAME} in size. The previous file was "
            f"<={MAXIMUM_SEED_SIZE_NAME}, so it has changed"
        )
        return msg


class SeedExceedsLimitSamePath(WarnLevel):
    def code(self) -> str:
        return "I053"

    def message(self) -> str:
        msg = (
            f"Found a seed ({self.package_name}.{self.name}) "
            f">{MAXIMUM_SEED_SIZE_NAME} in size at the same path, dbt "
            f"cannot tell if it has changed: assuming they are the same"
        )
        return msg


class SeedExceedsLimitAndPathChanged(WarnLevel):
    def code(self) -> str:
        return "I054"

    def message(self) -> str:
        msg = (
            f"Found a seed ({self.package_name}.{self.name}) "
            f">{MAXIMUM_SEED_SIZE_NAME} in size. The previous file was in "
            f"a different location, assuming it has changed"
        )
        return msg


class SeedExceedsLimitChecksumChanged(WarnLevel):
    def code(self) -> str:
        return "I055"

    def message(self) -> str:
        msg = (
            f"Found a seed ({self.package_name}.{self.name}) "
            f">{MAXIMUM_SEED_SIZE_NAME} in size. The previous file had a "
            f"checksum type of {self.checksum_name}, so it has changed"
        )
        return msg


class UnusedTables(WarnLevel):
    def code(self) -> str:
        return "I056"

    def message(self) -> str:
        msg = [
            "During parsing, dbt encountered source overrides that had no target:",
        ]
        msg += self.unused_tables
        msg.append("")
        return warning_tag("\n".join(msg))


class WrongResourceSchemaFile(WarnLevel):
    def code(self) -> str:
        return "I057"

    def message(self) -> str:
        msg = line_wrap_message(
            f"""\
            '{self.patch_name}' is a {self.resource_type} node, but it is
            specified in the {self.yaml_key} section of
            {self.file_path}.
            To fix this error, place the `{self.patch_name}`
            specification under the {self.plural_resource_type} key instead.
            """
        )
        return warning_tag(msg)


class NoNodeForYamlKey(WarnLevel):
    def code(self) -> str:
        return "I058"

    def message(self) -> str:
        msg = (
            f"Did not find matching node for patch with name '{self.patch_name}' "
            f"in the '{self.yaml_key}' section of "
            f"file '{self.file_path}'"
        )
        return warning_tag(msg)


class MacroNotFoundForPatch(WarnLevel):
    def code(self) -> str:
        return "I059"

    def message(self) -> str:
        msg = f'Found patch for macro "{self.patch_name}" which was not found'
        return warning_tag(msg)


class NodeNotFoundOrDisabled(WarnLevel):
    def code(self) -> str:
        return "I060"

    def message(self) -> str:
        # this is duplicated logic from exceptions.get_not_found_or_disabled_msg
        # when we convert exceptions to be structured maybe it can be combined?
        # converting the bool to a string since None is also valid
        if self.disabled == "None":
            reason = "was not found or is disabled"
        elif self.disabled == "True":
            reason = "is disabled"
        else:
            reason = "was not found"

        target_package_string = ""

        if self.target_package is not None:
            target_package_string = f"in package '{self.target_package}' "

        msg = (
            f"{self.resource_type_title} '{self.unique_id}' "
            f"({self.original_file_path}) depends on a {self.target_kind} "
            f"named '{self.target_name}' {target_package_string}which {reason}"
        )

        return warning_tag(msg)


class JinjaLogWarning(WarnLevel):
    def code(self) -> str:
        return "I061"

    def message(self) -> str:
        return self.msg


class JinjaLogInfo(InfoLevel):
    def code(self) -> str:
        return "I062"

    def message(self) -> str:
        # This is for the log method used in macros so msg cannot be built here
        return self.msg


class JinjaLogDebug(DebugLevel):
    def code(self) -> str:
        return "I063"

    def message(self) -> str:
        # This is for the log method used in macros so msg cannot be built here
        return self.msg


class UnpinnedRefNewVersionAvailable(InfoLevel):
    def code(self) -> str:
        return "I064"

    def message(self) -> str:
        msg = (
            f"While compiling '{self.node_info.node_name}':\n"
            f"Found an unpinned reference to versioned model '{self.ref_node_name}' in project '{self.ref_node_package}'.\n"
            f"Resolving to latest version: {self.ref_node_name}.v{self.ref_node_version}\n"
            f"A prerelease version {self.ref_max_version} is available. It has not yet been marked 'latest' by its maintainer.\n"
            f"When that happens, this reference will resolve to {self.ref_node_name}.v{self.ref_max_version} instead.\n\n"
            f"  Try out v{self.ref_max_version}: {{{{ ref('{self.ref_node_package}', '{self.ref_node_name}', v='{self.ref_max_version}') }}}}\n"
            f"  Pin to  v{self.ref_node_version}: {{{{ ref('{self.ref_node_package}', '{self.ref_node_name}', v='{self.ref_node_version}') }}}}\n"
        )
        return msg


class UpcomingReferenceDeprecation(WarnLevel):
    def code(self) -> str:
        return "I066"

    def message(self) -> str:
        ref_model_version = ".v" + self.ref_model_version if self.ref_model_version else ""
        msg = (
            f"While compiling '{self.model_name}': Found a reference to {self.ref_model_name}{ref_model_version}, "
            f"which is slated for deprecation on '{self.ref_model_deprecation_date}'. "
        )

        if self.ref_model_version and self.ref_model_version != self.ref_model_latest_version:
            coda = (
                f"A new version of '{self.ref_model_name}' is available. Try it out: "
                f"{{{{ ref('{self.ref_model_package}', '{self.ref_model_name}', "
                f"v='{self.ref_model_latest_version}') }}}}."
            )
            msg = msg + coda

        return warning_tag(msg)


class DeprecatedReference(WarnLevel):
    def code(self) -> str:
        return "I067"

    def message(self) -> str:
        ref_model_version = ".v" + self.ref_model_version if self.ref_model_version else ""
        msg = (
            f"While compiling '{self.model_name}': Found a reference to {self.ref_model_name}{ref_model_version}, "
            f"which was deprecated on '{self.ref_model_deprecation_date}'. "
        )

        if self.ref_model_version and self.ref_model_version != self.ref_model_latest_version:
            coda = (
                f"A new version of '{self.ref_model_name}' is available. Migrate now: "
                f"{{{{ ref('{self.ref_model_package}', '{self.ref_model_name}', "
                f"v='{self.ref_model_latest_version}') }}}}."
            )
            msg = msg + coda

        return warning_tag(msg)


class UnsupportedConstraintMaterialization(WarnLevel):
    def code(self) -> str:
        return "I068"

    def message(self) -> str:
        msg = (
            f"Constraint types are not supported for {self.materialized} materializations and will "
            "be ignored.  Set 'warn_unsupported: false' on this constraint to ignore this warning."
        )

        return line_wrap_message(warning_tag(msg))


class ParseInlineNodeError(ErrorLevel):
    def code(self) -> str:
        return "I069"

    def message(self) -> str:
        return "Error while parsing node: " + self.node_info.node_name + "\n" + self.exc


class SemanticValidationFailure(WarnLevel):
    def code(self) -> str:
        return "I070"

    def message(self) -> str:
        return self.msg


class UnversionedBreakingChange(WarnLevel):
    def code(self) -> str:
        return "I071"

    def message(self) -> str:
        reasons = "\n  - ".join(self.breaking_changes)

        msg = (
            f"Breaking change to contracted, unversioned model {self.model_name} ({self.model_file_path})"
            "\nWhile comparing to previous project state, dbt detected a breaking change to an unversioned model."
            f"\n  - {reasons}\n"
        )

        return warning_tag(msg)


class WarnStateTargetEqual(WarnLevel):
    def code(self) -> str:
        return "I072"

    def message(self) -> str:
        return yellow(
            f"Warning: The state and target directories are the same: '{self.state_path}'. "
            f"This could lead to missing changes due to overwritten state including non-idempotent retries."
        )


class FreshnessConfigProblem(WarnLevel):
    def code(self) -> str:
        return "I073"

    def message(self) -> str:
        return self.msg


class MicrobatchModelNoEventTimeInputs(WarnLevel):
    def code(self) -> str:
        return "I074"

    def message(self) -> str:
        msg = (
            f"The microbatch model '{self.model_name}' has no 'ref' or 'source' input with an 'event_time' configuration. "
            "\nThis means no filtering can be applied and can result in unexpected duplicate records in the resulting microbatch model."
        )

        return warning_tag(msg)


class InvalidConcurrentBatchesConfig(WarnLevel):
    def code(self) -> str:
        return "I075"

    def message(self) -> str:
        maybe_plural_count_of_models = pluralize(self.num_models, "microbatch model")
        description = f"Found {maybe_plural_count_of_models} with the `concurrent_batches` config set to true, but the {self.adapter_type} adapter does not support running batches concurrently. Batches will be run sequentially."
        return line_wrap_message(warning_tag(description))


# =======================================================
# M - Deps generation
# =======================================================


class GitSparseCheckoutSubdirectory(DebugLevel):
    def code(self) -> str:
        return "M001"

    def message(self) -> str:
        return f"Subdirectory specified: {self.subdir}, using sparse checkout."


class GitProgressCheckoutRevision(DebugLevel):
    def code(self) -> str:
        return "M002"

    def message(self) -> str:
        return f"Checking out revision {self.revision}."


class GitProgressUpdatingExistingDependency(DebugLevel):
    def code(self) -> str:
        return "M003"

    def message(self) -> str:
        return f"Updating existing dependency {self.dir}."


class GitProgressPullingNewDependency(DebugLevel):
    def code(self) -> str:
        return "M004"

    def message(self) -> str:
        return f"Pulling new dependency {self.dir}."


class GitNothingToDo(DebugLevel):
    def code(self) -> str:
        return "M005"

    def message(self) -> str:
        return f"Already at {self.sha}, nothing to do."


class GitProgressUpdatedCheckoutRange(DebugLevel):
    def code(self) -> str:
        return "M006"

    def message(self) -> str:
        return f"Updated checkout from {self.start_sha} to {self.end_sha}."


class GitProgressCheckedOutAt(DebugLevel):
    def code(self) -> str:
        return "M007"

    def message(self) -> str:
        return f"Checked out at {self.end_sha}."


class RegistryProgressGETRequest(DebugLevel):
    def code(self) -> str:
        return "M008"

    def message(self) -> str:
        return f"Making package registry request: GET {self.url}"


class RegistryProgressGETResponse(DebugLevel):
    def code(self) -> str:
        return "M009"

    def message(self) -> str:
        return f"Response from registry: GET {self.url} {self.resp_code}"


class SelectorReportInvalidSelector(InfoLevel):
    def code(self) -> str:
        return "M010"

    def message(self) -> str:
        return (
            f"The '{self.spec_method}' selector specified in {self.raw_spec} is "
            f"invalid. Must be one of [{self.valid_selectors}]"
        )


class DepsNoPackagesFound(InfoLevel):
    def code(self) -> str:
        return "M013"

    def message(self) -> str:
        return "Warning: No packages were found in packages.yml"


class DepsStartPackageInstall(InfoLevel):
    def code(self) -> str:
        return "M014"

    def message(self) -> str:
        return f"Installing {self.package_name}"


class DepsInstallInfo(InfoLevel):
    def code(self) -> str:
        return "M015"

    def message(self) -> str:
        return f"Installed from {self.version_name}"


class DepsUpdateAvailable(InfoLevel):
    def code(self) -> str:
        return "M016"

    def message(self) -> str:
        return f"Updated version available: {self.version_latest}"


class DepsUpToDate(InfoLevel):
    def code(self) -> str:
        return "M017"

    def message(self) -> str:
        return "Up to date!"


class DepsListSubdirectory(InfoLevel):
    def code(self) -> str:
        return "M018"

    def message(self) -> str:
        return f"and subdirectory {self.subdirectory}"


class DepsNotifyUpdatesAvailable(InfoLevel):
    def code(self) -> str:
        return "M019"

    def message(self) -> str:
        return f"Updates available for packages: {self.packages} \
                \nUpdate your versions in packages.yml, then run dbt deps"


class RegistryIndexProgressGETRequest(DebugLevel):
    def code(self) -> str:
        return "M022"

    def message(self) -> str:
        return f"Making package index registry request: GET {self.url}"


class RegistryIndexProgressGETResponse(DebugLevel):
    def code(self) -> str:
        return "M023"

    def message(self) -> str:
        return f"Response from registry index: GET {self.url} {self.resp_code}"


class RegistryResponseUnexpectedType(DebugLevel):
    def code(self) -> str:
        return "M024"

    def message(self) -> str:
        return f"Response was None: {self.response}"


class RegistryResponseMissingTopKeys(DebugLevel):
    def code(self) -> str:
        return "M025"

    def message(self) -> str:
        # expected/actual keys logged in exception
        return f"Response missing top level keys: {self.response}"


class RegistryResponseMissingNestedKeys(DebugLevel):
    def code(self) -> str:
        return "M026"

    def message(self) -> str:
        # expected/actual keys logged in exception
        return f"Response missing nested keys: {self.response}"


class RegistryResponseExtraNestedKeys(DebugLevel):
    def code(self) -> str:
        return "M027"

    def message(self) -> str:
        # expected/actual keys logged in exception
        return f"Response contained inconsistent keys: {self.response}"


class DepsSetDownloadDirectory(DebugLevel):
    def code(self) -> str:
        return "M028"

    def message(self) -> str:
        return f"Set downloads directory='{self.path}'"


class DepsUnpinned(WarnLevel):
    def code(self) -> str:
        return "M029"

    def message(self) -> str:
        if self.revision == "HEAD":
            unpinned_msg = "not pinned, using HEAD (default branch)"
        elif self.revision in ("main", "master"):
            unpinned_msg = f'pinned to the "{self.revision}" branch'
        else:
            unpinned_msg = None

        msg = (
            f'The git package "{self.git}" \n\tis {unpinned_msg}.\n\tThis can introduce '
            f"breaking changes into your project without warning!\n\nSee {PIN_PACKAGE_URL}"
        )
        return yellow(f"WARNING: {msg}")


class NoNodesForSelectionCriteria(WarnLevel):
    def code(self) -> str:
        return "M030"

    def message(self) -> str:
        return f"The selection criterion '{self.spec_raw}' does not match any enabled nodes"


class DepsLockUpdating(InfoLevel):
    def code(self):
        return "M031"

    def message(self) -> str:
        return f"Updating lock file in file path: {self.lock_filepath}"


class DepsAddPackage(InfoLevel):
    def code(self):
        return "M032"

    def message(self) -> str:
        return f"Added new package {self.package_name}@{self.version} to {self.packages_filepath}"


class DepsFoundDuplicatePackage(InfoLevel):
    def code(self):
        return "M033"

    def message(self) -> str:
        return f"Found duplicate package in packages.yml, removing: {self.removed_package}"


class DepsScrubbedPackageName(WarnLevel):
    def code(self):
        return "M035"

    def message(self) -> str:
        return f"Detected secret env var in {self.package_name}. dbt will write a scrubbed representation to the lock file. This will cause issues with subsequent 'dbt deps' using the lock file, requiring 'dbt deps --upgrade'"


# =======================================================
# P - Artifacts
# =======================================================


class ArtifactWritten(DebugLevel):
    def code(self):
        return "P001"

    def message(self) -> str:
        return f"Wrote artifact {self.artifact_type} to {self.artifact_path}"


# =======================================================
# Q - Node execution
# =======================================================


class RunningOperationCaughtError(ErrorLevel):
    def code(self) -> str:
        return "Q001"

    def message(self) -> str:
        return f"Encountered an error while running operation: {self.exc}"


class CompileComplete(InfoLevel):
    def code(self) -> str:
        return "Q002"

    def message(self) -> str:
        return "Done."


class FreshnessCheckComplete(InfoLevel):
    def code(self) -> str:
        return "Q003"

    def message(self) -> str:
        return "Done."


class SeedHeader(InfoLevel):
    def code(self) -> str:
        return "Q004"

    def message(self) -> str:
        return self.header


class SQLRunnerException(DebugLevel):
    def code(self) -> str:
        return "Q006"

    def message(self) -> str:
        return f"Got an exception: {self.exc}"


class LogTestResult(DynamicLevel):
    def code(self) -> str:
        return "Q007"

    def message(self) -> str:
        if self.status == "error":
            info = "ERROR"
            status = red(
                info,
            )
        elif self.status == "pass":
            info = "PASS"
            status = green(info)
        elif self.status == "warn":
            info = f"WARN {self.num_failures}"
            status = yellow(info)
        else:  # self.status == "fail":
            info = f"FAIL {self.num_failures}"
            status = red(info)
        msg = f"{info} {self.name}"

        return format_fancy_output_line(
            msg=msg,
            status=status,
            index=self.index,
            total=self.num_models,
            execution_time=self.execution_time,
        )

    @classmethod
    def status_to_level(cls, status):
        # The statuses come from TestStatus
        level_lookup = {
            "fail": EventLevel.ERROR,
            "pass": EventLevel.INFO,
            "warn": EventLevel.WARN,
            "error": EventLevel.ERROR,
        }
        if status in level_lookup:
            return level_lookup[status]
        else:
            return EventLevel.INFO


class LogNodeResult(DynamicLevel):
    def code(self) -> str:
        return "Q008"

    def message(self) -> str:
        return self.msg


# Skipped Q009, Q010


class LogStartLine(InfoLevel):
    def code(self) -> str:
        return "Q011"

    def message(self) -> str:
        msg = f"START {self.description}"
        return format_fancy_output_line(msg=msg, status="RUN", index=self.index, total=self.total)


class LogModelResult(DynamicLevel):
    def code(self) -> str:
        return "Q012"

    def message(self) -> str:
        if self.status == "error":
            info = "ERROR creating"
            status = red(self.status.upper())
        elif "PARTIAL SUCCESS" in self.status:
            info = "PARTIALLY created"
            status = yellow(self.status.upper())
        else:
            info = "OK created"
            status = green(self.status)

        msg = f"{info} {self.description}"
        return format_fancy_output_line(
            msg=msg,
            status=status,
            index=self.index,
            total=self.total,
            execution_time=self.execution_time,
        )


# Skipped Q013, Q014


class LogSnapshotResult(DynamicLevel):
    def code(self) -> str:
        return "Q015"

    def message(self) -> str:
        if self.status == "error":
            info = "ERROR snapshotting"
            status = red(self.status.upper())
        else:
            info = "OK snapshotted"
            status = green(self.result_message)

        msg = "{info} {description}".format(info=info, description=self.description, **self.cfg)
        return format_fancy_output_line(
            msg=msg,
            status=status,
            index=self.index,
            total=self.total,
            execution_time=self.execution_time,
        )


class LogSeedResult(DynamicLevel):
    def code(self) -> str:
        return "Q016"

    def message(self) -> str:
        if self.status == "error":
            info = "ERROR loading"
            status = red(self.status.upper())
        else:
            info = "OK loaded"
            status = green(self.result_message)
        msg = f"{info} seed file {self.schema}.{self.relation}"
        return format_fancy_output_line(
            msg=msg,
            status=status,
            index=self.index,
            total=self.total,
            execution_time=self.execution_time,
        )


# Skipped Q017


class LogFreshnessResult(DynamicLevel):
    def code(self) -> str:
        return "Q018"

    def message(self) -> str:
        if self.status == "runtime error":
            info = "ERROR"
            status = red(info)
        elif self.status == "error":
            info = "ERROR STALE"
            status = red(info)
        elif self.status == "warn":
            info = "WARN"
            status = yellow(info)
        else:
            info = "PASS"
            status = green(info)
        msg = f"{info} freshness of {self.source_name}.{self.table_name}"
        return format_fancy_output_line(
            msg=msg,
            status=status,
            index=self.index,
            total=self.total,
            execution_time=self.execution_time,
        )

    @classmethod
    def status_to_level(cls, status):
        # The statuses come from FreshnessStatus
        # TODO should this return EventLevel enum instead?
        level_lookup = {
            "runtime error": EventLevel.ERROR,
            "pass": EventLevel.INFO,
            "warn": EventLevel.WARN,
            "error": EventLevel.ERROR,
        }
        if status in level_lookup:
            return level_lookup[status]
        else:
            return EventLevel.INFO


class LogNodeNoOpResult(InfoLevel):
    def code(self) -> str:
        return "Q019"

    def message(self) -> str:
        msg = f"NO-OP {self.description}"
        return format_fancy_output_line(
            msg=msg,
            status=yellow("NO-OP"),
            index=self.index,
            total=self.total,
            execution_time=self.execution_time,
        )


# Skipped Q020, Q021


class LogCancelLine(ErrorLevel):
    def code(self) -> str:
        return "Q022"

    def message(self) -> str:
        msg = f"CANCEL query {self.conn_name}"
        return format_fancy_output_line(msg=msg, status=red("CANCEL"), index=None, total=None)


class DefaultSelector(InfoLevel):
    def code(self) -> str:
        return "Q023"

    def message(self) -> str:
        return f"Using default selector {self.name}"


class NodeStart(DebugLevel):
    def code(self) -> str:
        return "Q024"

    def message(self) -> str:
        return f"Began running node {self.node_info.unique_id}"


class NodeFinished(DebugLevel):
    def code(self) -> str:
        return "Q025"

    def message(self) -> str:
        return f"Finished running node {self.node_info.unique_id}"


class QueryCancelationUnsupported(InfoLevel):
    def code(self) -> str:
        return "Q026"

    def message(self) -> str:
        msg = (
            f"The {self.type} adapter does not support query "
            "cancellation. Some queries may still be "
            "running!"
        )
        return yellow(msg)


class ConcurrencyLine(InfoLevel):
    def code(self) -> str:
        return "Q027"

    def message(self) -> str:
        return f"Concurrency: {self.num_threads} threads (target='{self.target_name}')"


class WritingInjectedSQLForNode(DebugLevel):
    def code(self) -> str:
        return "Q029"

    def message(self) -> str:
        return f'Writing injected SQL for node "{self.node_info.unique_id}"'


class NodeCompiling(DebugLevel):
    def code(self) -> str:
        return "Q030"

    def message(self) -> str:
        return f"Began compiling node {self.node_info.unique_id}"


class NodeExecuting(DebugLevel):
    def code(self) -> str:
        return "Q031"

    def message(self) -> str:
        return f"Began executing node {self.node_info.unique_id}"


class LogHookStartLine(InfoLevel):
    def code(self) -> str:
        return "Q032"

    def message(self) -> str:
        msg = f"START hook: {self.statement}"
        return format_fancy_output_line(
            msg=msg, status="RUN", index=self.index, total=self.total, truncate=True
        )


class LogHookEndLine(InfoLevel):
    def code(self) -> str:
        return "Q033"

    def message(self) -> str:
        if self.status == "success":
            info = "OK"
            status = green(info)
        elif self.status == "skipped":
            info = "SKIP"
            status = yellow(info)
        else:
            info = "ERROR"
            status = red(info)
        msg = f"{info} hook: {self.statement}"

        return format_fancy_output_line(
            msg=msg,
            status=status,
            index=self.index,
            total=self.total,
            execution_time=self.execution_time,
            truncate=True,
        )


class SkippingDetails(InfoLevel):
    def code(self) -> str:
        return "Q034"

    def message(self) -> str:
        # ToDo: move to core or figure out NodeType
        if self.resource_type in ["model", "seed", "snapshot"]:
            msg = f"SKIP relation {self.schema}.{self.node_name}"
        else:
            msg = f"SKIP {self.resource_type} {self.node_name}"
        return format_fancy_output_line(
            msg=msg, status=yellow("SKIP"), index=self.index, total=self.total
        )


class NothingToDo(WarnLevel):
    def code(self) -> str:
        return "Q035"

    def message(self) -> str:
        return "Nothing to do. Try checking your model configs and model specification args"


class RunningOperationUncaughtError(ErrorLevel):
    def code(self) -> str:
        return "Q036"

    def message(self) -> str:
        return f"Encountered an error while running operation: {self.exc}"


class EndRunResult(DebugLevel):
    def code(self) -> str:
        return "Q037"

    def message(self) -> str:
        return "Command end result"


class NoNodesSelected(WarnLevel):
    def code(self) -> str:
        return "Q038"

    def message(self) -> str:
        return "No nodes selected!"


class CommandCompleted(DebugLevel):
    def code(self) -> str:
        return "Q039"

    def message(self) -> str:
        status = "succeeded" if self.success else "failed"
        completed_at = timestamp_to_datetime_string(self.completed_at)
        return f"Command `{self.command}` {status} at {completed_at} after {self.elapsed:0.2f} seconds"


class ShowNode(InfoLevel):
    def code(self) -> str:
        return "Q041"

    def message(self) -> str:
        if self.output_format == "json":
            if self.is_inline:
                return json.dumps({"show": json.loads(self.preview)}, indent=2)
            else:
                return json.dumps(
                    {"node": self.node_name, "show": json.loads(self.preview)}, indent=2
                )
        else:
            if self.quiet:
                return self.preview
            elif self.is_inline:
                return f"Previewing inline node:\n{self.preview}"
            else:
                return f"Previewing node '{self.node_name}':\n{self.preview}"


class CompiledNode(InfoLevel):
    def code(self) -> str:
        return "Q042"

    def message(self) -> str:
        if self.output_format == "json":
            if self.is_inline:
                return json.dumps({"compiled": self.compiled}, indent=2)
            else:
                return json.dumps({"node": self.node_name, "compiled": self.compiled}, indent=2)
        else:
            if self.quiet:
                return self.compiled
            elif self.is_inline:
                return f"Compiled inline node is:\n{self.compiled}"
            else:
                return f"Compiled node '{self.node_name}' is:\n{self.compiled}"


class SnapshotTimestampWarning(WarnLevel):
    def code(self) -> str:
        return "Q043"

    def message(self) -> str:
        return (
            f"Data type of snapshot table timestamp columns ({self.snapshot_time_data_type}) "
            f"doesn't match derived column 'updated_at' ({self.updated_at_data_type}). "
            "Please update snapshot config 'updated_at'."
        )


class MicrobatchExecutionDebug(DebugLevel):
    def code(self) -> str:
        return "Q044"

    def message(self) -> str:
        return self.msg


class LogStartBatch(InfoLevel):
    def code(self) -> str:
        return "Q045"

    def message(self) -> str:
        msg = f"START {self.description}"

        # TODO update common so that we can append "batch" in `format_fancy_output_line`
        formatted = format_fancy_output_line(
            msg=msg,
            status="RUN",
            index=self.batch_index,
            total=self.total_batches,
        )
        return f"Batch {formatted}"


class LogBatchResult(DynamicLevel):
    def code(self) -> str:
        return "Q046"

    def message(self) -> str:
        if self.status == "error":
            info = "ERROR creating"
            status = red(self.status.upper())
        elif self.status == "skipped":
            info = "SKIP"
            status = yellow(self.status.upper())
        else:
            info = "OK created"
            status = green(self.status)

        msg = f"{info} {self.description}"

        # TODO update common so that we can append "batch" in `format_fancy_output_line`
        formatted = format_fancy_output_line(
            msg=msg,
            status=status,
            index=self.batch_index,
            total=self.total_batches,
            execution_time=self.execution_time,
        )
        return f"Batch {formatted}"


# =======================================================
# W - Node testing
# =======================================================

# Skipped W001


class CatchableExceptionOnRun(DebugLevel):
    def code(self) -> str:
        return "W002"

    def message(self) -> str:
        return str(self.exc)


class InternalErrorOnRun(DebugLevel):
    def code(self) -> str:
        return "W003"

    def message(self) -> str:
        prefix = f"Internal error executing {self.build_path}"

        internal_error_string = """This is an error in dbt. Please try again. If \
the error persists, open an issue at https://github.com/dbt-labs/dbt-core
""".strip()

        return f"{red(prefix)}\n" f"{str(self.exc).strip()}\n\n" f"{internal_error_string}"


class GenericExceptionOnRun(ErrorLevel):
    def code(self) -> str:
        return "W004"

    def message(self) -> str:
        node_description = self.build_path
        if node_description is None:
            node_description = self.unique_id
        prefix = f"Unhandled error while executing {node_description}"
        return f"{red(prefix)}\n{str(self.exc).strip()}"


class NodeConnectionReleaseError(DebugLevel):
    def code(self) -> str:
        return "W005"

    def message(self) -> str:
        return f"Error releasing connection for node {self.node_name}: {str(self.exc)}"


class FoundStats(InfoLevel):
    def code(self) -> str:
        return "W006"

    def message(self) -> str:
        return f"Found {self.stat_line}"


# =======================================================
# Z - Misc
# =======================================================


class MainKeyboardInterrupt(InfoLevel):
    def code(self) -> str:
        return "Z001"

    def message(self) -> str:
        return "ctrl-c"


class MainEncounteredError(ErrorLevel):
    def code(self) -> str:
        return "Z002"

    def message(self) -> str:
        return f"Encountered an error:\n{self.exc}"


class MainStackTrace(ErrorLevel):
    def code(self) -> str:
        return "Z003"

    def message(self) -> str:
        return self.stack_trace


# Skipped Z004


class TimingInfoCollected(DebugLevel):
    def code(self) -> str:
        return "Z010"

    def message(self) -> str:
        started_at = timestamp_to_datetime_string(self.timing_info.started_at)
        completed_at = timestamp_to_datetime_string(self.timing_info.completed_at)
        return f"Timing info for {self.node_info.unique_id} ({self.timing_info.name}): {started_at} => {completed_at}"


# This prints the stack trace at the debug level while allowing just the nice exception message
# at the error level - or whatever other level chosen.  Used in multiple places.


class LogDebugStackTrace(DebugLevel):
    def code(self) -> str:
        return "Z011"

    def message(self) -> str:
        return f"{self.exc_info}"


# We don't write "clean" events to the log, because the clean command
# may have removed the log directory.


class CheckCleanPath(InfoLevel):
    def code(self) -> str:
        return "Z012"

    def message(self) -> str:
        return f"Checking {self.path}/*"


class ConfirmCleanPath(InfoLevel):
    def code(self) -> str:
        return "Z013"

    def message(self) -> str:
        return f"Cleaned {self.path}/*"


class ProtectedCleanPath(InfoLevel):
    def code(self) -> str:
        return "Z014"

    def message(self) -> str:
        return f"ERROR: not cleaning {self.path}/* because it is protected"


class FinishedCleanPaths(InfoLevel):
    def code(self) -> str:
        return "Z015"

    def message(self) -> str:
        return "Finished cleaning all paths."


class OpenCommand(InfoLevel):
    def code(self) -> str:
        return "Z016"

    def message(self) -> str:
        msg = f"""To view your profiles.yml file, run:

{self.open_cmd} {self.profiles_dir}"""

        return msg


class RunResultWarning(WarnLevel):
    def code(self) -> str:
        return "Z021"

    def message(self) -> str:
        info = "Warning"
        return yellow(f"{info} in {self.resource_type} {self.node_name} ({self.path})")


class RunResultFailure(ErrorLevel):
    def code(self) -> str:
        return "Z022"

    def message(self) -> str:
        info = "Failure"
        return red(f"{info} in {self.resource_type} {self.node_name} ({self.path})")


class StatsLine(InfoLevel):
    def code(self) -> str:
        return "Z023"

    def message(self) -> str:
        stats_line = (
            "Done. PASS={pass} WARN={warn} ERROR={error} SKIP={skip} NO-OP={noop} TOTAL={total}"
        )
        return stats_line.format(**self.stats)


class RunResultError(ErrorLevel):
    def code(self) -> str:
        return "Z024"

    def message(self) -> str:
        # This is the message on the result object, cannot be built here
        return f"  {self.msg}"


class RunResultErrorNoMessage(ErrorLevel):
    def code(self) -> str:
        return "Z025"

    def message(self) -> str:
        return f"  Status: {self.status}"


class SQLCompiledPath(InfoLevel):
    def code(self) -> str:
        return "Z026"

    def message(self) -> str:
        return f"  compiled code at {self.path}"


class CheckNodeTestFailure(InfoLevel):
    def code(self) -> str:
        return "Z027"

    def message(self) -> str:
        msg = f"select * from {self.relation_name}"
        border = "-" * len(msg)
        return f"  See test failures:\n  {border}\n  {msg}\n  {border}"


# Skipped Z028, Z029


class EndOfRunSummary(InfoLevel):
    def code(self) -> str:
        return "Z030"

    def message(self) -> str:
        error_plural = pluralize(self.num_errors, "error")
        warn_plural = pluralize(self.num_warnings, "warning")
        partial_success_plural = f"""{self.num_partial_success} partial {"success" if self.num_partial_success == 1 else "successes"}"""

        if self.keyboard_interrupt:
            message = yellow("Exited because of keyboard interrupt")
        elif self.num_errors > 0:
            message = red(
                f"Completed with {error_plural}, {partial_success_plural}, and {warn_plural}:"
            )
        elif self.num_partial_success > 0:
            message = yellow(f"Completed with {partial_success_plural} and {warn_plural}")
        elif self.num_warnings > 0:
            message = yellow(f"Completed with {warn_plural}:")
        else:
            message = green("Completed successfully")
        return message


# Skipped Z031, Z032


class MarkSkippedChildren(DebugLevel):
    def code(self) -> str:
        return "Z033"

    def message(self) -> str:
        msg = (
            f"Marking all children of '{self.unique_id}' to be skipped "
            f"because of status '{self.status}'. "
        )
        if self.run_result.message:
            msg = msg + f" Reason: {self.run_result.message}."
        return msg


class LogSkipBecauseError(ErrorLevel):
    def code(self) -> str:
        return "Z034"

    def message(self) -> str:
        msg = f"SKIP relation {self.schema}.{self.relation} due to ephemeral model status '{self.status}'"
        return format_fancy_output_line(
            msg=msg, status=red("ERROR SKIP"), index=self.index, total=self.total
        )


# Skipped Z035


class EnsureGitInstalled(ErrorLevel):
    def code(self) -> str:
        return "Z036"

    def message(self) -> str:
        return (
            "Make sure git is installed on your machine. More "
            "information: "
            "https://docs.getdbt.com/docs/package-management"
        )


class DepsCreatingLocalSymlink(DebugLevel):
    def code(self) -> str:
        return "Z037"

    def message(self) -> str:
        return "Creating symlink to local dependency."


class DepsSymlinkNotAvailable(DebugLevel):
    def code(self) -> str:
        return "Z038"

    def message(self) -> str:
        return "Symlinks are not available on this OS, copying dependency."


class DisableTracking(DebugLevel):
    def code(self) -> str:
        return "Z039"

    def message(self) -> str:
        return (
            "Error sending anonymous usage statistics. Disabling tracking for this execution. "
            "If you wish to permanently disable tracking, see: "
            "https://docs.getdbt.com/reference/global-configs#send-anonymous-usage-stats."
        )


class SendingEvent(DebugLevel):
    def code(self) -> str:
        return "Z040"

    def message(self) -> str:
        return f"Sending event: {self.kwargs}"


class SendEventFailure(DebugLevel):
    def code(self) -> str:
        return "Z041"

    def message(self) -> str:
        return "An error was encountered while trying to send an event"


class FlushEvents(DebugLevel):
    def code(self) -> str:
        return "Z042"

    def message(self) -> str:
        return "Flushing usage events"


class FlushEventsFailure(DebugLevel):
    def code(self) -> str:
        return "Z043"

    def message(self) -> str:
        return "An error was encountered while trying to flush usage events"


class TrackingInitializeFailure(DebugLevel):
    def code(self) -> str:
        return "Z044"

    def message(self) -> str:
        return "Got an exception trying to initialize tracking"


# this is the message from the result object


class RunResultWarningMessage(WarnLevel):
    def code(self) -> str:
        return "Z046"

    def message(self) -> str:
        # This is the message on the result object, cannot be formatted in event
        return self.msg


class DebugCmdOut(InfoLevel):
    def code(self) -> str:
        return "Z047"

    def message(self) -> str:
        return self.msg


class DebugCmdResult(InfoLevel):
    def code(self) -> str:
        return "Z048"

    def message(self) -> str:
        return self.msg


class ListCmdOut(InfoLevel):
    # No longer in use, switching to Z051 PrintEvent in dbt-common
    def code(self) -> str:
        return "Z049"

    def message(self) -> str:
        return self.msg


class ResourceReport(DebugLevel):
    def code(self) -> str:
        return "Z051"

    def message(self) -> str:
        return f"Resource report: {self.to_json()}"
