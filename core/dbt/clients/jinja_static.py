from typing import TYPE_CHECKING, Any, Dict, List, Optional, Union

import jinja2

from dbt.artifacts.resources import RefArgs
from dbt.exceptions import MacroNamespaceNotStringError, ParsingError
from dbt_common.clients.jinja import get_environment
from dbt_common.clients.jinja_macro_call import DbtMacroCall
from dbt_common.exceptions.macros import MacroNameNotStringError
from dbt_common.tests import test_caching_enabled
from dbt_extractor import ExtractionError, py_extract_from_source  # type: ignore

if TYPE_CHECKING:
    from dbt.context.providers import ParseDatabaseWrapper


_TESTING_MACRO_CACHE: Dict[str, Any] = {}


def statically_extract_has_name_this(source: str) -> bool:
    """Checks whether the raw jinja has any references to `this`"""
    env = get_environment(None, capture_macros=True)
    parsed = env.parse(source)
    names = tuple(parsed.find_all(jinja2.nodes.Name))

    for name in names:
        if hasattr(name, "name") and name.name == "this":
            return True
    return False


def statically_extract_macro_calls(
    source: str, ctx: Dict[str, Any], db_wrapper: Optional["ParseDatabaseWrapper"] = None
) -> List[DbtMacroCall]:
    # set 'capture_macros' to capture undefined
    env = get_environment(None, capture_macros=True)

    global _TESTING_MACRO_CACHE
    if test_caching_enabled() and source in _TESTING_MACRO_CACHE:
        parsed = _TESTING_MACRO_CACHE.get(source, None)
        func_calls = getattr(parsed, "_dbt_cached_calls")
    else:
        parsed = env.parse(source)
        func_calls = tuple(parsed.find_all(jinja2.nodes.Call))

        if test_caching_enabled():
            _TESTING_MACRO_CACHE[source] = parsed
            setattr(parsed, "_dbt_cached_calls", func_calls)

    standard_calls = ["source", "ref", "config"]
    possible_macro_calls: List[DbtMacroCall] = []
    for func_call in func_calls:
        macro_call: Optional[DbtMacroCall] = None
        if hasattr(func_call, "node") and hasattr(func_call.node, "name"):
            macro_call = DbtMacroCall.from_call(func_call, func_call.node.name)
        else:
            if (
                hasattr(func_call, "node")
                and hasattr(func_call.node, "node")
                and type(func_call.node.node).__name__ == "Name"
                and hasattr(func_call.node, "attr")
            ):
                package_name = func_call.node.node.name
                macro_name = func_call.node.attr
                if package_name == "adapter":
                    if macro_name == "dispatch":
                        ad_macro_calls = statically_parse_adapter_dispatch(
                            func_call, ctx, db_wrapper
                        )
                        possible_macro_calls.extend(ad_macro_calls)
                    else:
                        # This skips calls such as adapter.parse_index
                        continue
                else:
                    macro_call = DbtMacroCall.from_call(func_call, f"{package_name}.{macro_name}")
            else:
                continue

        if not macro_call or macro_call.name in standard_calls or ctx.get(macro_call.name):
            continue

        possible_macro_calls.append(macro_call)

    return possible_macro_calls


def statically_parse_adapter_dispatch(
    func_call, ctx: Dict[str, Any], db_wrapper: Optional["ParseDatabaseWrapper"]
) -> List[DbtMacroCall]:
    possible_macro_calls: List[DbtMacroCall] = []
    # This captures an adapter.dispatch('<macro_name>') call.

    func_name = None
    # macro_name positional argument
    if len(func_call.args) > 0:
        func_name = func_call.args[0].value

    if func_name:
        possible_macro_calls.append(DbtMacroCall.from_call(func_call, func_name))

    # packages positional argument
    macro_namespace = None
    packages_arg = None
    packages_arg_type = None

    if len(func_call.args) > 1:
        packages_arg = func_call.args[1]
        # This can be a List or a Call
        packages_arg_type = type(func_call.args[1]).__name__

    # keyword arguments
    if func_call.kwargs:
        for kwarg in func_call.kwargs:
            if kwarg.key == "macro_name":
                # This will remain to enable static resolution
                if type(kwarg.value).__name__ == "Const":
                    func_name = kwarg.value.value
                    possible_macro_calls.append(DbtMacroCall.from_call(func_call, func_name))
                else:
                    raise MacroNameNotStringError(kwarg_value=kwarg.value.value)
            elif kwarg.key == "macro_namespace":
                # This will remain to enable static resolution
                kwarg_type = type(kwarg.value).__name__
                if kwarg_type == "Const":
                    macro_namespace = kwarg.value.value
                else:
                    raise MacroNamespaceNotStringError(kwarg_type)

    # positional arguments
    if packages_arg:
        if packages_arg_type == "List":
            # This will remain to enable static resolution
            packages = []
            for item in packages_arg.items:
                packages.append(item.value)
        elif packages_arg_type == "Const":
            # This will remain to enable static resolution
            macro_namespace = packages_arg.value

    if db_wrapper:
        macro = db_wrapper.dispatch(func_name, macro_namespace=macro_namespace).macro
        func_name = f"{macro.package_name}.{macro.name}"  # type: ignore[attr-defined]
        possible_macro_calls.append(DbtMacroCall.from_call(func_call, func_name))
    else:  # this is only for tests/unit/test_macro_calls.py
        if macro_namespace:
            packages = [macro_namespace]
        else:
            packages = []
        for package_name in packages:
            possible_macro_calls.append(
                DbtMacroCall.from_call(func_call, f"{package_name}.{func_name}")
            )

    return possible_macro_calls


def statically_parse_ref_or_source(expression: str) -> Union[RefArgs, List[str]]:
    """
    Returns a RefArgs or List[str] object, corresponding to ref or source respectively, given an input jinja expression.

    input: str representing how input node is referenced in tested model sql
        * examples:
        - "ref('my_model_a')"
        - "ref('my_model_a', version=3)"
        - "ref('package', 'my_model_a', version=3)"
        - "source('my_source_schema', 'my_source_name')"

    If input is not a well-formed jinja ref or source expression, a ParsingError is raised.
    """
    ref_or_source: Union[RefArgs, List[str]]

    try:
        statically_parsed = py_extract_from_source(f"{{{{ {expression} }}}}")
    except ExtractionError:
        raise ParsingError(f"Invalid jinja expression: {expression}")

    if statically_parsed.get("refs"):
        raw_ref = list(statically_parsed["refs"])[0]
        ref_or_source = RefArgs(
            package=raw_ref.get("package"),
            name=raw_ref.get("name"),
            version=raw_ref.get("version"),
        )
    elif statically_parsed.get("sources"):
        source_name, source_table_name = list(statically_parsed["sources"])[0]
        ref_or_source = [source_name, source_table_name]
    else:
        raise ParsingError(f"Invalid ref or source expression: {expression}")

    return ref_or_source


def statically_parse_unrendered_config(string: str) -> Optional[Dict[str, Any]]:
    """
    Given a string with jinja, extract an unrendered config call.
    If no config call is present, returns None.

    For example, given:
    "{{ config(materialized=env_var('DBT_TEST_STATE_MODIFIED')) }}\nselect 1 as id"
    returns: {'materialized': "Keyword(key='materialized', value=Call(node=Name(name='env_var', ctx='load'), args=[Const(value='DBT_TEST_STATE_MODIFIED')], kwargs=[], dyn_args=None, dyn_kwargs=None))"}

    No config call:
    "select 1 as id"
    returns: None
    """
    # Return early to avoid creating jinja environemt if no config call in input string
    if "config(" not in string:
        return None

    # set 'capture_macros' to capture undefined
    env = get_environment(None, capture_macros=True)

    global _TESTING_MACRO_CACHE
    if test_caching_enabled() and _TESTING_MACRO_CACHE and string in _TESTING_MACRO_CACHE:
        parsed = _TESTING_MACRO_CACHE.get(string, None)
        func_calls = getattr(parsed, "_dbt_cached_calls")
    else:
        parsed = env.parse(string)
        func_calls = tuple(parsed.find_all(jinja2.nodes.Call))

    config_func_calls = list(
        filter(
            lambda f: hasattr(f, "node") and hasattr(f.node, "name") and f.node.name == "config",
            func_calls,
        )
    )
    # There should only be one {{ config(...) }} call per input
    config_func_call = config_func_calls[0] if config_func_calls else None

    if not config_func_call:
        return None

    unrendered_config = {}
    for kwarg in config_func_call.kwargs:
        unrendered_config[kwarg.key] = construct_static_kwarg_value(kwarg)

    return unrendered_config


def construct_static_kwarg_value(kwarg) -> str:
    # Instead of trying to re-assemble complex kwarg value, simply stringify the value.
    # This is still useful to be able to detect changes in unrendered configs, even if it is
    # not an exact representation of the user input.
    return str(kwarg)
