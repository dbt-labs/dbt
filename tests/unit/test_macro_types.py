from dbt.parser.schemas import is_valid_type


def test_valid_names() -> None:
    valid_names = [
        "str",
        "string",
        "bool",
        "boolean",
        "int",
        "integer",
        "float",
        "any",
        "list[int]",
        "dict[str, int]",
        "optional[any]",
        "relation",
        "column",
        "list[dict[str, any]]",
        "dict[str, list[int]]",
        "optional[boolean]",
        "list[boolean]",
    ]

    for name in valid_names:
        assert is_valid_type(name)


def test_invalid_names() -> None:
    invalid_names = [
        "strang",  # Not a valid name
        "int int",  # Repeat not allowed
        "intint",  # No repeat, even with no space
        "list[dict[any]]",  # dict needs two args
        "dict[str, list[int]]]",  # Can't have extra closing brace
        "dict[str, list[[int]]",  # Can't have extra opening brace
        "dict[str,]",  # Can't have blank nexted type
        "dict[str,,str]",  # Can't have extra comma
    ]

    for name in invalid_names:
        assert not is_valid_type(name)
