import builtins
import json
import os
import re
import io
import agate
from typing import Any, Dict, List, Mapping, Optional, Union

from dbt.constants import SECRET_ENV_PREFIX
from dbt.dataclass_schema import ValidationError
from dbt.node_types import NodeType, AccessType
from dbt.ui import line_wrap_message

import dbt.dataclass_schema


def env_secrets() -> List[str]:
    return [v for k, v in os.environ.items() if k.startswith(SECRET_ENV_PREFIX) and v.strip()]


def scrub_secrets(msg: str, secrets: List[str]) -> str:
    scrubbed = str(msg)

    for secret in secrets:
        scrubbed = scrubbed.replace(secret, "*****")

    return scrubbed


class MacroReturn(builtins.BaseException):
    """
    Hack of all hacks
    This is not actually an exception.
    It's how we return a value from a macro.
    """

    def __init__(self, value) -> None:
        self.value = value


class Exception(builtins.Exception):
    CODE = -32000
    MESSAGE = "Server Error"

    def data(self):
        # if overriding, make sure the result is json-serializable.
        return {
            "type": self.__class__.__name__,
            "message": str(self),
        }


class DbtInternalError(Exception):
    def __init__(self, msg: str) -> None:
        self.stack: List = []
        self.msg = scrub_secrets(msg, env_secrets())

    @property
    def type(self):
        return "Internal"

    def process_stack(self):
        lines = []
        stack = self.stack
        first = True

        if len(stack) > 1:
            lines.append("")

            for item in stack:
                msg = "called by"

                if first:
                    msg = "in"
                    first = False

                lines.append(f"> {msg}")

        return lines

    def __str__(self):
        if hasattr(self.msg, "split"):
            split_msg = self.msg.split("\n")
        else:
            split_msg = str(self.msg).split("\n")

        lines = ["{}".format(self.type + " Error")] + split_msg

        lines += self.process_stack()

        return lines[0] + "\n" + "\n".join(["  " + line for line in lines[1:]])


class DbtRuntimeError(RuntimeError, Exception):
    CODE = 10001
    MESSAGE = "Runtime error"

    def __init__(self, msg: str, node=None) -> None:
        self.stack: List = []
        self.node = node
        self.msg = scrub_secrets(msg, env_secrets())

    def add_node(self, node=None):
        if node is not None and node is not self.node:
            if self.node is not None:
                self.stack.append(self.node)
            self.node = node

    @property
    def type(self):
        return "Runtime"

    def node_to_string(self, node):
        if node is None:
            return "<Unknown>"
        if not hasattr(node, "name"):
            # we probably failed to parse a block, so we can't know the name
            return f"{node.resource_type} ({node.original_file_path})"

        if hasattr(node, "contents"):
            # handle FileBlocks. They aren't really nodes but we want to render
            # out the path we know at least. This indicates an error during
            # block parsing.
            return f"{node.path.original_file_path}"
        return f"{node.resource_type} {node.name} ({node.original_file_path})"

    def process_stack(self):
        lines = []
        stack = self.stack + [self.node]
        first = True

        if len(stack) > 1:
            lines.append("")

            for item in stack:
                msg = "called by"

                if first:
                    msg = "in"
                    first = False

                lines.append(f"> {msg} {self.node_to_string(item)}")

        return lines

    def validator_error_message(self, exc: builtins.Exception):
        """Given a dbt.dataclass_schema.ValidationError (which is basically a
        jsonschema.ValidationError), return the relevant parts as a string
        """
        if not isinstance(exc, dbt.dataclass_schema.ValidationError):
            return str(exc)
        path = "[%s]" % "][".join(map(repr, exc.relative_path))
        return f"at path {path}: {exc.message}"

    def __str__(self, prefix: str = "! "):
        node_string = ""

        if self.node is not None:
            node_string = f" in {self.node_to_string(self.node)}"

        if hasattr(self.msg, "split"):
            split_msg = self.msg.split("\n")
        else:
            split_msg = str(self.msg).split("\n")

        lines = ["{}{}".format(self.type + " Error", node_string)] + split_msg

        lines += self.process_stack()

        return lines[0] + "\n" + "\n".join(["  " + line for line in lines[1:]])

    def data(self):
        result = Exception.data(self)
        if self.node is None:
            return result

        result.update(
            {
                "raw_code": self.node.raw_code,
                # the node isn't always compiled, but if it is, include that!
                "compiled_code": getattr(self.node, "compiled_code", None),
            }
        )
        return result


class DbtDatabaseError(DbtRuntimeError):
    CODE = 10003
    MESSAGE = "Database Error"

    def process_stack(self):
        lines = []

        if hasattr(self.node, "build_path") and self.node.build_path:
            lines.append(f"compiled Code at {self.node.build_path}")

        return lines + DbtRuntimeError.process_stack(self)

    @property
    def type(self):
        return "Database"


class CompilationError(DbtRuntimeError):
    CODE = 10004
    MESSAGE = "Compilation Error"

    @property
    def type(self):
        return "Compilation"

    def _fix_dupe_msg(self, path_1: str, path_2: str, name: str, type_name: str) -> str:
        if path_1 == path_2:
            return (
                f"remove one of the {type_name} entries for {name} in this file:\n - {path_1!s}\n"
            )
        else:
            return (
                f"remove the {type_name} entry for {name} in one of these files:\n"
                f" - {path_1!s}\n{path_2!s}"
            )


class ContractBreakingChangeError(DbtRuntimeError):
    CODE = 10016
    MESSAGE = "Breaking Change to Contract"

    def __init__(
        self,
        breaking_changes: List[str],
        node=None,
    ) -> None:
        self.breaking_changes = breaking_changes
        super().__init__(self.message(), node)

    @property
    def type(self):
        return "Breaking change to contract"

    def message(self):
        reasons = "\n  - ".join(self.breaking_changes)

        return (
            "While comparing to previous project state, dbt detected a breaking change to an enforced contract."
            f"\n  - {reasons}\n"
            "Consider making an additive (non-breaking) change instead, if possible.\n"
            "Otherwise, create a new model version: https://docs.getdbt.com/docs/collaborate/govern/model-versions"
        )


class RecursionError(DbtRuntimeError):
    pass


class DbtValidationError(DbtRuntimeError):
    CODE = 10005
    MESSAGE = "Validation Error"


class ParsingError(DbtRuntimeError):
    CODE = 10015
    MESSAGE = "Parsing Error"

    @property
    def type(self):
        return "Parsing"


class dbtPluginError(DbtRuntimeError):
    CODE = 10020
    MESSAGE = "Plugin Error"


# TODO: this isn't raised in the core codebase.  Is it raised elsewhere?
class JSONValidationError(DbtValidationError):
    def __init__(self, typename, errors) -> None:
        self.typename = typename
        self.errors = errors
        self.errors_message = ", ".join(errors)
        msg = f'Invalid arguments passed to "{self.typename}" instance: {self.errors_message}'
        super().__init__(msg)

    def __reduce__(self):
        # see https://stackoverflow.com/a/36342588 for why this is necessary
        return (JSONValidationError, (self.typename, self.errors))


class IncompatibleSchemaError(DbtRuntimeError):
    def __init__(self, expected: str, found: Optional[str] = None) -> None:
        self.expected = expected
        self.found = found
        self.filename = "input file"

        super().__init__(msg=self.get_message())

    def add_filename(self, filename: str):
        self.filename = filename
        self.msg = self.get_message()

    def get_message(self) -> str:
        found_str = "nothing"
        if self.found is not None:
            found_str = f'"{self.found}"'

        msg = (
            f'Expected a schema version of "{self.expected}" in '
            f"{self.filename}, but found {found_str}. Are you running with a "
            f"different version of dbt?"
        )
        return msg

    CODE = 10014
    MESSAGE = "Incompatible Schema"


class JinjaRenderingError(CompilationError):
    pass


class UndefinedMacroError(CompilationError):
    def __str__(self, prefix: str = "! ") -> str:
        msg = super().__str__(prefix)
        return (
            f"{msg}. This can happen when calling a macro that does "
            "not exist. Check for typos and/or install package dependencies "
            'with "dbt deps".'
        )


class AliasError(DbtValidationError):
    pass


class DependencyError(Exception):
    CODE = 10006
    MESSAGE = "Dependency Error"


class DbtConfigError(DbtRuntimeError):
    CODE = 10007
    MESSAGE = "DBT Configuration Error"

    def __init__(self, msg: str, project=None, result_type="invalid_project", path=None) -> None:
        self.project = project
        super().__init__(msg)
        self.result_type = result_type
        self.path = path

    def __str__(self, prefix="! ") -> str:
        msg = super().__str__(prefix)
        if self.path is None:
            return msg
        else:
            return f"{msg}\n\nError encountered in {self.path}"


class FailFastError(DbtRuntimeError):
    CODE = 10013
    MESSAGE = "FailFast Error"

    def __init__(self, msg: str, result=None, node=None) -> None:
        super().__init__(msg=msg, node=node)
        self.result = result

    @property
    def type(self):
        return "FailFast"


class DbtProjectError(DbtConfigError):
    pass


class DbtSelectorsError(DbtConfigError):
    pass


class DbtProfileError(DbtConfigError):
    pass


class SemverError(Exception):
    def __init__(self, msg: Optional[str] = None) -> None:
        self.msg = msg
        if msg is not None:
            super().__init__(msg)
        else:
            super().__init__()


class VersionsNotCompatibleError(SemverError):
    pass


class NotImplementedError(Exception):
    def __init__(self, msg: str) -> None:
        self.msg = msg
        self.formatted_msg = f"ERROR: {self.msg}"
        super().__init__(self.formatted_msg)


class FailedToConnectError(DbtDatabaseError):
    pass


class CommandError(DbtRuntimeError):
    def __init__(self, cwd: str, cmd: List[str], msg: str = "Error running command") -> None:
        cmd_scrubbed = list(scrub_secrets(cmd_txt, env_secrets()) for cmd_txt in cmd)
        super().__init__(msg)
        self.cwd = cwd
        self.cmd = cmd_scrubbed
        self.args = (cwd, cmd_scrubbed, msg)

    def __str__(self):
        if len(self.cmd) == 0:
            return f"{self.msg}: No arguments given"
        return f'{self.msg}: "{self.cmd[0]}"'


class ExecutableError(CommandError):
    def __init__(self, cwd: str, cmd: List[str], msg: str) -> None:
        super().__init__(cwd, cmd, msg)


class WorkingDirectoryError(CommandError):
    def __init__(self, cwd: str, cmd: List[str], msg: str) -> None:
        super().__init__(cwd, cmd, msg)

    def __str__(self):
        return f'{self.msg}: "{self.cwd}"'


class CommandResultError(CommandError):
    def __init__(
        self,
        cwd: str,
        cmd: List[str],
        returncode: Union[int, Any],
        stdout: bytes,
        stderr: bytes,
        msg: str = "Got a non-zero returncode",
    ) -> None:
        super().__init__(cwd, cmd, msg)
        self.returncode = returncode
        self.stdout = scrub_secrets(stdout.decode("utf-8"), env_secrets())
        self.stderr = scrub_secrets(stderr.decode("utf-8"), env_secrets())
        self.args = (cwd, self.cmd, returncode, self.stdout, self.stderr, msg)

    def __str__(self):
        return f"{self.msg} running: {self.cmd}"


class InvalidConnectionError(DbtRuntimeError):
    def __init__(self, thread_id, known: List) -> None:
        self.thread_id = thread_id
        self.known = known
        super().__init__(
            msg=f"connection never acquired for thread {self.thread_id}, have {self.known}"
        )


class InvalidSelectorError(DbtRuntimeError):
    def __init__(self, name: str) -> None:
        self.name = name
        super().__init__(name)


class DuplicateYamlKeyError(CompilationError):
    pass


class ConnectionError(Exception):
    """
    There was a problem with the connection that returned a bad response,
    timed out, or resulted in a file that is corrupt.
    """

    pass


# event level exception
class EventCompilationError(CompilationError):
    def __init__(self, msg: str, node) -> None:
        self.msg = scrub_secrets(msg, env_secrets())
        self.node = node
        super().__init__(msg=self.msg)


# compilation level exceptions
class GraphDependencyNotFoundError(CompilationError):
    def __init__(self, node, dependency: str) -> None:
        self.node = node
        self.dependency = dependency
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f"'{self.node.unique_id}' depends on '{self.dependency}' which is not in the graph!"
        return msg


# client level exceptions


class NoSupportedLanguagesFoundError(CompilationError):
    def __init__(self, node) -> None:
        self.node = node
        self.msg = f"No supported_languages found in materialization macro {self.node.name}"
        super().__init__(msg=self.msg)


class MaterializtionMacroNotUsedError(CompilationError):
    def __init__(self, node) -> None:
        self.node = node
        self.msg = "Only materialization macros can be used with this function"
        super().__init__(msg=self.msg)


class UndefinedCompilationError(CompilationError):
    def __init__(self, name: str, node) -> None:
        self.name = name
        self.node = node
        self.msg = f"{self.name} is undefined"
        super().__init__(msg=self.msg)


class CaughtMacroErrorWithNodeError(CompilationError):
    def __init__(self, exc, node) -> None:
        self.exc = exc
        self.node = node
        super().__init__(msg=str(exc))


class CaughtMacroError(CompilationError):
    def __init__(self, exc) -> None:
        self.exc = exc
        super().__init__(msg=str(exc))


class MacroNameNotStringError(CompilationError):
    def __init__(self, kwarg_value) -> None:
        self.kwarg_value = kwarg_value
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            f"The macro_name parameter ({self.kwarg_value}) "
            "to adapter.dispatch was not a string"
        )
        return msg


class MissingControlFlowStartTagError(CompilationError):
    def __init__(self, tag, expected_tag: str, tag_parser) -> None:
        self.tag = tag
        self.expected_tag = expected_tag
        self.tag_parser = tag_parser
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        linepos = self.tag_parser.linepos(self.tag.start)
        msg = (
            f"Got an unexpected control flow end tag, got {self.tag.block_type_name} but "
            f"expected {self.expected_tag} next (@ {linepos})"
        )
        return msg


class UnexpectedControlFlowEndTagError(CompilationError):
    def __init__(self, tag, expected_tag: str, tag_parser) -> None:
        self.tag = tag
        self.expected_tag = expected_tag
        self.tag_parser = tag_parser
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        linepos = self.tag_parser.linepos(self.tag.start)
        msg = (
            f"Got an unexpected control flow end tag, got {self.tag.block_type_name} but "
            f"never saw a preceeding {self.expected_tag} (@ {linepos})"
        )
        return msg


class UnexpectedMacroEOFError(CompilationError):
    def __init__(self, expected_name: str, actual_name: str) -> None:
        self.expected_name = expected_name
        self.actual_name = actual_name
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f'unexpected EOF, expected {self.expected_name}, got "{self.actual_name}"'
        return msg


class MacroNamespaceNotStringError(CompilationError):
    def __init__(self, kwarg_type: Any) -> None:
        self.kwarg_type = kwarg_type
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            "The macro_namespace parameter to adapter.dispatch "
            f"is a {self.kwarg_type}, not a string"
        )
        return msg


class NestedTagsError(CompilationError):
    def __init__(self, outer, inner) -> None:
        self.outer = outer
        self.inner = inner
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            f"Got nested tags: {self.outer.block_type_name} (started at {self.outer.start}) did "
            f"not have a matching {{{{% end{self.outer.block_type_name} %}}}} before a "
            f"subsequent {self.inner.block_type_name} was found (started at {self.inner.start})"
        )
        return msg


class BlockDefinitionNotAtTopError(CompilationError):
    def __init__(self, tag_parser, tag_start) -> None:
        self.tag_parser = tag_parser
        self.tag_start = tag_start
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        position = self.tag_parser.linepos(self.tag_start)
        msg = (
            f"Got a block definition inside control flow at {position}. "
            "All dbt block definitions must be at the top level"
        )
        return msg


class MissingCloseTagError(CompilationError):
    def __init__(self, block_type_name: str, linecount: int) -> None:
        self.block_type_name = block_type_name
        self.linecount = linecount
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f"Reached EOF without finding a close tag for {self.block_type_name} (searched from line {self.linecount})"
        return msg


class UnknownGitCloningProblemError(DbtRuntimeError):
    def __init__(self, repo: str) -> None:
        self.repo = scrub_secrets(repo, env_secrets())
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f"""\
        Something went wrong while cloning {self.repo}
        Check the debug logs for more information
        """
        return msg


class NoAdaptersAvailableError(DbtRuntimeError):
    def __init__(self) -> None:
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = "No adapters available. Learn how to install an adapter by going to https://docs.getdbt.com/docs/connect-adapters#install-using-the-cli"
        return msg


class BadSpecError(DbtInternalError):
    def __init__(self, repo, revision, error) -> None:
        self.repo = repo
        self.revision = revision
        self.stderr = scrub_secrets(error.stderr.strip(), env_secrets())
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f"Error checking out spec='{self.revision}' for repo {self.repo}\n{self.stderr}"
        return msg


class GitCloningError(DbtInternalError):
    def __init__(self, repo: str, revision: str, error: CommandResultError) -> None:
        self.repo = repo
        self.revision = revision
        self.error = error
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        stderr = self.error.stderr.strip()
        if "usage: git" in stderr:
            stderr = stderr.split("\nusage: git")[0]
        if re.match("fatal: destination path '(.+)' already exists", stderr):
            self.error.cmd = list(scrub_secrets(str(self.error.cmd), env_secrets()))
            raise self.error

        msg = f"Error checking out spec='{self.revision}' for repo {self.repo}\n{stderr}"
        return scrub_secrets(msg, env_secrets())


class GitCheckoutError(BadSpecError):
    pass


class MaterializationArgError(CompilationError):
    def __init__(self, name: str, argument: str) -> None:
        self.name = name
        self.argument = argument
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f"materialization '{self.name}' received unknown argument '{self.argument}'."
        return msg


class OperationError(CompilationError):
    def __init__(self, operation_name) -> None:
        self.operation_name = operation_name
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            f"dbt encountered an error when attempting to create a {self.operation_name}. "
            "If this error persists, please create an issue at: \n\n"
            "https://github.com/dbt-labs/dbt-core"
        )

        return msg


# context level exceptions
class ZipStrictWrongTypeError(CompilationError):
    def __init__(self, exc) -> None:
        self.exc = exc
        msg = str(self.exc)
        super().__init__(msg=msg)


class SetStrictWrongTypeError(CompilationError):
    def __init__(self, exc) -> None:
        self.exc = exc
        msg = str(self.exc)
        super().__init__(msg=msg)


class LoadAgateTableValueError(CompilationError):
    def __init__(self, exc: ValueError, node) -> None:
        self.exc = exc
        self.node = node
        msg = str(self.exc)
        super().__init__(msg=msg)


class LoadAgateTableNotSeedError(CompilationError):
    def __init__(self, resource_type, node) -> None:
        self.resource_type = resource_type
        self.node = node
        msg = f"can only load_agate_table for seeds (got a {self.resource_type})"
        super().__init__(msg=msg)


class MacrosSourcesUnWriteableError(CompilationError):
    def __init__(self, node) -> None:
        self.node = node
        msg = 'cannot "write" macros or sources'
        super().__init__(msg=msg)


class PackageNotInDepsError(CompilationError):
    def __init__(self, package_name: str, node) -> None:
        self.package_name = package_name
        self.node = node
        msg = f"Node package named {self.package_name} not found!"
        super().__init__(msg=msg)


class OperationsCannotRefEphemeralNodesError(CompilationError):
    def __init__(self, target_name: str, node) -> None:
        self.target_name = target_name
        self.node = node
        msg = f"Operations can not ref() ephemeral nodes, but {target_name} is ephemeral"
        super().__init__(msg=msg)


class PersistDocsValueTypeError(CompilationError):
    def __init__(self, persist_docs: Any) -> None:
        self.persist_docs = persist_docs
        msg = (
            "Invalid value provided for 'persist_docs'. Expected dict "
            f"but received {type(self.persist_docs)}"
        )
        super().__init__(msg=msg)


class InlineModelConfigError(CompilationError):
    def __init__(self, node) -> None:
        self.node = node
        msg = "Invalid inline model config"
        super().__init__(msg=msg)


class ConflictingConfigKeysError(CompilationError):
    def __init__(self, oldkey: str, newkey: str, node) -> None:
        self.oldkey = oldkey
        self.newkey = newkey
        self.node = node
        msg = f'Invalid config, has conflicting keys "{self.oldkey}" and "{self.newkey}"'
        super().__init__(msg=msg)


class NumberSourceArgsError(CompilationError):
    def __init__(self, args, node) -> None:
        self.args = args
        self.node = node
        msg = f"source() takes exactly two arguments ({len(self.args)} given)"
        super().__init__(msg=msg)


class RequiredVarNotFoundError(CompilationError):
    def __init__(self, var_name: str, merged: Dict, node) -> None:
        self.var_name = var_name
        self.merged = merged
        self.node = node
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        if self.node is not None:
            node_name = self.node.name
        else:
            node_name = "<Configuration>"

        dct = {k: self.merged[k] for k in self.merged}
        pretty_vars = json.dumps(dct, sort_keys=True, indent=4)

        msg = f"Required var '{self.var_name}' not found in config:\nVars supplied to {node_name} = {pretty_vars}"
        return msg


class PackageNotFoundForMacroError(CompilationError):
    def __init__(self, package_name: str) -> None:
        self.package_name = package_name
        msg = f"Could not find package '{self.package_name}'"
        super().__init__(msg=msg)


class SecretEnvVarLocationError(ParsingError):
    def __init__(self, env_var_name: str) -> None:
        self.env_var_name = env_var_name
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            "Secret env vars are allowed only in profiles.yml or packages.yml. "
            f"Found '{self.env_var_name}' referenced elsewhere."
        )
        return msg


class MacroArgTypeError(CompilationError):
    def __init__(self, method_name: str, arg_name: str, got_value: Any, expected_type) -> None:
        self.method_name = method_name
        self.arg_name = arg_name
        self.got_value = got_value
        self.expected_type = expected_type
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        got_type = type(self.got_value)
        msg = (
            f"'adapter.{self.method_name}' expects argument "
            f"'{self.arg_name}' to be of type '{self.expected_type}', instead got "
            f"{self.got_value} ({got_type})"
        )
        return msg


class BooleanError(CompilationError):
    def __init__(self, return_value: Any, macro_name: str) -> None:
        self.return_value = return_value
        self.macro_name = macro_name
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            f"Macro '{self.macro_name}' returns '{self.return_value}'.  It is not type 'bool' "
            "and cannot not be converted reliably to a bool."
        )
        return msg


class RefArgsError(CompilationError):
    def __init__(self, node, args) -> None:
        self.node = node
        self.args = args
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f"ref() takes at most two arguments ({len(self.args)} given)"
        return msg


class MetricArgsError(CompilationError):
    def __init__(self, node, args) -> None:
        self.node = node
        self.args = args
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f"metric() takes at most two arguments ({len(self.args)} given)"
        return msg


class RefBadContextError(CompilationError):
    def __init__(self, node, args) -> None:
        self.node = node
        self.args = args.positional_args  # type: ignore
        self.kwargs = args.keyword_args  # type: ignore
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        # This explicitly references model['name'], instead of model['alias'], for
        # better error messages. Ex. If models foo_users and bar_users are aliased
        # to 'users', in their respective schemas, then you would want to see
        # 'bar_users' in your error messge instead of just 'users'.
        if isinstance(self.node, dict):
            model_name = self.node["name"]
        else:
            model_name = self.node.name

        ref_args = ", ".join("'{}'".format(a) for a in self.args)

        keyword_args = ""
        if self.kwargs:
            keyword_args = ", ".join(
                "{}='{}'".format(k, v) for k, v in self.kwargs.items()  # type: ignore
            )
            keyword_args = "," + keyword_args

        ref_string = f"{{{{ ref({ref_args}{keyword_args}) }}}}"

        msg = f"""dbt was unable to infer all dependencies for the model "{model_name}".
This typically happens when ref() is placed within a conditional block.

To fix this, add the following hint to the top of the model "{model_name}":

-- depends_on: {ref_string}"""

        return msg


class DocArgsError(CompilationError):
    def __init__(self, node, args) -> None:
        self.node = node
        self.args = args
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f"doc() takes at most two arguments ({len(self.args)} given)"
        return msg


class DocTargetNotFoundError(CompilationError):
    def __init__(
        self, node, target_doc_name: str, target_doc_package: Optional[str] = None
    ) -> None:
        self.node = node
        self.target_doc_name = target_doc_name
        self.target_doc_package = target_doc_package
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        target_package_string = ""
        if self.target_doc_package is not None:
            target_package_string = f"in package '{self. target_doc_package}' "
        msg = f"Documentation for '{self.node.unique_id}' depends on doc '{self.target_doc_name}' {target_package_string} which was not found"
        return msg


class MacroDispatchArgError(CompilationError):
    def __init__(self, macro_name: str) -> None:
        self.macro_name = macro_name
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f"""\
        The "packages" argument of adapter.dispatch() has been deprecated.
        Use the "macro_namespace" argument instead.

        Raised during dispatch for: {self.macro_name}

        For more information, see:

        https://docs.getdbt.com/reference/dbt-jinja-functions/dispatch
        """
        return msg


class DuplicateMacroNameError(CompilationError):
    def __init__(self, node_1, node_2, namespace: str) -> None:
        self.node_1 = node_1
        self.node_2 = node_2
        self.namespace = namespace
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        duped_name = self.node_1.name
        if self.node_1.package_name != self.node_2.package_name:
            extra = f' ("{self.node_1.package_name}" and "{self.node_2.package_name}" are both in the "{self.namespace}" namespace)'
        else:
            extra = ""

        msg = (
            f'dbt found two macros with the name "{duped_name}" in the namespace "{self.namespace}"{extra}. '
            "Since these macros have the same name and exist in the same "
            "namespace, dbt will be unable to decide which to call. To fix this, "
            f"change the name of one of these macros:\n- {self.node_1.unique_id} "
            f"({self.node_1.original_file_path})\n- {self.node_2.unique_id} ({self.node_2.original_file_path})"
        )

        return msg


class MacroResultAlreadyLoadedError(CompilationError):
    def __init__(self, result_name) -> None:
        self.result_name = result_name
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f"The 'statement' result named '{self.result_name}' has already been loaded into a variable"

        return msg


# parser level exceptions
class DictParseError(ParsingError):
    def __init__(self, exc: ValidationError, node) -> None:
        self.exc = exc
        self.node = node
        msg = self.validator_error_message(exc)
        super().__init__(msg=msg)


class ConfigUpdateError(ParsingError):
    def __init__(self, exc: ValidationError, node) -> None:
        self.exc = exc
        self.node = node
        msg = self.validator_error_message(exc)
        super().__init__(msg=msg)


class PythonParsingError(ParsingError):
    def __init__(self, exc: SyntaxError, node) -> None:
        self.exc = exc
        self.node = node
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        validated_exc = self.validator_error_message(self.exc)
        msg = f"{validated_exc}\n{self.exc.text}"
        return msg


class PythonLiteralEvalError(ParsingError):
    def __init__(self, exc: Exception, node) -> None:
        self.exc = exc
        self.node = node
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            f"Error when trying to literal_eval an arg to dbt.ref(), dbt.source(), dbt.config() or dbt.config.get() \n{self.exc}\n"
            "https://docs.python.org/3/library/ast.html#ast.literal_eval\n"
            "In dbt python model, `dbt.ref`, `dbt.source`, `dbt.config`, `dbt.config.get` function args only support Python literal structures"
        )

        return msg


class ModelConfigError(ParsingError):
    def __init__(self, exc: ValidationError, node) -> None:
        self.msg = self.validator_error_message(exc)
        self.node = node
        super().__init__(msg=self.msg)


class YamlParseListError(ParsingError):
    def __init__(
        self,
        path: str,
        key: str,
        yaml_data: List,
        cause,
    ) -> None:
        self.path = path
        self.key = key
        self.yaml_data = yaml_data
        self.cause = cause
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        if isinstance(self.cause, str):
            reason = self.cause
        elif isinstance(self.cause, ValidationError):
            reason = self.validator_error_message(self.cause)
        else:
            reason = self.cause.msg
        msg = f"Invalid {self.key} config given in {self.path} @ {self.key}: {self.yaml_data} - {reason}"
        return msg


class YamlParseDictError(ParsingError):
    def __init__(
        self,
        path: str,
        key: str,
        yaml_data: Dict[str, Any],
        cause,
    ) -> None:
        self.path = path
        self.key = key
        self.yaml_data = yaml_data
        self.cause = cause
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        if isinstance(self.cause, str):
            reason = self.cause
        elif isinstance(self.cause, ValidationError):
            reason = self.validator_error_message(self.cause)
        else:
            reason = self.cause.msg
        msg = f"Invalid {self.key} config given in {self.path} @ {self.key}: {self.yaml_data} - {reason}"
        return msg


class YamlLoadError(ParsingError):
    def __init__(
        self,
        path: str,
        exc: DbtValidationError,
        project_name: Optional[str] = None,
    ) -> None:
        self.project_name = project_name
        self.path = path
        self.exc = exc
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        reason = self.validator_error_message(self.exc)

        msg = f"Error reading {self.project_name}: {self.path} - {reason}"

        return msg


class TestConfigError(ParsingError):
    def __init__(self, exc: ValidationError, node) -> None:
        self.msg = self.validator_error_message(exc)
        self.node = node
        super().__init__(msg=self.msg)


class SchemaConfigError(ParsingError):
    def __init__(self, exc: ValidationError, node) -> None:
        self.msg = self.validator_error_message(exc)
        self.node = node
        super().__init__(msg=self.msg)


class SnapshopConfigError(ParsingError):
    def __init__(self, exc: ValidationError, node) -> None:
        self.msg = self.validator_error_message(exc)
        self.node = node
        super().__init__(msg=self.msg)


class DbtReferenceError(ParsingError):
    def __init__(self, unique_id: str, ref_unique_id: str, access: AccessType, scope: str) -> None:
        self.unique_id = unique_id
        self.ref_unique_id = ref_unique_id
        self.access = access
        self.scope = scope
        self.scope_type = "group" if self.access == AccessType.Private else "package"
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        return (
            f"Node {self.unique_id} attempted to reference node {self.ref_unique_id}, "
            f"which is not allowed because the referenced node is {self.access} to the '{self.scope}' {self.scope_type}."
        )


class InvalidAccessTypeError(ParsingError):
    def __init__(
        self, unique_id: str, field_value: str, materialization: Optional[str] = None
    ) -> None:
        self.unique_id = unique_id
        self.field_value = field_value
        self.materialization = materialization

        with_materialization = (
            f"with '{self.materialization}' materialization " if self.materialization else ""
        )
        msg = f"Node {self.unique_id} {with_materialization}has an invalid value ({self.field_value}) for the access field"
        super().__init__(msg=msg)


class SameKeyNestedError(CompilationError):
    def __init__(self) -> None:
        msg = "Test cannot have the same key at the top-level and in config"
        super().__init__(msg=msg)


class TestArgIncludesModelError(CompilationError):
    def __init__(self) -> None:
        msg = 'Test arguments include "model", which is a reserved argument'
        super().__init__(msg=msg)


class UnexpectedTestNamePatternError(CompilationError):
    def __init__(self, test_name: str) -> None:
        self.test_name = test_name
        msg = f"Test name string did not match expected pattern: {self.test_name}"
        super().__init__(msg=msg)


class CustomMacroPopulatingConfigValueError(CompilationError):
    def __init__(
        self,
        target_name: str,
        name: str,
        key: str,
        err_msg: str,
        column_name: Optional[str] = None,
    ) -> None:
        self.target_name = target_name
        self.column_name = column_name
        self.name = name
        self.key = key
        self.err_msg = err_msg
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        # Generic tests do not include custom macros in the Jinja
        # rendering context, so this will almost always fail. As it
        # currently stands, the error message is inscrutable, which
        # has caused issues for some projects migrating from
        # pre-0.20.0 to post-0.20.0.
        # See https://github.com/dbt-labs/dbt-core/issues/4103
        # and https://github.com/dbt-labs/dbt-core/issues/5294

        msg = (
            f"The {self.target_name}.{self.column_name} column's "
            f'"{self.name}" test references an undefined '
            f"macro in its {self.key} configuration argument. "
            f"The macro {self.err_msg}.\n"
            "Please note that the generic test configuration parser "
            "currently does not support using custom macros to "
            "populate configuration values"
        )
        return msg


class TagsNotListOfStringsError(CompilationError):
    def __init__(self, tags: Any) -> None:
        self.tags = tags
        msg = f"got {self.tags} ({type(self.tags)}) for tags, expected a list of strings"
        super().__init__(msg=msg)


class TagNotStringError(CompilationError):
    def __init__(self, tag: Any) -> None:
        self.tag = tag
        msg = f"got {self.tag} ({type(self.tag)}) for tag, expected a str"
        super().__init__(msg=msg)


class TestNameNotStringError(ParsingError):
    def __init__(self, test_name: Any) -> None:
        self.test_name = test_name
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:

        msg = f"test name must be a str, got {type(self.test_name)} (value {self.test_name})"
        return msg


class TestArgsNotDictError(ParsingError):
    def __init__(self, test_args: Any) -> None:
        self.test_args = test_args
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:

        msg = f"test arguments must be a dict, got {type(self.test_args)} (value {self.test_args})"
        return msg


class TestDefinitionDictLengthError(ParsingError):
    def __init__(self, test):
        self.test = test
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:

        msg = (
            "test definition dictionary must have exactly one key, got"
            f" {self.test} instead ({len(self.test)} keys)"
        )
        return msg


class TestTypeError(ParsingError):
    def __init__(self, test: Any):
        self.test = test
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f"test must be dict or str, got {type(self.test)} (value {self.test})"
        return msg


# This is triggered across multiple files
class EnvVarMissingError(ParsingError):
    def __init__(self, var: str):
        self.var = var
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f"Env var required but not provided: '{self.var}'"
        return msg


class TargetNotFoundError(CompilationError):
    def __init__(
        self,
        node,
        target_name: str,
        target_kind: str,
        target_package: Optional[str] = None,
        target_version: Optional[Union[str, float]] = None,
        disabled: Optional[bool] = None,
    ):
        self.node = node
        self.target_name = target_name
        self.target_kind = target_kind
        self.target_package = target_package
        self.target_version = target_version
        self.disabled = disabled
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        original_file_path = self.node.original_file_path
        unique_id = self.node.unique_id
        resource_type_title = self.node.resource_type.title()

        if self.disabled is None:
            reason = "was not found or is disabled"
        elif self.disabled is True:
            reason = "is disabled"
        else:
            reason = "was not found"

        target_version_string = ""
        if self.target_version is not None:
            target_version_string = f"with version '{self.target_version}' "

        target_package_string = ""
        if self.target_package is not None:
            target_package_string = f"in package or project '{self.target_package}' "

        msg = (
            f"{resource_type_title} '{unique_id}' ({original_file_path}) depends on a "
            f"{self.target_kind} named '{self.target_name}' {target_version_string}{target_package_string}which {reason}"
        )
        return msg


class DuplicateSourcePatchNameError(CompilationError):
    def __init__(self, patch_1, patch_2):
        self.patch_1 = patch_1
        self.patch_2 = patch_2
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        name = f"{self.patch_1.overrides}.{self.patch_1.name}"
        fix = self._fix_dupe_msg(
            self.patch_1.path,
            self.patch_2.path,
            name,
            "sources",
        )
        msg = (
            f"dbt found two schema.yml entries for the same source named "
            f"{self.patch_1.name} in package {self.patch_1.overrides}. Sources may only be "
            f"overridden a single time. To fix this, {fix}"
        )
        return msg


class DuplicateMacroPatchNameError(CompilationError):
    def __init__(self, patch_1, existing_patch_path):
        self.patch_1 = patch_1
        self.existing_patch_path = existing_patch_path
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        package_name = self.patch_1.package_name
        name = self.patch_1.name
        fix = self._fix_dupe_msg(
            self.patch_1.original_file_path, self.existing_patch_path, name, "macros"
        )
        msg = (
            f"dbt found two schema.yml entries for the same macro in package "
            f"{package_name} named {name}. Macros may only be described a single "
            f"time. To fix this, {fix}"
        )
        return msg


# core level exceptions
class DuplicateAliasError(AliasError):
    def __init__(self, kwargs: Mapping[str, Any], aliases: Mapping[str, str], canonical_key: str):
        self.kwargs = kwargs
        self.aliases = aliases
        self.canonical_key = canonical_key
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        # dupe found: go through the dict so we can have a nice-ish error
        key_names = ", ".join(
            "{}".format(k) for k in self.kwargs if self.aliases.get(k) == self.canonical_key
        )
        msg = f'Got duplicate keys: ({key_names}) all map to "{self.canonical_key}"'
        return msg


# Postgres Exceptions
class UnexpectedDbReferenceError(NotImplementedError):
    def __init__(self, adapter, database, expected):
        self.adapter = adapter
        self.database = database
        self.expected = expected
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f"Cross-db references not allowed in {self.adapter} ({self.database} vs {self.expected})"
        return msg


class CrossDbReferenceProhibitedError(CompilationError):
    def __init__(self, adapter, exc_msg: str):
        self.adapter = adapter
        self.exc_msg = exc_msg
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f"Cross-db references not allowed in adapter {self.adapter}: Got {self.exc_msg}"
        return msg


class IndexConfigNotDictError(CompilationError):
    def __init__(self, raw_index: Any):
        self.raw_index = raw_index
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            f"Invalid index config:\n"
            f"  Got: {self.raw_index}\n"
            f'  Expected a dictionary with at minimum a "columns" key'
        )
        return msg


class IndexConfigError(CompilationError):
    def __init__(self, exc: TypeError):
        self.exc = exc
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        validator_msg = self.validator_error_message(self.exc)
        msg = f"Could not parse index config: {validator_msg}"
        return msg


# adapters exceptions
class MacroResultError(CompilationError):
    def __init__(self, freshness_macro_name: str, table):
        self.freshness_macro_name = freshness_macro_name
        self.table = table
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f'Got an invalid result from "{self.freshness_macro_name}" macro: {[tuple(r) for r in self.table]}'

        return msg


class SnapshotTargetNotSnapshotTableError(CompilationError):
    def __init__(self, missing: List):
        self.missing = missing
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = 'Snapshot target is not a snapshot table (missing "{}")'.format(
            '", "'.join(self.missing)
        )
        return msg


class SnapshotTargetIncompleteError(CompilationError):
    def __init__(self, extra: List, missing: List):
        self.extra = extra
        self.missing = missing
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            'Snapshot target has ("{}") but not ("{}") - is it an '
            "unmigrated previous version archive?".format(
                '", "'.join(self.extra), '", "'.join(self.missing)
            )
        )
        return msg


class RenameToNoneAttemptedError(CompilationError):
    def __init__(self, src_name: str, dst_name: str, name: str):
        self.src_name = src_name
        self.dst_name = dst_name
        self.name = name
        self.msg = f"Attempted to rename {self.src_name} to {self.dst_name} for {self.name}"
        super().__init__(msg=self.msg)


class NullRelationDropAttemptedError(CompilationError):
    def __init__(self, name: str):
        self.name = name
        self.msg = f"Attempted to drop a null relation for {self.name}"
        super().__init__(msg=self.msg)


class NullRelationCacheAttemptedError(CompilationError):
    def __init__(self, name: str):
        self.name = name
        self.msg = f"Attempted to cache a null relation for {self.name}"
        super().__init__(msg=self.msg)


class QuoteConfigTypeError(CompilationError):
    def __init__(self, quote_config: Any):
        self.quote_config = quote_config
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            'The seed configuration value of "quote_columns" has an '
            f"invalid type {type(self.quote_config)}"
        )
        return msg


class MultipleDatabasesNotAllowedError(CompilationError):
    def __init__(self, databases):
        self.databases = databases
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = str(self.databases)
        return msg


class RelationTypeNullError(CompilationError):
    def __init__(self, relation):
        self.relation = relation
        self.msg = f"Tried to drop relation {self.relation}, but its type is null."
        super().__init__(msg=self.msg)


class MaterializationNotAvailableError(CompilationError):
    def __init__(self, materialization, adapter_type: str):
        self.materialization = materialization
        self.adapter_type = adapter_type
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f"Materialization '{self.materialization}' is not available for {self.adapter_type}!"
        return msg


class RelationReturnedMultipleResultsError(CompilationError):
    def __init__(self, kwargs: Mapping[str, Any], matches: List):
        self.kwargs = kwargs
        self.matches = matches
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            "get_relation returned more than one relation with the given args. "
            "Please specify a database or schema to narrow down the result set."
            f"\n{self.kwargs}\n\n{self.matches}"
        )
        return msg


class ApproximateMatchError(CompilationError):
    def __init__(self, target, relation):
        self.target = target
        self.relation = relation
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:

        msg = (
            "When searching for a relation, dbt found an approximate match. "
            "Instead of guessing \nwhich relation to use, dbt will move on. "
            f"Please delete {self.relation}, or rename it to be less ambiguous."
            f"\nSearched for: {self.target}\nFound: {self.relation}"
        )

        return msg


class UnexpectedNullError(DbtDatabaseError):
    def __init__(self, field_name: str, source):
        self.field_name = field_name
        self.source = source
        msg = (
            f"Expected a non-null value when querying field '{self.field_name}' of table "
            f" {self.source} but received value 'null' instead"
        )
        super().__init__(msg)


class UnexpectedNonTimestampError(DbtDatabaseError):
    def __init__(self, field_name: str, source, dt: Any):
        self.field_name = field_name
        self.source = source
        self.type_name = type(dt).__name__
        msg = (
            f"Expected a timestamp value when querying field '{self.field_name}' of table "
            f"{self.source} but received value of type '{self.type_name}' instead"
        )
        super().__init__(msg)


# deps exceptions
class MultipleVersionGitDepsError(DependencyError):
    def __init__(self, git: str, requested):
        self.git = git
        self.requested = requested
        msg = (
            "git dependencies should contain exactly one version. "
            f"{self.git} contains: {self.requested}"
        )
        super().__init__(msg)


class DuplicateProjectDependencyError(DependencyError):
    def __init__(self, project_name: str):
        self.project_name = project_name
        msg = (
            f'Found duplicate project "{self.project_name}". This occurs when '
            "a dependency has the same project name as some other dependency."
        )
        super().__init__(msg)


class DuplicateDependencyToRootError(DependencyError):
    def __init__(self, project_name: str):
        self.project_name = project_name
        msg = (
            "Found a dependency with the same name as the root project "
            f'"{self.project_name}". Package names must be unique in a project.'
            " Please rename one of these packages."
        )
        super().__init__(msg)


class MismatchedDependencyTypeError(DependencyError):
    def __init__(self, new, old):
        self.new = new
        self.old = old
        msg = (
            f"Cannot incorporate {self.new} ({self.new.__class__.__name__}) in {self.old} "
            f"({self.old.__class__.__name__}): mismatched types"
        )
        super().__init__(msg)


class PackageVersionNotFoundError(DependencyError):
    def __init__(
        self,
        package_name: str,
        version_range,
        available_versions: List[str],
        should_version_check: bool,
    ):
        self.package_name = package_name
        self.version_range = version_range
        self.available_versions = available_versions
        self.should_version_check = should_version_check
        super().__init__(self.get_message())

    def get_message(self) -> str:
        base_msg = (
            "Could not find a matching compatible version for package {}\n"
            "  Requested range: {}\n"
            "  Compatible versions: {}\n"
        )
        addendum = (
            (
                "\n"
                "  Not shown: package versions incompatible with installed version of dbt-core\n"
                "  To include them, run 'dbt --no-version-check deps'"
            )
            if self.should_version_check
            else ""
        )
        msg = (
            base_msg.format(self.package_name, self.version_range, self.available_versions)
            + addendum
        )
        return msg


class PackageNotFoundError(DependencyError):
    def __init__(self, package_name: str):
        self.package_name = package_name
        msg = f"Package {self.package_name} was not found in the package index"
        super().__init__(msg)


# config level exceptions
class ProfileConfigError(DbtProfileError):
    def __init__(self, exc: ValidationError):
        self.exc = exc
        msg = self.validator_error_message(self.exc)
        super().__init__(msg=msg)


class ProjectContractError(DbtProjectError):
    def __init__(self, exc: ValidationError):
        self.exc = exc
        msg = self.validator_error_message(self.exc)
        super().__init__(msg=msg)


class ProjectContractBrokenError(DbtProjectError):
    def __init__(self, exc: ValidationError):
        self.exc = exc
        msg = self.validator_error_message(self.exc)
        super().__init__(msg=msg)


class ConfigContractBrokenError(DbtProjectError):
    def __init__(self, exc: ValidationError):
        self.exc = exc
        msg = self.validator_error_message(self.exc)
        super().__init__(msg=msg)


class NonUniquePackageNameError(CompilationError):
    def __init__(self, project_name: str):
        self.project_name = project_name
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            "dbt found more than one package with the name "
            f'"{self.project_name}" included in this project. Package '
            "names must be unique in a project. Please rename "
            "one of these packages."
        )
        return msg


class UninstalledPackagesFoundError(CompilationError):
    def __init__(
        self,
        count_packages_specified: int,
        count_packages_installed: int,
        packages_specified_path: str,
        packages_install_path: str,
    ):
        self.count_packages_specified = count_packages_specified
        self.count_packages_installed = count_packages_installed
        self.packages_specified_path = packages_specified_path
        self.packages_install_path = packages_install_path
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            f"dbt found {self.count_packages_specified} package(s) "
            f"specified in {self.packages_specified_path}, but only "
            f"{self.count_packages_installed} package(s) installed "
            f'in {self.packages_install_path}. Run "dbt deps" to '
            "install package dependencies."
        )
        return msg


class OptionNotYamlDictError(CompilationError):
    def __init__(self, var_type, option_name):
        self.var_type = var_type
        self.option_name = option_name
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        type_name = self.var_type.__name__

        msg = f"The --{self.option_name} argument must be a YAML dictionary, but was of type '{type_name}'"
        return msg


# contracts level
class UnrecognizedCredentialTypeError(CompilationError):
    def __init__(self, typename: str, supported_types: List):
        self.typename = typename
        self.supported_types = supported_types
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = 'Unrecognized credentials type "{}" - supported types are ({})'.format(
            self.typename, ", ".join('"{}"'.format(t) for t in self.supported_types)
        )
        return msg


class DuplicateMacroInPackageError(CompilationError):
    def __init__(self, macro, macro_mapping: Mapping):
        self.macro = macro
        self.macro_mapping = macro_mapping
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        other_path = self.macro_mapping[self.macro.unique_id].original_file_path
        # subtract 2 for the "Compilation Error" indent
        # note that the line wrap eats newlines, so if you want newlines,
        # this is the result :(
        msg = line_wrap_message(
            f"""\
            dbt found two macros named "{self.macro.name}" in the project
            "{self.macro.package_name}".


            To fix this error, rename or remove one of the following
            macros:

                - {self.macro.original_file_path}

                - {other_path}
            """,
            subtract=2,
        )
        return msg


class DuplicateMaterializationNameError(CompilationError):
    def __init__(self, macro, other_macro):
        self.macro = macro
        self.other_macro = other_macro
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        macro_name = self.macro.name
        macro_package_name = self.macro.package_name
        other_package_name = self.other_macro.macro.package_name

        msg = (
            f"Found two materializations with the name {macro_name} (packages "
            f"{macro_package_name} and {other_package_name}). dbt cannot resolve "
            "this ambiguity"
        )
        return msg


# jinja exceptions
class ColumnTypeMissingError(CompilationError):
    def __init__(self, column_names: List):
        self.column_names = column_names
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            "Contracted models require data_type to be defined for each column. "
            "Please ensure that the column name and data_type are defined within "
            f"the YAML configuration for the {self.column_names} column(s)."
        )
        return msg


class PatchTargetNotFoundError(CompilationError):
    def __init__(self, patches: Dict):
        self.patches = patches
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        patch_list = "\n\t".join(
            f"model {p.name} (referenced in path {p.original_file_path})"
            for p in self.patches.values()
        )
        msg = f"dbt could not find models for the following patches:\n\t{patch_list}"
        return msg


class MacroNotFoundError(CompilationError):
    def __init__(self, node, target_macro_id: str):
        self.node = node
        self.target_macro_id = target_macro_id
        msg = f"'{self.node.unique_id}' references macro '{self.target_macro_id}' which is not defined!"

        super().__init__(msg=msg)


class MissingConfigError(CompilationError):
    def __init__(self, unique_id: str, name: str):
        self.unique_id = unique_id
        self.name = name
        msg = (
            f"Model '{self.unique_id}' does not define a required config parameter '{self.name}'."
        )
        super().__init__(msg=msg)


class MissingMaterializationError(CompilationError):
    def __init__(self, materialization, adapter_type):
        self.materialization = materialization
        self.adapter_type = adapter_type
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:

        valid_types = "'default'"

        if self.adapter_type != "default":
            valid_types = f"'default' and '{self.adapter_type}'"

        msg = f"No materialization '{self.materialization}' was found for adapter {self.adapter_type}! (searched types {valid_types})"
        return msg


class MissingRelationError(CompilationError):
    def __init__(self, relation, model=None):
        self.relation = relation
        self.model = model
        msg = f"Relation {self.relation} not found!"
        super().__init__(msg=msg)


class AmbiguousAliasError(CompilationError):
    def __init__(self, node_1, node_2, duped_name=None):
        self.node_1 = node_1
        self.node_2 = node_2
        if duped_name is None:
            self.duped_name = f"{self.node_1.database}.{self.node_1.schema}.{self.node_1.alias}"
        else:
            self.duped_name = duped_name
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:

        msg = (
            f'dbt found two resources with the database representation "{self.duped_name}".\ndbt '
            "cannot create two resources with identical database representations. "
            "To fix this,\nchange the configuration of one of these resources:"
            f"\n- {self.node_1.unique_id} ({self.node_1.original_file_path})\n- {self.node_2.unique_id} ({self.node_2.original_file_path})"
        )
        return msg


class AmbiguousResourceNameRefError(CompilationError):
    def __init__(self, duped_name, unique_ids, node=None):
        self.duped_name = duped_name
        self.unique_ids = unique_ids
        self.packages = [unique_id.split(".")[1] for unique_id in unique_ids]
        super().__init__(msg=self.get_message(), node=node)

    def get_message(self) -> str:
        formatted_unique_ids = "'{0}'".format("', '".join(self.unique_ids))
        formatted_packages = "'{0}'".format("' or '".join(self.packages))
        msg = (
            f"When referencing '{self.duped_name}', dbt found nodes in multiple packages: {formatted_unique_ids}"
            f"\nTo fix this, use two-argument 'ref', with the package name first: {formatted_packages}"
        )
        return msg


class AmbiguousCatalogMatchError(CompilationError):
    def __init__(self, unique_id: str, match_1, match_2):
        self.unique_id = unique_id
        self.match_1 = match_1
        self.match_2 = match_2
        super().__init__(msg=self.get_message())

    def get_match_string(self, match):
        match_schema = match.get("metadata", {}).get("schema")
        match_name = match.get("metadata", {}).get("name")
        return f"{match_schema}.{match_name}"

    def get_message(self) -> str:
        msg = (
            "dbt found two relations in your warehouse with similar database identifiers. "
            "dbt\nis unable to determine which of these relations was created by the model "
            f'"{self.unique_id}".\nIn order for dbt to correctly generate the catalog, one '
            "of the following relations must be deleted or renamed:\n\n - "
            f"{self.get_match_string(self.match_1)}\n - {self.get_match_string(self.match_2)}"
        )

        return msg


class CacheInconsistencyError(DbtInternalError):
    def __init__(self, msg: str):
        self.msg = msg
        formatted_msg = f"Cache inconsistency detected: {self.msg}"
        super().__init__(msg=formatted_msg)


class NewNameAlreadyInCacheError(CacheInconsistencyError):
    def __init__(self, old_key: str, new_key: str):
        self.old_key = old_key
        self.new_key = new_key
        msg = (
            f'in rename of "{self.old_key}" -> "{self.new_key}", new name is in the cache already'
        )
        super().__init__(msg)


class ReferencedLinkNotCachedError(CacheInconsistencyError):
    def __init__(self, referenced_key: str):
        self.referenced_key = referenced_key
        msg = f"in add_link, referenced link key {self.referenced_key} not in cache!"
        super().__init__(msg)


class DependentLinkNotCachedError(CacheInconsistencyError):
    def __init__(self, dependent_key: str):
        self.dependent_key = dependent_key
        msg = f"in add_link, dependent link key {self.dependent_key} not in cache!"
        super().__init__(msg)


class TruncatedModelNameCausedCollisionError(CacheInconsistencyError):
    def __init__(self, new_key, relations: Dict):
        self.new_key = new_key
        self.relations = relations
        super().__init__(self.get_message())

    def get_message(self) -> str:
        # Tell user when collision caused by model names truncated during
        # materialization.
        match = re.search("__dbt_backup|__dbt_tmp$", self.new_key.identifier)
        if match:
            truncated_model_name_prefix = self.new_key.identifier[: match.start()]
            message_addendum = (
                "\n\nName collisions can occur when the length of two "
                "models' names approach your database's builtin limit. "
                "Try restructuring your project such that no two models "
                f"share the prefix '{truncated_model_name_prefix}'. "
                "Then, clean your warehouse of any removed models."
            )
        else:
            message_addendum = ""

        msg = f"in rename, new key {self.new_key} already in cache: {list(self.relations.keys())}{message_addendum}"

        return msg


class NoneRelationFoundError(CacheInconsistencyError):
    def __init__(self):
        msg = "in get_relations, a None relation was found in the cache!"
        super().__init__(msg)


# this is part of the context and also raised in dbt.contracts.relation.py
class DataclassNotDictError(CompilationError):
    def __init__(self, obj: Any):
        self.obj = obj
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            f'The object ("{self.obj}") was used as a dictionary. This '
            "capability has been removed from objects of this type."
        )

        return msg


class DependencyNotFoundError(CompilationError):
    def __init__(self, node, node_description, required_pkg):
        self.node = node
        self.node_description = node_description
        self.required_pkg = required_pkg
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            f"Error while parsing {self.node_description}.\nThe required package "
            f'"{self.required_pkg}" was not found. Is the package installed?\n'
            "Hint: You may need to run `dbt deps`."
        )

        return msg


class DuplicatePatchPathError(CompilationError):
    def __init__(self, patch_1, existing_patch_path):
        self.patch_1 = patch_1
        self.existing_patch_path = existing_patch_path
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        name = self.patch_1.name
        fix = self._fix_dupe_msg(
            self.patch_1.original_file_path,
            self.existing_patch_path,
            name,
            "resource",
        )
        msg = (
            f"dbt found two schema.yml entries for the same resource named "
            f"{name}. Resources and their associated columns may only be "
            f"described a single time. To fix this, {fix}"
        )
        return msg


# should this inherit ParsingError instead?
class DuplicateResourceNameError(CompilationError):
    def __init__(self, node_1, node_2):
        self.node_1 = node_1
        self.node_2 = node_2
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        duped_name = self.node_1.name
        node_type = NodeType(self.node_1.resource_type)
        pluralized = (
            node_type.pluralize()
            if self.node_1.resource_type == self.node_2.resource_type
            else "resources"  # still raise if ref() collision, e.g. model + seed
        )

        action = "looking for"
        # duplicate 'ref' targets
        if node_type in NodeType.refable():
            formatted_name = f'ref("{duped_name}")'
        # duplicate sources
        elif node_type == NodeType.Source:
            duped_name = self.node_1.get_full_source_name()
            formatted_name = self.node_1.get_source_representation()
        # duplicate docs blocks
        elif node_type == NodeType.Documentation:
            formatted_name = f'doc("{duped_name}")'
        # duplicate generic tests
        elif node_type == NodeType.Test and hasattr(self.node_1, "test_metadata"):
            column_name = (
                f'column "{self.node_1.column_name}" in ' if self.node_1.column_name else ""
            )
            model_name = self.node_1.file_key_name
            duped_name = f'{self.node_1.name}" defined on {column_name}"{model_name}'
            action = "running"
            formatted_name = "tests"
        # all other resource types
        else:
            formatted_name = duped_name

        msg = f"""
dbt found two {pluralized} with the name "{duped_name}".

Since these resources have the same name, dbt will be unable to find the correct resource
when {action} {formatted_name}.

To fix this, change the name of one of these resources:
- {self.node_1.unique_id} ({self.node_1.original_file_path})
- {self.node_2.unique_id} ({self.node_2.original_file_path})
    """.strip()
        return msg


class DuplicateVersionedUnversionedError(ParsingError):
    def __init__(self, versioned_node, unversioned_node):
        self.versioned_node = versioned_node
        self.unversioned_node = unversioned_node
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = f"""
dbt found versioned and unversioned models with the name "{self.versioned_node.name}".

Since these resources have the same name, dbt will be unable to find the correct resource
when looking for ref('{self.versioned_node.name}').

To fix this, change the name of the unversioned resource
{self.unversioned_node.unique_id} ({self.unversioned_node.original_file_path})
or add the unversioned model to the versions in {self.versioned_node.patch_path}
    """.strip()
        return msg


class PropertyYMLError(CompilationError):
    def __init__(self, path: str, issue: str):
        self.path = path
        self.issue = issue
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            f"The yml property file at {self.path} is invalid because {self.issue}. "
            "Please consult the documentation for more information on yml property file "
            "syntax:\n\nhttps://docs.getdbt.com/reference/configs-and-properties"
        )
        return msg


class RelationWrongTypeError(CompilationError):
    def __init__(self, relation, expected_type, model=None):
        self.relation = relation
        self.expected_type = expected_type
        self.model = model
        super().__init__(msg=self.get_message())

    def get_message(self) -> str:
        msg = (
            f"Trying to create {self.expected_type} {self.relation}, "
            f"but it currently exists as a {self.relation.type}. Either "
            f"drop {self.relation} manually, or run dbt with "
            "`--full-refresh` and dbt will drop it for you."
        )

        return msg


class ContractError(CompilationError):
    def __init__(self, yaml_columns, sql_columns):
        self.yaml_columns = yaml_columns
        self.sql_columns = sql_columns
        super().__init__(msg=self.get_message())

    def get_mismatches(self) -> agate.Table:
        # avoid a circular import
        from dbt.clients.agate_helper import table_from_data_flat

        column_names = ["column_name", "definition_type", "contract_type", "mismatch_reason"]
        # list of mismatches
        mismatches: List[Dict[str, str]] = []
        # track sql cols so we don't need another for loop later
        sql_col_set = set()
        # for each sql col list
        for sql_col in self.sql_columns:
            # add sql col to set
            sql_col_set.add(sql_col["name"])
            # for each yaml col list
            for i, yaml_col in enumerate(self.yaml_columns):
                # if name matches
                if sql_col["name"] == yaml_col["name"]:
                    # if type matches
                    if sql_col["data_type"] == yaml_col["data_type"]:
                        # its a perfect match! don't include in mismatch table
                        break
                    else:
                        # same name, diff type
                        row = [
                            sql_col["name"],
                            sql_col["data_type"],
                            yaml_col["data_type"],
                            "data type mismatch",
                        ]
                        mismatches += [dict(zip(column_names, row))]
                        break
                # if last loop, then no name match
                if i == len(self.yaml_columns) - 1:
                    row = [sql_col["name"], sql_col["data_type"], "", "missing in contract"]
                    mismatches += [dict(zip(column_names, row))]

        # now add all yaml cols without a match
        for yaml_col in self.yaml_columns:
            if yaml_col["name"] not in sql_col_set:
                row = [yaml_col["name"], "", yaml_col["data_type"], "missing in definition"]
                mismatches += [dict(zip(column_names, row))]

        mismatches_sorted = sorted(mismatches, key=lambda d: d["column_name"])
        return table_from_data_flat(mismatches_sorted, column_names)

    def get_message(self) -> str:
        if not self.yaml_columns:
            return (
                "This model has an enforced contract, and its 'columns' specification is missing"
            )

        table: agate.Table = self.get_mismatches()
        # Hack to get Agate table output as string
        output = io.StringIO()
        table.print_table(output=output, max_rows=None, max_column_width=50)  # type: ignore
        mismatches = output.getvalue()

        msg = (
            "This model has an enforced contract that failed.\n"
            "Please ensure the name, data_type, and number of columns in your contract "
            "match the columns in your model's definition.\n\n"
            f"{mismatches}"
        )

        return msg


# not modifying these since rpc should be deprecated soon
class UnknownAsyncIDException(Exception):
    CODE = 10012
    MESSAGE = "RPC server got an unknown async ID"

    def __init__(self, task_id):
        self.task_id = task_id

    def __str__(self):
        return f"{self.MESSAGE}: {self.task_id}"


class RPCFailureResult(DbtRuntimeError):
    CODE = 10002
    MESSAGE = "RPC execution error"


class RPCTimeoutException(DbtRuntimeError):
    CODE = 10008
    MESSAGE = "RPC timeout error"

    def __init__(self, timeout: Optional[float] = None):
        super().__init__(self.MESSAGE)
        self.timeout = timeout

    def data(self):
        result = super().data()
        result.update(
            {
                "timeout": self.timeout,
                "message": f"RPC timed out after {self.timeout}s",
            }
        )
        return result


class RPCKilledException(DbtRuntimeError):
    CODE = 10009
    MESSAGE = "RPC process killed"

    def __init__(self, signum: int):
        self.signum = signum
        self.msg = f"RPC process killed by signal {self.signum}"
        super().__init__(self.msg)

    def data(self):
        return {
            "signum": self.signum,
            "message": self.msg,
        }


class RPCCompiling(DbtRuntimeError):
    CODE = 10010
    MESSAGE = 'RPC server is compiling the project, call the "status" method for' " compile status"

    def __init__(self, msg: Optional[str] = None, node=None):
        if msg is None:
            msg = "compile in progress"
        super().__init__(msg, node)


class RPCLoadException(DbtRuntimeError):
    CODE = 10011
    MESSAGE = (
        'RPC server failed to compile project, call the "status" method for' " compile status"
    )

    def __init__(self, cause: Dict[str, Any]):
        self.cause = cause
        self.msg = f'{self.MESSAGE}: {self.cause["message"]}'
        super().__init__(self.msg)

    def data(self):
        return {"cause": self.cause, "message": self.msg}
