import re
from copy import deepcopy
from typing import Any, Dict, Generic, List, Optional, Tuple

from dbt.artifacts.resources import NodeVersion
from dbt.clients.jinja import GENERIC_TEST_KWARGS_NAME, get_rendered
from dbt.contracts.graph.nodes import UnpatchedSourceDefinition
from dbt.contracts.graph.unparsed import UnparsedModelUpdate, UnparsedNodeUpdate
from dbt.exceptions import (
    CustomMacroPopulatingConfigValueError,
    SameKeyNestedError,
    TagNotStringError,
    TagsNotListOfStringsError,
    TestArgIncludesModelError,
    TestArgsNotDictError,
    TestDefinitionDictLengthError,
    TestNameNotStringError,
    TestTypeError,
    UnexpectedTestNamePatternError,
)
from dbt.parser.common import Testable
from dbt.utils import md5
from dbt_common.exceptions.macros import UndefinedMacroError


def synthesize_generic_test_names(
    test_type: str, test_name: str, args: Dict[str, Any]
) -> Tuple[str, str]:
    # Using the type, name, and arguments to this generic test, synthesize a (hopefully) unique name
    # Will not be unique if multiple tests have same name + arguments, and only configs differ
    # Returns a shorter version (hashed/truncated, for the compiled file)
    # as well as the full name (for the unique_id + FQN)
    flat_args = []
    for arg_name in sorted(args):
        # the model is already embedded in the name, so skip it
        if arg_name == "model":
            continue
        arg_val = args[arg_name]

        if isinstance(arg_val, dict):
            parts = list(arg_val.values())
        elif isinstance(arg_val, (list, tuple)):
            parts = list(arg_val)
        else:
            parts = [arg_val]

        flat_args.extend([str(part) for part in parts])

    clean_flat_args = [re.sub("[^0-9a-zA-Z_]+", "_", arg) for arg in flat_args]
    unique = "__".join(clean_flat_args)

    # for the file path + alias, the name must be <64 characters
    # if the full name is too long, include the first 30 identifying chars plus
    # a 32-character hash of the full contents

    test_identifier = "{}_{}".format(test_type, test_name)
    full_name = "{}_{}".format(test_identifier, unique)

    if len(full_name) >= 64:
        test_trunc_identifier = test_identifier[:30]
        label = md5(full_name)
        short_name = "{}_{}".format(test_trunc_identifier, label)
    else:
        short_name = full_name

    return short_name, full_name


class TestBuilder(Generic[Testable]):
    """An object to hold assorted test settings and perform basic parsing

    Test names have the following pattern:
        - the test name itself may be namespaced (package.test)
        - or it may not be namespaced (test)

    """

    # The 'test_name' is used to find the 'macro' that implements the test
    TEST_NAME_PATTERN = re.compile(
        r"((?P<test_namespace>([a-zA-Z_][0-9a-zA-Z_]*))\.)?"
        r"(?P<test_name>([a-zA-Z_][0-9a-zA-Z_]*))"
    )
    # args in the test entry representing test configs
    CONFIG_ARGS = (
        "severity",
        "tags",
        "enabled",
        "where",
        "limit",
        "warn_if",
        "error_if",
        "fail_calc",
        "store_failures",
        "store_failures_as",
        "sql_header",
        "meta",
        "database",
        "schema",
        "alias",
    )

    def __init__(
        self,
        data_test: Dict[str, Any],
        target: Testable,
        package_name: str,
        render_ctx: Dict[str, Any],
        column_name: Optional[str] = None,
        version: Optional[NodeVersion] = None,
    ) -> None:
        test_name, test_args = self.extract_test_args(data_test, column_name)
        self.args: Dict[str, Any] = test_args
        if "model" in self.args:
            raise TestArgIncludesModelError()
        self.package_name: str = package_name
        self.target: Testable = target
        self.version: Optional[NodeVersion] = version
        self.render_ctx: Dict[str, Any] = render_ctx
        self.column_name: Optional[str] = column_name
        self.args["model"] = self.build_model_str()

        match = self.TEST_NAME_PATTERN.match(test_name)
        if match is None:
            raise UnexpectedTestNamePatternError(test_name)

        groups = match.groupdict()
        self.name: str = groups["test_name"]
        self.namespace: str = groups["test_namespace"]
        self.config: Dict[str, Any] = {}
        # Process legacy args
        self.config.update(self._process_legacy_args())

        # Process config args if present
        if "config" in self.args:
            self.config.update(self._render_values(self.args.pop("config", {})))

        if self.namespace is not None:
            self.package_name = self.namespace

        # If the user has provided a description for this generic test, use it
        # Then delete the "description" argument to:
        # 1. Avoid passing it into the test macro
        # 2. Avoid passing it into the test name synthesis
        # Otherwise, use an empty string
        self.description: str = ""

        if "description" in self.args:
            self.description = self.args["description"]
            del self.args["description"]

        # If the user has provided a custom name for this generic test, use it
        # Then delete the "name" argument to avoid passing it into the test macro
        # Otherwise, use an auto-generated name synthesized from test inputs
        self.compiled_name: str = ""
        self.fqn_name: str = ""

        if "name" in self.args:
            # Assign the user-defined name here, which will be checked for uniqueness later
            # we will raise an error if two tests have same name for same model + column combo
            self.compiled_name = self.args["name"]
            self.fqn_name = self.args["name"]
            del self.args["name"]
        else:
            short_name, full_name = self.get_synthetic_test_names()
            self.compiled_name = short_name
            self.fqn_name = full_name
            # use hashed name as alias if full name is too long
            if short_name != full_name and "alias" not in self.config:
                self.config["alias"] = short_name

    def _process_legacy_args(self):
        config = {}
        for key in self.CONFIG_ARGS:
            value = self.args.pop(key, None)
            if value and "config" in self.args and key in self.args["config"]:
                raise SameKeyNestedError()
            if not value and "config" in self.args:
                value = self.args["config"].pop(key, None)
            config[key] = value

        return self._render_values(config)

    def _render_values(self, config: Dict[str, Any]) -> Dict[str, Any]:
        rendered_config = {}
        for key, value in config.items():
            if isinstance(value, str):
                try:
                    value = get_rendered(value, self.render_ctx, native=True)
                except UndefinedMacroError as e:
                    raise CustomMacroPopulatingConfigValueError(
                        target_name=self.target.name,
                        column_name=self.column_name,
                        name=self.name,
                        key=key,
                        err_msg=e.msg,
                    )
            if value is not None:
                rendered_config[key] = value
        return rendered_config

    def _bad_type(self) -> TypeError:
        return TypeError('invalid target type "{}"'.format(type(self.target)))

    @staticmethod
    def extract_test_args(data_test, name=None) -> Tuple[str, Dict[str, Any]]:
        if not isinstance(data_test, dict):
            raise TestTypeError(data_test)

        # If the test is a dictionary with top-level keys, the test name is "test_name"
        # and the rest are arguments
        # {'name': 'my_favorite_test', 'test_name': 'unique', 'config': {'where': '1=1'}}
        if "test_name" in data_test.keys():
            test_name = data_test.pop("test_name")
            test_args = data_test
        # If the test is a nested dictionary with one top-level key, the test name
        # is the dict name, and nested keys are arguments
        # {'unique': {'name': 'my_favorite_test', 'config': {'where': '1=1'}}}
        else:
            data_test = list(data_test.items())
            if len(data_test) != 1:
                raise TestDefinitionDictLengthError(data_test)
            test_name, test_args = data_test[0]

        if not isinstance(test_args, dict):
            raise TestArgsNotDictError(test_args)
        if not isinstance(test_name, str):
            raise TestNameNotStringError(test_name)
        test_args = deepcopy(test_args)
        if name is not None:
            test_args["column_name"] = name
        return test_name, test_args

    @property
    def enabled(self) -> Optional[bool]:
        return self.config.get("enabled")

    @property
    def alias(self) -> Optional[str]:
        return self.config.get("alias")

    @property
    def severity(self) -> Optional[str]:
        sev = self.config.get("severity")
        if sev:
            return sev.upper()
        else:
            return None

    @property
    def store_failures(self) -> Optional[bool]:
        return self.config.get("store_failures")

    @property
    def store_failures_as(self) -> Optional[bool]:
        return self.config.get("store_failures_as")

    @property
    def sql_header(self) -> Optional[str]:
        return self.config.get("sql_header")

    @property
    def where(self) -> Optional[str]:
        return self.config.get("where")

    @property
    def limit(self) -> Optional[int]:
        return self.config.get("limit")

    @property
    def warn_if(self) -> Optional[str]:
        return self.config.get("warn_if")

    @property
    def error_if(self) -> Optional[str]:
        return self.config.get("error_if")

    @property
    def fail_calc(self) -> Optional[str]:
        return self.config.get("fail_calc")

    @property
    def meta(self) -> Optional[dict]:
        return self.config.get("meta")

    @property
    def database(self) -> Optional[str]:
        return self.config.get("database")

    @property
    def schema(self) -> Optional[str]:
        return self.config.get("schema")

    def get_static_config(self):
        config = {}
        if self.alias is not None:
            config["alias"] = self.alias
        if self.severity is not None:
            config["severity"] = self.severity
        if self.enabled is not None:
            config["enabled"] = self.enabled
        if self.where is not None:
            config["where"] = self.where
        if self.limit is not None:
            config["limit"] = self.limit
        if self.warn_if is not None:
            config["warn_if"] = self.warn_if
        if self.error_if is not None:
            config["error_if"] = self.error_if
        if self.fail_calc is not None:
            config["fail_calc"] = self.fail_calc
        if self.store_failures is not None:
            config["store_failures"] = self.store_failures
        if self.store_failures_as is not None:
            config["store_failures_as"] = self.store_failures_as
        if self.sql_header is not None:
            config["sql_header"] = self.sql_header
        if self.meta is not None:
            config["meta"] = self.meta
        if self.database is not None:
            config["database"] = self.database
        if self.schema is not None:
            config["schema"] = self.schema
        return config

    def tags(self) -> List[str]:
        tags = self.config.get("tags", [])
        if isinstance(tags, str):
            tags = [tags]
        if not isinstance(tags, list):
            raise TagsNotListOfStringsError(tags)
        for tag in tags:
            if not isinstance(tag, str):
                raise TagNotStringError(tag)
        return tags[:]

    def macro_name(self) -> str:
        macro_name = "test_{}".format(self.name)
        if self.namespace is not None:
            macro_name = "{}.{}".format(self.namespace, macro_name)
        return macro_name

    def get_synthetic_test_names(self) -> Tuple[str, str]:
        # Returns two names: shorter (for the compiled file), full (for the unique_id + FQN)
        target_name = self.target.name
        if isinstance(self.target, UnparsedModelUpdate):
            name = self.name
            if self.version:
                target_name = f"{self.target.name}_v{self.version}"
        elif isinstance(self.target, UnparsedNodeUpdate):
            name = self.name
        elif isinstance(self.target, UnpatchedSourceDefinition):
            name = "source_" + self.name
        else:
            raise self._bad_type()
        if self.namespace is not None:
            name = "{}_{}".format(self.namespace, name)
        return synthesize_generic_test_names(name, target_name, self.args)

    def construct_config(self) -> str:
        configs = ",".join(
            [
                f"{key}="
                + (
                    ('"' + value.replace('"', '\\"') + '"')
                    if isinstance(value, str)
                    else str(value)
                )
                for key, value in self.config.items()
            ]
        )
        if configs:
            return f"{{{{ config({configs}) }}}}"
        else:
            return ""

    # this is the 'raw_code' that's used in 'render_update' and execution
    # of the test macro
    def build_raw_code(self) -> str:
        return ("{{{{ {macro}(**{kwargs_name}) }}}}{config}").format(
            macro=self.macro_name(),
            config=self.construct_config(),
            kwargs_name=GENERIC_TEST_KWARGS_NAME,
        )

    def build_model_str(self):
        targ = self.target
        if isinstance(self.target, UnparsedModelUpdate):
            if self.version:
                target_str = f"ref('{targ.name}', version='{self.version}')"
            else:
                target_str = f"ref('{targ.name}')"
        elif isinstance(self.target, UnparsedNodeUpdate):
            target_str = f"ref('{targ.name}')"
        elif isinstance(self.target, UnpatchedSourceDefinition):
            target_str = f"source('{targ.source.name}', '{targ.table.name}')"
        return f"{{{{ get_where_subquery({target_str}) }}}}"
