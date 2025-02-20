import click
import pytest

from dbt.cli import main as cli_main
from dbt.cli.types import Command
from dbt.task.retry import CMD_DICT
from dbt.utils import args_to_dict


def is_problematic_option(option: click.Option) -> bool:
    return (
        option.is_flag and not option.default and not option.secondary_opts and option.expose_value
    )


def get_problemetic_options_for_command(command: click.Command) -> list[str]:
    """
    Get boolean flags of a ClickCommand that are False by default, do not
    have a secondary option (--no-*), and expose their value.
    Why do we care? If not dealt with, these arguments are stored in run_results.json
    and converted to non-existent --no-* options when running dbt retry.
    """
    return [
        option.name
        for option in command.params
        if isinstance(option, click.Option) and is_problematic_option(option)
    ]


def get_commands_supported_by_retry() -> list[click.Command]:
    command_names = [convert_enum_to_command_function_name(value) for value in CMD_DICT.values()]
    return [getattr(cli_main, name) for name in command_names]


def convert_enum_to_command_function_name(enum: Command) -> str:
    return "_".join(enum.to_list()).replace("-", "_")


class FlagsDummy:
    def __init__(self, args: dict[str, bool]):
        self.__dict__ = args


@pytest.mark.parametrize("command", get_commands_supported_by_retry())
def test_flags_problematic_for_retry_are_dealt_with(command: click.Command):
    """
    For each command supported by retry, get a list of flags that should
    not be converted to --no-* when False, and assert if args_to_dict correctly
    skips it.
    """
    flag_names = get_problemetic_options_for_command(command)
    flags = FlagsDummy({name: False for name in flag_names})
    args_dict = args_to_dict(flags)
    for flag_name in flag_names:
        assert flag_name not in args_dict, f"add {flag_name} to default_false_keys in args_to_dict"
