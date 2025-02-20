import os
from typing import Any, Dict, Optional

import yaml

import dbt_common.exceptions
import dbt_common.exceptions.base

# the C version is faster, but it doesn't always exist
try:
    from yaml import CDumper as Dumper
    from yaml import CLoader as Loader
    from yaml import CSafeLoader as SafeLoader
except ImportError:
    from yaml import Dumper, Loader, SafeLoader  # type: ignore  # noqa: F401


YAML_ERROR_MESSAGE = """
Syntax error near line {line_number}
------------------------------
{nice_error}

Raw Error:
------------------------------
{raw_error}
""".strip()


def line_no(i, line, width=3):
    line_number = str(i).ljust(width)
    return "{}| {}".format(line_number, line)


def prefix_with_line_numbers(string, no_start, no_end):
    line_list = string.split("\n")

    numbers = range(no_start, no_end)
    relevant_lines = line_list[no_start:no_end]

    return "\n".join([line_no(i + 1, line) for (i, line) in zip(numbers, relevant_lines)])


def contextualized_yaml_error(raw_contents, error):
    mark = error.problem_mark

    min_line = max(mark.line - 3, 0)
    max_line = mark.line + 4

    nice_error = prefix_with_line_numbers(raw_contents, min_line, max_line)

    return YAML_ERROR_MESSAGE.format(
        line_number=mark.line + 1, nice_error=nice_error, raw_error=error
    )


def safe_load(contents) -> Optional[Dict[str, Any]]:
    return yaml.load(contents, Loader=SafeLoader)


def load_yaml_text(contents, path=None):
    try:
        return safe_load(contents)
    except (yaml.scanner.ScannerError, yaml.YAMLError) as e:
        if hasattr(e, "problem_mark"):
            error = contextualized_yaml_error(contents, e)
        else:
            error = str(e)

        raise dbt_common.exceptions.base.DbtValidationError(error)


def get_yaml_path(path: str) -> str:
    if os.path.exists("{}.yml".format(path)):
        return path + ".yml"
    elif os.path.exists("{}.yaml".format(path)):
        return path + ".yaml"
    else:
        return ""
