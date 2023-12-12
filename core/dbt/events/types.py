from dbt.events.base_types import WarnLevel, InfoLevel
from dbt.common.ui import warning_tag, line_wrap_message


# =======================================================
# E Adapter Events
# =======================================================


class AdapterRegistered(InfoLevel):
    """
    This event signifies that an adapter has been registered with dbt-core.
    While it is about an adapter it specifically signifies a point in the dbt runner's workflow
    """

    def code(self) -> str:
        return "E034"

    def message(self) -> str:
        return f"Registered adapter: {self.adapter_name}{self.adapter_version}"


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


# =======================================================
# M - Deps generation
# =======================================================


class DepsScrubbedPackageName(WarnLevel):
    def code(self):
        return "M035"

    def message(self) -> str:
        return f"Detected secret env var in {self.package_name}. dbt will write a scrubbed representation to the lock file. This will cause issues with subsequent 'dbt deps' using the lock file, requiring 'dbt deps --upgrade'"
