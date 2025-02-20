import os
from functools import cached_property
from typing import Any, Dict, List, Optional, Union, overload

import yaml

import dbt_common.exceptions

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


class LoaderWithInclude(Loader):
    """Loader with a name being set."""

    def __init__(self, stream: Any) -> None:
        """Initialize a safe line loader."""
        self.stream = stream

        # Set name in same way as the Python loader does in yaml.reader.__init__
        if isinstance(stream, str):
            self.name = "<unicode string>"
        elif isinstance(stream, bytes):
            self.name = "<byte string>"
        else:
            self.name = getattr(stream, "name", "<file>")

        super().__init__(stream)

    @cached_property
    def get_name(self) -> str:
        """Get the name of the loader."""
        return self.name


def safe_load(contents) -> Optional[Dict[str, Any]]:
    loader = LoaderWithInclude
    loader.add_constructor("!include", _include_yaml)
    return yaml.load(contents, Loader=loader)


def load_yaml_text(contents, path=None):
    try:
        return safe_load(contents)
    except (yaml.scanner.ScannerError, yaml.YAMLError) as e:
        if hasattr(e, "problem_mark"):
            error = contextualized_yaml_error(contents, e)
        else:
            error = str(e)

        raise dbt_common.exceptions.base.DbtValidationError(error)


JSON_TYPE = Union[List, Dict, str]


def parse_yaml(content: Any, secrets=None) -> JSON_TYPE:
    """Parse YAML with the fastest available loader."""
    return _parse_yaml(LoaderWithInclude, content, secrets)


def _parse_yaml(
    loader: LoaderWithInclude,
    content: Any,
    secrets: Optional[str] = None,
) -> JSON_TYPE:
    """Load a YAML file."""
    return yaml.load(content, LoaderWithInclude)  # type: ignore[arg-type]


def load_yaml(fname: Any) -> Optional[JSON_TYPE]:
    """Load a YAML file."""
    try:
        with open(fname, encoding="utf-8") as conf_file:
            return parse_yaml(conf_file, None)
    except UnicodeDecodeError as exc:
        raise dbt_common.exceptions.base.DbtValidationError(str(exc))


@overload
def _add_reference(
    obj: list,
    loader: LoaderWithInclude,
    node: yaml.nodes.Node,
) -> list: ...


@overload
def _add_reference(
    obj: str,
    loader: LoaderWithInclude,
    node: yaml.nodes.Node,
) -> str: ...


@overload
def _add_reference(obj: dict, loader: LoaderWithInclude, node: yaml.nodes.Node) -> dict: ...


def _add_reference(obj, loader: LoaderWithInclude, node: yaml.nodes.Node):  # type: ignore[no-untyped-def]
    """Add file reference information to an object."""
    if isinstance(obj, list):
        obj = obj
    if isinstance(obj, str):
        obj = obj
    try:  # noqa: SIM105 suppress is much slower
        setattr(obj, "__config_file__", loader.get_name)
        setattr(obj, "__line__", node.start_mark.line + 1)
    except AttributeError:
        pass
    return obj


def _include_yaml(loader: LoaderWithInclude, node: yaml.nodes.Node) -> JSON_TYPE:
    """Load another YAML file and embed it using the !include tag.

    Example:
        +schema: !include schema_config.yml

    """
    fname = os.path.join(os.path.dirname(loader.get_name), node.value)
    try:
        loaded_yaml = load_yaml(fname)
        if loaded_yaml is None:
            loaded_yaml = {}
        return _add_reference(loaded_yaml, loader, node)
    except FileNotFoundError as exc:
        raise dbt_common.exceptions.base.DbtValidationError(str(exc))
