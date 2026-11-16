import re
from typing import Optional

_QUERY_START = re.compile(r"(?:select|with)\b", re.IGNORECASE)
_DOLLAR_QUOTE = re.compile(r"\$(?:[A-Za-z_][A-Za-z_0-9]*)?\$")


def _quoted_end(contents: str, start: int) -> Optional[int]:
    quote = contents[start]
    index = start + 1
    while index < len(contents):
        end = contents.find(quote, index)
        if end == -1:
            return None
        if end + 1 < len(contents) and contents[end + 1] == quote:
            index = end + 2
        else:
            return end + 1
    return None


def normalize_sql_whitespace(contents: str) -> str:
    """Canonicalize horizontal spacing in a conservative subset of plain SQL.

    Keep line breaks and quoted contents exactly. Jinja, comments, prefixed or
    ambiguous quoting, and syntax outside this subset retain their original hash
    input. In particular, this is not a general SQL semantic-equivalence check.
    """
    if not _QUERY_START.match(contents):
        return contents
    if any(marker in contents for marker in ("{{", "{%", "{#", "'''", '"""', "\\")):
        return contents

    normalized = []
    index = 0
    while index < len(contents):
        char = contents[index]
        if char in " \t":
            while index < len(contents) and contents[index] in " \t":
                index += 1
            normalized.append(" ")
            continue
        if char in "\r\n":
            normalized.append(char)
            index += 1
            continue
        if char in "'\"`":
            # Prefixes such as E'', N'', q'[]', r'' and U&'' have dialect-specific
            # escaping rules. Do not guess how much source belongs to a literal.
            if index and (contents[index - 1].isalnum() or contents[index - 1] in "_$@&"):
                return contents
            end = _quoted_end(contents, index)
            if end is None:
                return contents
            normalized.append(contents[index:end])
            index = end
            continue
        if char == "$":
            if index and (contents[index - 1].isalnum() or contents[index - 1] in "_$"):
                return contents
            match = _DOLLAR_QUOTE.match(contents, index)
            if match is None:
                return contents
            delimiter = match.group()
            end = contents.find(delimiter, match.end())
            if end == -1:
                return contents
            end += len(delimiter)
            normalized.append(contents[index:end])
            index = end
            continue
        # Comments can contain executable directives or quotes that would shift
        # literal boundaries. Snowflake also supports // line comments.
        # Unknown punctuation may introduce another dialect's quoting syntax.
        if contents.startswith(("--", "//", "/*"), index):
            return contents
        if char == ";" and index != len(contents) - 1:
            return contents
        if not (char.isascii() and (char.isalnum() or char in "_.,():;+-*/%=<>!|&^~?")):
            return contents
        normalized.append(char)
        index += 1

    return "".join(normalized)
