# New for Python models :p
import ast
import random
from copy import deepcopy
from functools import reduce
from itertools import chain
from typing import Any, Dict, Iterator, List, Optional, Tuple, Union

import dbt.tracking as tracking
from dbt import utils
from dbt.artifacts.resources import RefArgs
from dbt.clients.jinja import get_rendered
from dbt.context.context_config import ContextConfig
from dbt.contracts.graph.nodes import ModelNode
from dbt.exceptions import (
    ModelConfigError,
    ParsingError,
    PythonLiteralEvalError,
    PythonParsingError,
)
from dbt.flags import get_flags
from dbt.node_types import ModelLanguage, NodeType
from dbt.parser.base import SimpleSQLParser
from dbt.parser.search import FileBlock
from dbt_common.contracts.config.base import merge_config_dicts
from dbt_common.dataclass_schema import ValidationError
from dbt_common.exceptions.macros import UndefinedMacroError
from dbt_extractor import ExtractionError, py_extract_from_source  # type: ignore

dbt_function_key_words = set(["ref", "source", "config", "get", "var"])
dbt_function_full_names = set(
    ["dbt.ref", "dbt.source", "dbt.config", "dbt.config.get", "dbt.var", "dbt.var.get"]
)


class PythonValidationVisitor(ast.NodeVisitor):
    def __init__(self) -> None:
        super().__init__()
        self.dbt_errors: List[str] = []
        self.num_model_def = 0

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        if node.name == "model":
            self.num_model_def += 1
            if node.args.args and not node.args.args[0].arg == "dbt":
                self.dbt_errors.append("'dbt' not provided for model as the first argument")
            if len(node.args.args) != 2:
                self.dbt_errors.append(
                    "model function should have two args, `dbt` and a session to current warehouse"
                )
            # check we have a return and only one
            if not isinstance(node.body[-1], ast.Return) or isinstance(
                node.body[-1].value, ast.Tuple
            ):
                self.dbt_errors.append(
                    "In current version, model function should return only one dataframe object"
                )

    def check_error(self, node):
        if self.num_model_def != 1:
            raise ParsingError(
                f"dbt allows exactly one model defined per python file, found {self.num_model_def}",
                node=node,
            )

        if len(self.dbt_errors) != 0:
            raise ParsingError("\n".join(self.dbt_errors), node=node)


class PythonParseVisitor(ast.NodeVisitor):
    def __init__(self, dbt_node):
        super().__init__()

        self.dbt_node = dbt_node
        self.dbt_function_calls = []
        self.packages = []

    @classmethod
    def _flatten_attr(cls, node):
        if isinstance(node, ast.Attribute):
            return str(cls._flatten_attr(node.value)) + "." + node.attr
        elif isinstance(node, ast.Name):
            return str(node.id)
        else:
            pass

    def _safe_eval(self, node):
        try:
            return ast.literal_eval(node)
        except (SyntaxError, ValueError, TypeError, MemoryError, RecursionError) as exc:
            raise PythonLiteralEvalError(exc, node=self.dbt_node) from exc

    def _get_call_literals(self, node):
        # List of literals
        arg_literals = []
        kwarg_literals = {}

        # TODO : Make sure this throws (and that we catch it)
        # for non-literal inputs
        for arg in node.args:
            rendered = self._safe_eval(arg)
            arg_literals.append(rendered)

        for keyword in node.keywords:
            key = keyword.arg
            rendered = self._safe_eval(keyword.value)
            kwarg_literals[key] = rendered

        return arg_literals, kwarg_literals

    def visit_Call(self, node: ast.Call) -> None:
        # check weather the current call could be a dbt function call
        if isinstance(node.func, ast.Attribute) and node.func.attr in dbt_function_key_words:
            func_name = self._flatten_attr(node.func)
            # check weather the current call really is a dbt function call
            if func_name in dbt_function_full_names:
                # drop the dot-dbt prefix
                func_name = func_name.split(".")[-1]
                args, kwargs = self._get_call_literals(node)
                value = node.func.value
                # dbt_ctx_obj will be used to get which obj uses the get method: config, var
                dbt_ctx_obj = value.attr if hasattr(value, "attr") else None
                self.dbt_function_calls.append((func_name, args, kwargs, dbt_ctx_obj))

        # no matter what happened above, we should keep visiting the rest of the tree
        # visit args and kwargs to see if there's call in it
        for obj in node.args + [kwarg.value for kwarg in node.keywords]:
            if isinstance(obj, ast.Call):
                self.visit_Call(obj)
            # support dbt.ref in list args, kwargs
            elif isinstance(obj, ast.List) or isinstance(obj, ast.Tuple):
                for el in obj.elts:
                    if isinstance(el, ast.Call):
                        self.visit_Call(el)
            # support dbt.ref in dict args, kwargs
            elif isinstance(obj, ast.Dict):
                for value in obj.values:
                    if isinstance(value, ast.Call):
                        self.visit_Call(value)
            # support dbt function calls in f-strings
            elif isinstance(obj, ast.JoinedStr):
                for value in obj.values:
                    if isinstance(value, ast.FormattedValue) and isinstance(value.value, ast.Call):
                        self.visit_Call(value.value)

        # visit node.func.value if we are at an call attr
        if isinstance(node.func, ast.Attribute):
            self.attribute_helper(node.func)

    def attribute_helper(self, node: ast.Attribute) -> None:
        while isinstance(node, ast.Attribute):
            node = node.value  # type: ignore
        if isinstance(node, ast.Call):
            self.visit_Call(node)

    def visit_Import(self, node: ast.Import) -> None:
        for n in node.names:
            self.packages.append(n.name.split(".")[0])

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        if node.module:
            self.packages.append(node.module.split(".")[0])


def verify_python_model_code(node):
    # TODO: add a test for this
    try:
        rendered_python = get_rendered(
            node.raw_code,
            {},
            node,
        )
        if rendered_python != node.raw_code:
            raise ParsingError("")
    except (UndefinedMacroError, ParsingError):
        raise ParsingError("No jinja in python model code is allowed", node=node)


class ModelParser(SimpleSQLParser[ModelNode]):
    def parse_from_dict(self, dct, validate=True) -> ModelNode:
        if validate:
            ModelNode.validate(dct)
        return ModelNode.from_dict(dct)

    @property
    def resource_type(self) -> NodeType:
        return NodeType.Model

    @classmethod
    def get_compiled_path(cls, block: FileBlock):
        return block.path.relative_path

    def parse_python_model(self, node, config, context):
        config_keys_used = []
        config_keys_defaults = []
        var_keys_used = []
        var_keys_defaults = []
        try:
            tree = ast.parse(node.raw_code, filename=node.original_file_path)
        except SyntaxError as exc:
            raise PythonParsingError(exc, node=node) from exc

        # Only parse if AST tree has instructions in body
        if tree.body:
            # We are doing a validator and a parser because visit_FunctionDef in parser
            # would actually make the parser not doing the visit_Calls any more
            dbt_validator = PythonValidationVisitor()
            dbt_validator.visit(tree)
            dbt_validator.check_error(node)

            dbt_parser = PythonParseVisitor(node)
            dbt_parser.visit(tree)
            for func, args, kwargs, dbt_ctx_obj in dbt_parser.dbt_function_calls:
                if func == "get":
                    num_args = len(args)
                    if num_args == 0:
                        raise ParsingError(
                            "dbt.config.get() requires at least one argument",
                            node=node,
                        )
                    if num_args > 2:
                        raise ParsingError(
                            f"dbt.config.get() takes at most 2 arguments ({num_args} given)",
                            node=node,
                        )
                    key = args[0]
                    default_value = args[1] if num_args == 2 else None
                    if dbt_ctx_obj == "config":
                        config_keys_used.append(key)
                        config_keys_defaults.append(default_value)
                    elif dbt_ctx_obj == "var":
                        var_keys_used.append(key)
                        var_keys_defaults.append(default_value)
                    continue
                context[func](*args, **kwargs)
        kwargs_config = {}
        # this is being used in macro build_config_dict
        if config_keys_used:
            kwargs_config["config_keys_used"] = config_keys_used
            kwargs_config["config_keys_defaults"] = (config_keys_defaults,)
        if var_keys_used:
            kwargs_config["var_keys_used"] = var_keys_used
            kwargs_config["var_keys_defaults"] = var_keys_defaults
        if kwargs_config:
            context["config"](**kwargs_config)

    def render_update(self, node: ModelNode, config: ContextConfig) -> None:
        self.manifest._parsing_info.static_analysis_path_count += 1
        flags = get_flags()
        if node.language == ModelLanguage.python:
            try:
                verify_python_model_code(node)
                context = self._context_for(node, config)
                self.parse_python_model(node, config, context)
                self.update_parsed_node_config(node, config, context=context)

            except ValidationError as exc:
                # we got a ValidationError - probably bad types in config()
                raise ModelConfigError(exc, node=node) from exc
            return

        elif not flags.STATIC_PARSER:
            # jinja rendering
            super().render_update(node, config)
            return

        # only sample for experimental parser correctness on normal runs,
        # not when the experimental parser flag is on.
        exp_sample: bool = False
        # sampling the stable static parser against jinja is significantly
        # more expensive and therefore done far less frequently.
        stable_sample: bool = False
        # there are two samples above, and it is perfectly fine if both happen
        # at the same time. If that happens, the experimental parser, stable
        # parser, and jinja rendering will run on the same model file and
        # send back codes for experimental v stable, and stable v jinja.
        if not flags.USE_EXPERIMENTAL_PARSER:
            # `True` roughly 1/5000 times this function is called
            # sample = random.randint(1, 5001) == 5000
            stable_sample = random.randint(1, 5001) == 5000
            # sampling the experimental parser is explicitly disabled here, but use the following
            # commented code to sample a fraction of the time when new
            # experimental features are added.
            # `True` roughly 1/100 times this function is called
            # exp_sample = random.randint(1, 101) == 100

        # top-level declaration of variables
        statically_parsed: Optional[Union[str, Dict[str, List[Any]]]] = None
        experimental_sample: Optional[Union[str, Dict[str, List[Any]]]] = None
        exp_sample_node: Optional[ModelNode] = None
        exp_sample_config: Optional[ContextConfig] = None
        jinja_sample_node: Optional[ModelNode] = None
        jinja_sample_config: Optional[ContextConfig] = None
        result: List[str] = []

        # sample the experimental parser only during a normal run
        if exp_sample and not flags.USE_EXPERIMENTAL_PARSER:
            experimental_sample = self.run_experimental_parser(node)
            # if the experimental parser succeeded, make a full copy of model parser
            # and populate _everything_ into it so it can be compared apples-to-apples
            # with a fully jinja-rendered project. This is necessary because the experimental
            # parser will likely add features that the existing static parser will fail on
            # so comparing those directly would give us bad results. The comparison will be
            # conducted after this model has been fully rendered either by the static parser
            # or by full jinja rendering
            if isinstance(experimental_sample, dict):
                model_parser_copy = self.partial_deepcopy()
                exp_sample_node = deepcopy(node)
                exp_sample_config = deepcopy(config)
                model_parser_copy.populate(exp_sample_node, exp_sample_config, experimental_sample)
        # use the experimental parser exclusively if the flag is on
        if flags.USE_EXPERIMENTAL_PARSER:
            statically_parsed = self.run_experimental_parser(node)
        # run the stable static parser unless it is explicitly turned off
        else:
            statically_parsed = self.run_static_parser(node)

        # if the static parser succeeded, extract some data in easy-to-compare formats
        if isinstance(statically_parsed, dict):
            # only sample jinja for the purpose of comparing with the stable static parser
            # if we know we don't need to fall back to jinja (i.e. - nothing to compare
            # with jinja v jinja).
            # This means we skip sampling for 40% of the 1/5000 samples. We could run the
            # sampling rng here, but the effect would be the same since we would only roll
            # it 40% of the time. So I've opted to keep all the rng code colocated above.
            if stable_sample and not flags.USE_EXPERIMENTAL_PARSER:
                # if this will _never_ mutate anything `self` we could avoid these deep copies,
                # but we can't really guarantee that going forward.
                model_parser_copy = self.partial_deepcopy()
                jinja_sample_node = deepcopy(node)
                jinja_sample_config = deepcopy(config)
                # rendering mutates the node and the config
                super(ModelParser, model_parser_copy).render_update(
                    jinja_sample_node, jinja_sample_config
                )

            # update the unrendered config with values from the static parser.
            # values from yaml files are in there already
            self.populate(node, config, statically_parsed)

            # if we took a jinja sample, compare now that the base node has been populated
            if jinja_sample_node is not None and jinja_sample_config is not None:
                result = _get_stable_sample_result(
                    jinja_sample_node, jinja_sample_config, node, config
                )

            # if we took an experimental sample, compare now that the base node has been populated
            if exp_sample_node is not None and exp_sample_config is not None:
                result = _get_exp_sample_result(
                    exp_sample_node,
                    exp_sample_config,
                    node,
                    config,
                )

            self.manifest._parsing_info.static_analysis_parsed_path_count += 1
        # if the static parser didn't succeed, fall back to jinja
        else:
            # jinja rendering
            super().render_update(node, config)

            # if sampling, add the correct messages for tracking
            if exp_sample and isinstance(experimental_sample, str):
                if experimental_sample == "cannot_parse":
                    result += ["01_experimental_parser_cannot_parse"]
                elif experimental_sample == "has_banned_macro":
                    result += ["08_has_banned_macro"]
            elif stable_sample and isinstance(statically_parsed, str):
                if statically_parsed == "cannot_parse":
                    result += ["81_stable_parser_cannot_parse"]
                elif statically_parsed == "has_banned_macro":
                    result += ["88_has_banned_macro"]

        # only send the tracking event if there is at least one result code
        if result:
            # fire a tracking event. this fires one event for every sample
            # so that we have data on a per file basis. Not only can we expect
            # no false positives or misses, we can expect the number model
            # files parseable by the experimental parser to match our internal
            # testing.
            if tracking.active_user is not None:  # None in some tests
                tracking.track_experimental_parser_sample(
                    {
                        "project_id": self.root_project.hashed_name(),
                        "file_id": utils.get_hash(node),
                        "status": result,
                    }
                )

    def run_static_parser(self, node: ModelNode) -> Optional[Union[str, Dict[str, List[Any]]]]:
        # if any banned macros have been overridden by the user, we cannot use the static parser.
        if self._has_banned_macro(node):
            return "has_banned_macro"

        # run the stable static parser and return the results
        try:
            statically_parsed = py_extract_from_source(node.raw_code)
            return _shift_sources(statically_parsed)
        # if we want information on what features are barring the static
        # parser from reading model files, this is where we would add that
        # since that information is stored in the `ExtractionError`.
        except ExtractionError:
            return "cannot_parse"

    def run_experimental_parser(
        self, node: ModelNode
    ) -> Optional[Union[str, Dict[str, List[Any]]]]:
        # if any banned macros have been overridden by the user, we cannot use the static parser.
        if self._has_banned_macro(node):
            return "has_banned_macro"

        # run the experimental parser and return the results
        try:
            # for now, this line calls the stable static parser since there are no
            # experimental features. Change `py_extract_from_source` to the new
            # experimental call when we add additional features.
            experimentally_parsed = py_extract_from_source(node.raw_code)
            return _shift_sources(experimentally_parsed)
        # if we want information on what features are barring the experimental
        # parser from reading model files, this is where we would add that
        # since that information is stored in the `ExtractionError`.
        except ExtractionError:
            return "cannot_parse"

    # checks for banned macros
    def _has_banned_macro(self, node: ModelNode) -> bool:
        # first check if there is a banned macro defined in scope for this model file
        root_project_name = self.root_project.project_name
        project_name = node.package_name
        banned_macros = ["ref", "source", "config"]

        all_banned_macro_keys: Iterator[str] = chain.from_iterable(
            map(
                lambda name: [f"macro.{project_name}.{name}", f"macro.{root_project_name}.{name}"],
                banned_macros,
            )
        )

        return reduce(
            lambda z, key: z or (key in self.manifest.macros), all_banned_macro_keys, False
        )

    # this method updates the model node rendered and unrendered config as well
    # as the node object. Used to populate these values when circumventing jinja
    # rendering like the static parser.
    def populate(self, node: ModelNode, config: ContextConfig, statically_parsed: Dict[str, Any]):
        # manually fit configs in
        config._config_call_dict = _get_config_call_dict(statically_parsed)

        # if there are hooks present this, it WILL render jinja. Will need to change
        # when the experimental parser supports hooks
        self.update_parsed_node_config(node, config)

        # update the unrendered config with values from the file.
        # values from yaml files are in there already
        node.unrendered_config.update(dict(statically_parsed["configs"]))

        # set refs and sources on the node object
        refs: List[RefArgs] = []
        for ref in statically_parsed["refs"]:
            name = ref.get("name")
            package = ref.get("package")
            version = ref.get("version")
            refs.append(RefArgs(name, package, version))

        node.refs += refs
        node.sources += statically_parsed["sources"]

        # configs don't need to be merged into the node because they
        # are read from config._config_call_dict

    # the manifest is often huge so this method avoids deepcopying it
    def partial_deepcopy(self):
        return ModelParser(deepcopy(self.project), self.manifest, deepcopy(self.root_project))


# pure function. safe to use elsewhere, but unlikely to be useful outside this file.
def _get_config_call_dict(static_parser_result: Dict[str, Any]) -> Dict[str, Any]:
    config_call_dict: Dict[str, Any] = {}

    for c in static_parser_result["configs"]:
        merge_config_dicts(config_call_dict, {c[0]: c[1]})

    return config_call_dict


# TODO if we format sources in the extractor to match this type, we won't need this function.
def _shift_sources(static_parser_result: Dict[str, List[Any]]) -> Dict[str, List[Any]]:
    shifted_result = deepcopy(static_parser_result)
    source_calls = []

    for s in static_parser_result["sources"]:
        source_calls.append([s[0], s[1]])
    shifted_result["sources"] = source_calls

    return shifted_result


# returns a list of string codes to be sent as a tracking event
def _get_exp_sample_result(
    sample_node: ModelNode,
    sample_config: ContextConfig,
    node: ModelNode,
    config: ContextConfig,
) -> List[str]:
    result: List[Tuple[int, str]] = _get_sample_result(sample_node, sample_config, node, config)

    def process(codemsg):
        code, msg = codemsg
        return f"0{code}_experimental_{msg}"

    return list(map(process, result))


# returns a list of string codes to be sent as a tracking event
def _get_stable_sample_result(
    sample_node: ModelNode,
    sample_config: ContextConfig,
    node: ModelNode,
    config: ContextConfig,
) -> List[str]:
    result: List[Tuple[int, str]] = _get_sample_result(sample_node, sample_config, node, config)

    def process(codemsg):
        code, msg = codemsg
        return f"8{code}_stable_{msg}"

    return list(map(process, result))


# returns a list of string codes that need a single digit prefix to be prepended
# before being sent as a tracking event
def _get_sample_result(
    sample_node: ModelNode,
    sample_config: ContextConfig,
    node: ModelNode,
    config: ContextConfig,
) -> List[Tuple[int, str]]:
    result: List[Tuple[int, str]] = []
    # look for false positive configs
    for k in sample_config._config_call_dict.keys():
        if k not in config._config_call_dict.keys():
            result += [(2, "false_positive_config_value")]
            break

    # look for missed configs
    for k in config._config_call_dict.keys():
        if k not in sample_config._config_call_dict.keys():
            result += [(3, "missed_config_value")]
            break

    # look for false positive sources
    for s in sample_node.sources:
        if s not in node.sources:
            result += [(4, "false_positive_source_value")]
            break

    # look for missed sources
    for s in node.sources:
        if s not in sample_node.sources:
            result += [(5, "missed_source_value")]
            break

    # look for false positive refs
    for r in sample_node.refs:
        if r not in node.refs:
            result += [(6, "false_positive_ref_value")]
            break

    # look for missed refs
    for r in node.refs:
        if r not in sample_node.refs:
            result += [(7, "missed_ref_value")]
            break

    # if there are no errors, return a success value
    if not result:
        result = [(0, "exact_match")]

    return result
