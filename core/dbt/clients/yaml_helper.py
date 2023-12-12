import re
import dbt.exceptions
from typing import Any, Dict, Optional, Tuple
import yaml

# the C version is faster, but it doesn't always exist
try:
    from yaml import CLoader as Loader, CSafeLoader as SafeLoader, CDumper as Dumper
except ImportError:
    from yaml import Loader, SafeLoader, Dumper  # type: ignore  # noqa: F401

FRONTMATTER_CHECK = ["---\n", "---\r\n"]
FRONTMATTER_DELIMITER = re.compile(r"^---", re.MULTILINE)
NON_WHITESPACE = re.compile(r"\S")

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

        raise dbt.exceptions.DbtValidationError(error)


def split_yaml_frontmatter(content: str) -> Tuple[Optional[str], str]:
    """Splits `content` into raw YAML frontmatter (as a raw string) and everything else proceeding.

    Frontmatter is defined as a block of YAML appearing between two `---` tokens in an otherwise non-YAML document.
    The frontmatter must be placed at the beginning of the file: anything other than whitespace preceding the first `---`
    will cause the frontmatter block to be ignored, with a warning.
    """
    parts = FRONTMATTER_DELIMITER.split(content, 2)
    if len(parts) != 3:
        # Zero or one `---` token, so return the original string
        return None, content
    elif NON_WHITESPACE.search(parts[0]) is not None:
        # No frontmatter section or non-whitespace preceding the first `---`, so skip frontmatter
        return None, content

    frontmatter_content, after_footer = parts[1:]
    return frontmatter_content, after_footer


def parse_yaml_frontmatter(frontmatter_content: str, original_content: str):
    try:
        parsed_yaml = safe_load(frontmatter_content)
    except (yaml.scanner.ScannerError, yaml.YAMLError) as e:
        if hasattr(e, "problem_mark"):
            error = contextualized_yaml_error(original_content, e)
        else:
            error = str(e)
        error = f"Error parsing YAML frontmatter:  {error}"
        raise dbt.exceptions.DbtValidationError(error)

    return parsed_yaml


def has_yaml_frontmatter(content: str) -> bool:
    """Check first line for yaml frontmatter"""

    return content.startswith(FRONTMATTER_CHECK[0]) or content.startswith(FRONTMATTER_CHECK[1])
