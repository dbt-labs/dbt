from typing import Any, Dict, Optional, Set, Tuple

from dbt import deprecations
from dbt.clients import yaml_helper
from dbt.events.v2_warn_error_options import V2_WARN_ERROR_OPTION_NAMES
from dbt.events.types import InvalidOptionYAML
from dbt.exceptions import DbtExclusivePropertyUseError, OptionNotYamlDictError
from dbt_common.events.functions import fire_event
from dbt_common.events.types import Note
from dbt_common.exceptions import DbtValidationError
from dbt_common.helper_types import WarnErrorOptionsV2


def parse_cli_vars(var_string: str) -> Dict[str, Any]:
    return parse_cli_yaml_string(var_string, "vars")


def parse_cli_yaml_string(var_string: str, cli_option_name: str) -> Dict[str, Any]:
    try:
        cli_vars = yaml_helper.load_yaml_text(var_string)
        var_type = type(cli_vars)
        if cli_vars is not None and var_type is dict:
            return cli_vars
        else:
            raise OptionNotYamlDictError(var_type, cli_option_name)
    except (DbtValidationError, OptionNotYamlDictError):
        fire_event(InvalidOptionYAML(option_name=cli_option_name))
        raise


def exclusive_primary_alt_value_setting(
    dictionary: Optional[Dict[str, Any]],
    primary: str,
    alt: str,
    parent_config: Optional[str] = None,
) -> None:
    """Munges in place under the primary the options for the primary and alt values

    Sometimes we allow setting something via TWO keys, but not at the same time. If both the primary
    key and alt key have values, an error gets raised. If the alt key has values, then we update
    the dictionary to ensure the primary key contains the values. If neither are set, nothing happens.
    """

    if dictionary is None:
        return

    primary_options = dictionary.get(primary)
    alt_options = dictionary.get(alt)

    if primary_options and alt_options:
        where = f" in `{parent_config}`" if parent_config is not None else ""
        raise DbtExclusivePropertyUseError(
            f"Only `{alt}` or `{primary}` can be specified{where}, not both"
        )

    if alt in dictionary:
        alt_value = dictionary.pop(alt)
        dictionary[primary] = alt_value


def normalize_warn_error_options(warn_error_options: Dict[str, Any]) -> None:
    has_include = "include" in warn_error_options
    has_exclude = "exclude" in warn_error_options

    if has_include or has_exclude:
        deprecations.buffer(
            "weo-include-exclude-deprecation",
            found_include=has_include,
            found_exclude=has_exclude,
        )

    exclusive_primary_alt_value_setting(
        warn_error_options, "error", "include", "warn_error_options"
    )
    exclusive_primary_alt_value_setting(
        warn_error_options, "warn", "exclude", "warn_error_options"
    )
    for key in ("error", "warn", "silence"):
        if key in warn_error_options and warn_error_options[key] is None:
            warn_error_options[key] = []


def partition_warn_error_options(
    warn_error_options: Dict[str, Any], valid_error_names: Set[str]
) -> Tuple[Set[str], Dict[str, Any]]:

    v2_only_vocab = V2_WARN_ERROR_OPTION_NAMES - valid_error_names

    def is_v2_only(name: Any) -> bool:
        return isinstance(name, str) and name in v2_only_vocab

    v2_only_names: Set[str] = set()
    core_v2_warn_error_options = dict(warn_error_options)
    for key in ("error", "warn", "silence"):
        names = warn_error_options.get(key)
        if isinstance(names, list):
            v2_only_names.update(filter(is_v2_only, names))
            core_v2_warn_error_options[key] = [n for n in names if not is_v2_only(n)]
    return v2_only_names, core_v2_warn_error_options


def build_warn_error_options_v2(
    warn_error_options: Dict[str, Any], valid_error_names: Set[str]
) -> WarnErrorOptionsV2:
    v2_only_names, core_v2_warn_error_options = partition_warn_error_options(
        warn_error_options, valid_error_names
    )
    for name in sorted(v2_only_names):
        fire_event(
            Note(msg=f"{name} is not being used because it's specific to the dbt v2 engine.")
        )

    return WarnErrorOptionsV2(
        error=core_v2_warn_error_options.get("error", []),
        warn=core_v2_warn_error_options.get("warn", []),
        silence=core_v2_warn_error_options.get("silence", []),
        valid_error_names=valid_error_names,
    )
