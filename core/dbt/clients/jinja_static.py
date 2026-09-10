import typing
from typing import Any, Callable, Dict, List, Optional, Union

import jinja2

from dbt.artifacts.resources import RefArgs
from dbt.exceptions import MacroNamespaceNotStringError, ParsingError
from dbt_common.clients.jinja import get_environment
from dbt_common.exceptions.macros import MacroNameNotStringError
from dbt_common.tests import test_caching_enabled
from dbt_extractor import ExtractionError, py_extract_from_source  # type: ignore

if typing.TYPE_CHECKING:
    from dbt.context.providers import ParseDatabaseWrapper


_TESTING_MACRO_CACHE: Dict[str, Any] = {}
_COMPARE_OPS = {
    "eq": "==",
    "ne": "!=",
    "lt": "<",
    "lteq": "<=",
    "gt": ">",
    "gteq": ">=",
    "in": "in",
    "notin": "not in",
}


def statically_check_has_jinja(source: Optional[str]) -> bool:
    """Returns True if the source string contains Jinja delimiters (`{{` or `{%`).

    Used as a fast-path check to skip Jinja parsing/compilation for plain SQL.
    Safely handles None.
    """
    if not source:
        return False
    return "{{" in source or "{%" in source


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
) -> List[str]:
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
    possible_macro_calls = []
    for func_call in func_calls:
        func_name = None
        if hasattr(func_call, "node") and hasattr(func_call.node, "name"):
            func_name = func_call.node.name
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
                    func_name = f"{package_name}.{macro_name}"
            else:
                continue
        if not func_name:
            continue
        if func_name in standard_calls:
            continue
        elif ctx.get(func_name):
            continue
        else:
            if func_name not in possible_macro_calls:
                possible_macro_calls.append(func_name)

    return possible_macro_calls


def statically_parse_adapter_dispatch(
    func_call, ctx: Dict[str, Any], db_wrapper: Optional["ParseDatabaseWrapper"]
) -> List[str]:
    possible_macro_calls = []
    # This captures an adapter.dispatch('<macro_name>') call.

    func_name = None
    # macro_name positional argument
    if len(func_call.args) > 0:
        func_name = func_call.args[0].value
    if func_name:
        possible_macro_calls.append(func_name)

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
                    possible_macro_calls.append(func_name)
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
        possible_macro_calls.append(func_name)
    else:  # this is only for tests/unit/test_macro_calls.py
        if macro_namespace:
            packages = [macro_namespace]
        else:
            packages = []
        for package_name in packages:
            possible_macro_calls.append(f"{package_name}.{func_name}")

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


def _try_evaluate_to_python(node: Any) -> Any:
    """Evaluate a Jinja2 AST node to a native Python value when possible.

    Handles literal types (Const, List, Dict, Tuple, Neg) recursively.
    Falls back to a string representation via ``_reconstruct_node`` for complex
    nodes such as Call, Name, Getattr, etc.

    The *node* parameter is typed as ``Any`` because jinja2 AST node classes
    define their fields dynamically, which mypy cannot resolve.
    """
    if isinstance(node, jinja2.nodes.Const):
        return node.value  # type: ignore[attr-defined]
    if isinstance(node, jinja2.nodes.List):
        return [_try_evaluate_to_python(item) for item in node.items]  # type: ignore[attr-defined]
    if isinstance(node, jinja2.nodes.Dict):
        return {
            _try_evaluate_to_python(pair.key): _try_evaluate_to_python(pair.value)
            for pair in node.items  # type: ignore[attr-defined]
        }
    if isinstance(node, jinja2.nodes.Tuple):
        return tuple(_try_evaluate_to_python(item) for item in node.items)  # type: ignore[attr-defined]
    if isinstance(node, jinja2.nodes.Neg):
        inner = _try_evaluate_to_python(node.node)  # type: ignore[attr-defined]
        if isinstance(inner, (int, float)):
            return -inner
        return _reconstruct_node(node)
    return _reconstruct_node(node)


def construct_static_kwarg_value(kwarg) -> Any:
    try:
        kw: Any = kwarg
        return _try_evaluate_to_python(kw.value)
    except Exception:
        # Sensitive codepath — fall back to the original AST repr on any error
        return str(kwarg)


def _reconstruct_call(n: Any) -> str:
    func = _reconstruct_node(n.node)
    parts: List[str] = [_reconstruct_node(a) for a in n.args]
    parts += [f"{kw.key}={_reconstruct_node(kw.value)}" for kw in n.kwargs]
    if n.dyn_args is not None:
        parts.append(f"*{_reconstruct_node(n.dyn_args)}")
    if n.dyn_kwargs is not None:
        parts.append(f"**{_reconstruct_node(n.dyn_kwargs)}")
    return f"{func}({', '.join(parts)})"


def _reconstruct_tuple(n: Any) -> str:
    items = [_reconstruct_node(i) for i in n.items]
    return f"({items[0]},)" if len(items) == 1 else f"({', '.join(items)})"


def _reconstruct_dict(n: Any) -> str:
    pairs = [f"{_reconstruct_node(p.key)}: {_reconstruct_node(p.value)}" for p in n.items]
    return "{" + ", ".join(pairs) + "}"


def _reconstruct_compare(n: Any) -> str:
    expr = _reconstruct_node(n.expr)
    ops = " ".join(
        f"{_COMPARE_OPS.get(op.op, op.op)} {_reconstruct_node(op.expr)}" for op in n.ops
    )
    return f"{expr} {ops}"


# Dispatch table keyed by exact node type. All keys are concrete leaf classes in
# the jinja2 AST, so an exact `type(node)` lookup is sufficient.
_NODE_HANDLERS: Dict[type, Callable[[Any], str]] = {
    jinja2.nodes.Const: lambda n: repr(n.value),
    jinja2.nodes.Name: lambda n: n.name,
    jinja2.nodes.Call: _reconstruct_call,
    jinja2.nodes.List: lambda n: f"[{', '.join(_reconstruct_node(i) for i in n.items)}]",
    jinja2.nodes.Tuple: _reconstruct_tuple,
    jinja2.nodes.Dict: _reconstruct_dict,
    jinja2.nodes.Getattr: lambda n: f"{_reconstruct_node(n.node)}.{n.attr}",
    jinja2.nodes.Getitem: lambda n: f"{_reconstruct_node(n.node)}[{_reconstruct_node(n.arg)}]",
    jinja2.nodes.Concat: lambda n: " ~ ".join(_reconstruct_node(c) for c in n.nodes),
    jinja2.nodes.Compare: _reconstruct_compare,
    jinja2.nodes.Not: lambda n: f"not {_reconstruct_node(n.node)}",
    jinja2.nodes.Neg: lambda n: f"-{_reconstruct_node(n.node)}",
    jinja2.nodes.And: lambda n: f"{_reconstruct_node(n.left)} and {_reconstruct_node(n.right)}",
    jinja2.nodes.Or: lambda n: f"{_reconstruct_node(n.left)} or {_reconstruct_node(n.right)}",
}


def _reconstruct_node(node) -> str:
    """Reconstruct a Jinja2 AST node back to a source-like expression string."""
    # `str` is the fallback for any unhandled node type — preserves existing behaviour.
    handler = _NODE_HANDLERS.get(type(node), str)
    return handler(node)
