from pathlib import PurePath
from typing import (
    Any,
    Callable,
    Literal,
    TypeVar,
    Protocol,
    overload,
)
from typing_extensions import Final, TypeAlias, Self
from minijinja._lowlevel import State
from collections.abc import Mapping

__all__ = [
    "Environment",
    "TemplateError",
    "safe",
    "escape",
    "render_str",
    "eval_expr",
    "pass_state",
]

_A_contra = TypeVar("_A_contra", contravariant=True)
_R_co = TypeVar("_R_co", covariant=True)

class _PassesState(Protocol[_A_contra, _R_co]):
    def __call__(self, state: State, value: _A_contra, /) -> _R_co: ...

    __minijinja_pass_state__: Literal[True]

_StrPath: TypeAlias = PurePath | str
_Behavior = Literal["strict", "lenient", "chainable"]

DEFAULT_ENVIRONMENT: Final[Environment]

def render_str(source: str, name: str | None = None, /, **context: Any) -> str: ...
def eval_expr(expression: str, /, **context: Any) -> Any: ...

class Environment:
    loader: Callable[[str], str] | None
    fuel: int | None
    debug: bool
    undefined_behavior: _Behavior
    auto_escape_callback: Callable[[str], bool] | None
    path_join_callback: Callable[[str, str], _StrPath] | None
    keep_trailing_newline: bool
    trim_blocks: bool
    lstrip_blocks: bool
    finalizer: _PassesState[Any, Any] | Callable[[Any], Any] | None
    reload_before_render: bool
    block_start_string: str
    block_end_string: str
    variable_start_string: str
    variable_end_string: str
    comment_start_string: str
    comment_end_string: str
    line_statement_prefix: str | None
    line_comment_prefix: str | None

    @overload
    def __init__(
        self,
        loader: Callable[[str], str] | None = None,
        templates: Mapping[str, str] | None = None,
        filters: Mapping[str, Callable[[Any], Any]] | None = None,
        tests: Mapping[str, Callable[[Any], bool]] | None = None,
        globals: Mapping[str, Any] | None = None,
        debug: bool = True,
        fuel: int | None = None,
        undefined_behavior: _Behavior = "lenient",
        auto_escape_callback: Callable[[str], bool] | None = None,
        path_join_callback: Callable[[str, str], _StrPath] | None = None,
        keep_trailing_newline: bool = False,
        trim_blocks: bool = False,
        lstrip_blocks: bool = False,
        finalizer: _PassesState[Any, Any] | None = None,
        reload_before_render: bool = False,
        block_start_string: str = "{%",
        block_end_string: str = "%}",
        variable_start_string: str = "{{",
        variable_end_string: str = "}}",
        comment_start_string: str = "{#",
        comment_end_string: str = "#}",
        line_statement_prefix: str | None = None,
        line_comment_prefix: str | None = None,
    ) -> None: ...
    @overload
    def __init__(
        self,
        loader: Callable[[str], str] | None = None,
        templates: Mapping[str, str] | None = None,
        filters: Mapping[str, Callable[[Any], Any]] | None = None,
        tests: Mapping[str, Callable[[Any], bool]] | None = None,
        globals: Mapping[str, Any] | None = None,
        debug: bool = True,
        fuel: int | None = None,
        undefined_behavior: _Behavior = "lenient",
        auto_escape_callback: Callable[[str], bool] | None = None,
        path_join_callback: Callable[[str, str], _StrPath] | None = None,
        keep_trailing_newline: bool = False,
        trim_blocks: bool = False,
        lstrip_blocks: bool = False,
        finalizer: Callable[[Any], Any] | None = None,
        reload_before_render: bool = False,
        block_start_string: str = "{%",
        block_end_string: str = "%}",
        variable_start_string: str = "{{",
        variable_end_string: str = "}}",
        comment_start_string: str = "{#",
        comment_end_string: str = "#}",
        line_statement_prefix: str | None = None,
        line_comment_prefix: str | None = None,
    ) -> None: ...
    def remove_filter(self, name: str) -> None: ...
    def add_test(self, name: str, test: Callable[[Any], bool]) -> None: ...
    def remove_test(self, name: str) -> None: ...
    def add_global(self, name: str, value: Any) -> None: ...
    def remove_global(self, name: str) -> None: ...
    def render_template(self, template_name: str, /, **context: Any) -> str: ...
    def render_str(
        self, source: str, name: str | None = None, /, **context: Any
    ) -> str: ...
    def eval_expr(self, expression: str, /, **context: Any) -> Any: ...

class TemplateError(RuntimeError):
    def __init__(self, message: str) -> None: ...
    @property
    def message(self) -> str: ...
    @property
    def kind(self) -> str: ...
    @property
    def name(self) -> str | None: ...
    @property
    def detail(self) -> str | None: ...
    @property
    def line(self) -> int | None: ...
    @property
    def range(self) -> int | None: ...
    @property
    def template_source(self) -> str | None: ...
    def __str__(self) -> str: ...

class Markup(str):
    def __html__(self) -> Self: ...

def safe(value: str) -> str: ...
def escape(value: Any) -> str: ...
def pass_state(
    f: Callable[[State, _A_contra], _R_co],
) -> _PassesState[_A_contra, _R_co]: ...
