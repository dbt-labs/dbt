from typing import Optional

import pytz
from click import Choice, Context, Parameter, ParamType

from dbt.config.utils import normalize_warn_error_options, parse_cli_yaml_string
from dbt.event_time.sample_window import SampleWindow
from dbt.events import ALL_EVENT_NAMES
from dbt.exceptions import OptionNotYamlDictError, ValidationError
from dbt_common.exceptions import DbtValidationError
from dbt_common.helper_types import WarnErrorOptions


class YAML(ParamType):
    """The Click YAML type. Converts YAML strings into objects."""

    name = "YAML"

    def convert(self, value, param, ctx):
        # assume non-string values are a problem
        if not isinstance(value, str):
            self.fail(f"Cannot load YAML from type {type(value)}", param, ctx)
        else:
            return parse_cli_yaml_string(value, param.name)
        try:
            param_option_name = param.opts[0] if param.opts else param.name
            return parse_cli_yaml_string(value, param_option_name.strip("-"))
        except (ValidationError, DbtValidationError, OptionNotYamlDictError):
            self.fail(f"String '{value}' is not valid YAML", param, ctx)
        


class Package(ParamType):
    """The Click STRING type. Converts string into dict with package name and version.
    Example package:
        package-name@1.0.0
        package-name
    """

    name = "NewPackage"

    def convert(self, value, param, ctx):
        # assume non-string values are a problem
        if not isinstance(value, str):
            self.fail(f"Cannot load Package from type {type(value)}", param, ctx)
        try:
            package_name, package_version = value.split("@")
            return {"name": package_name, "version": package_version}
        except ValueError:
            return {"name": value, "version": None}
        
    def convert(self, value, param, ctx):

        if  isinstance(value, str):
            try:
                package_name, package_version = value.split("@")
                return {"name": package_name, "version": package_version}
            except ValueError:
                return {"name": value, "version": None}


class WarnErrorOptionsType(YAML):
    """The Click WarnErrorOptions type. Converts YAML strings into objects."""

    name = "WarnErrorOptionsType"

    def convert(self, value, param, ctx):
        # this function is being used by param in click
        include_exclude = super().convert(value, param, ctx)
        normalize_warn_error_options(include_exclude)

        return WarnErrorOptions(
            include=include_exclude.get("include", []),
            exclude=include_exclude.get("exclude", []),
            silence=include_exclude.get("silence", []),
            valid_error_names=ALL_EVENT_NAMES,
        )
    



class Truthy(ParamType):
    """The Click Truthy type.  Converts strings into a "truthy" type"""

    name = "TRUTHY"

    def convert(self, value, param, ctx):
        # assume non-string / non-None values are a problem
        if not isinstance(value, (str, None)):
            self.fail(f"Cannot load TRUTHY from type {type(value)}", param, ctx)

        if value is None or value.lower() in ("0", "false", "f"):
            return None
        else:
            return value
        

 
 


class ChoiceTuple(Choice):
    name = "CHOICE_TUPLE"

    def convert(self, value, param, ctx):
        if not isinstance(value, str):
            for value_item in value:
                super().convert(value_item, param, ctx)
        else:
            super().convert(value, param, ctx)

        return value


class SampleType(ParamType):
    name = "SAMPLE"

    def convert(
        self, value, param: Optional[Parameter], ctx: Optional[Context]
    ) -> Optional[SampleWindow]:
        if value is None:
            return None

        if isinstance(value, str):
            try:
                # Try and identify if it's a "dict" or a "str"
                if value.lstrip()[0] == "{":
                    param_option_name: str = param.opts[0] if param.opts else param.name  # type: ignore
                    parsed_dict = parse_cli_yaml_string(value, param_option_name.strip("-"))
                    sample_window = SampleWindow.from_dict(parsed_dict)
                    sample_window.start = sample_window.start.replace(tzinfo=pytz.UTC)
                    sample_window.end = sample_window.end.replace(tzinfo=pytz.UTC)
                    return sample_window
                else:
                    return SampleWindow.from_relative_string(value)
            except Exception as e:
                self.fail(e.__str__(), param, ctx)
        else:
            self.fail(f"Cannot load SAMPLE_WINDOW from type {type(value)}", param, ctx)
