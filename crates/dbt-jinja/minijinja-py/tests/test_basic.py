import binascii
import pytest
import posixpath
import types

from _pytest.unraisableexception import catch_unraisable_exception
from minijinja import (
    Environment,
    TemplateError,
    safe,
    pass_state,
    eval_expr,
    render_str,
)


def test_expression():
    env = Environment()
    rv = env.eval_expr("1 + b", b=42)
    assert rv == 43
    rv = env.eval_expr("range(n)", n=10)
    assert rv == list(range(10))


def test_pass_callable():
    def magic():
        return [1, 2, 3]

    env = Environment()
    rv = env.eval_expr("x()", x=magic)
    assert rv == [1, 2, 3]


def test_callable_attrs():
    def hmm():
        pass

    hmm.public_attr = 42
    env = Environment()
    rv = env.eval_expr("[hmm.public_attr, hmm.__module__]", hmm=hmm)
    assert rv == [42, None]


def test_generator():
    def hmm():
        yield 1
        yield 2
        yield 3

    hmm.public_attr = 42
    env = Environment()
    rv = env.eval_expr("values", values=hmm())
    assert isinstance(rv, types.GeneratorType)

    rv = env.eval_expr("values|list", values=hmm())
    assert rv == [1, 2, 3]


def test_method_calling():
    class MyClass(object):
        def my_method(self):
            return 23

        def __repr__(self):
            return "This is X"

    env = Environment()
    rv = env.eval_expr("[x ~ '', x.my_method()]", x=MyClass())
    assert rv == ["This is X", 23]
    rv = env.eval_expr("x.items()|list", x={"a": "b"})
    assert rv == [("a", "b")]


def test_types_passthrough():
    tup = (1, 2, 3)
    assert eval_expr("x", x=tup) == tup
    assert render_str("{{ x }}", x=tup) == "(1, 2, 3)"
    assert eval_expr("x is sequence", x=tup) == True
    assert render_str("{{ x }}", x=(1, True)) == "(1, True)"
    assert eval_expr("x[0] == 42", x=[42]) == True


def test_custom_filter():
    def my_filter(value):
        return "<%s>" % value.upper()

    env = Environment()
    env.add_filter("myfilter", my_filter)
    rv = env.eval_expr("'hello'|myfilter")
    assert rv == "<HELLO>"


def test_custom_filter_kwargs():
    def my_filter(value, x):
        return "<%s %s>" % (value.upper(), x)

    env = Environment()
    env.add_filter("myfilter", my_filter)
    rv = env.eval_expr("'hello'|myfilter(x=42)")
    assert rv == "<HELLO 42>"


def test_custom_test():
    def my_test(value, arg):
        return value == arg

    env = Environment()
    env.add_filter("mytest", my_test)
    rv = env.eval_expr("'hello'|mytest(arg='hello')")
    assert rv == True
    rv = env.eval_expr("'hello'|mytest(arg='hellox')")
    assert rv == False


def test_basic_types():
    env = Environment()
    rv = env.eval_expr("{'a': 42, 'b': 42.5, 'c': 'blah'}")
    assert rv == {"a": 42, "b": 42.5, "c": "blah"}


def test_loader():
    called = []

    def my_loader(name):
        called.append(name)
        return "Hello from " + name

    env = Environment(loader=my_loader)
    assert env.render_template("index.html") == "Hello from index.html"
    assert env.render_template("index.html") == "Hello from index.html"
    assert env.render_template("other.html") == "Hello from other.html"
    assert env.loader is my_loader
    assert called == ["index.html", "other.html"]
    env.loader = my_loader
    assert env.render_template("index.html") == "Hello from index.html"
    assert called == ["index.html", "other.html"]
    env.reload()
    assert env.render_template("index.html") == "Hello from index.html"
    assert called == ["index.html", "other.html", "index.html"]


def test_loader_reload():
    called = []

    def my_loader(name):
        called.append(name)
        return "Hello from " + name

    env = Environment(loader=my_loader)
    env.reload_before_render = True
    assert env.render_template("index.html") == "Hello from index.html"
    assert env.render_template("index.html") == "Hello from index.html"
    assert env.render_template("other.html") == "Hello from other.html"
    assert called == ["index.html", "index.html", "other.html"]


def test_autoescape():
    assert Environment().auto_escape_callback is None

    def auto_escape(name):
        assert name == "foo.html"
        return "html"

    env = Environment(
        auto_escape_callback=auto_escape,
        loader=lambda x: "Hello {{ foo }}",
    )
    assert env.auto_escape_callback is auto_escape

    rv = env.render_template("foo.html", foo="<x>")
    assert rv == "Hello &lt;x&gt;"

    with catch_unraisable_exception() as cm:
        rv = env.render_template("invalid.html", foo="<x>")
        assert rv == "Hello <x>"
        assert cm.unraisable[0] is AssertionError


def test_finalizer():
    assert Environment().finalizer is None

    @pass_state
    def my_finalizer(state, value):
        assert state.name == "<string>"
        if value is None:
            return ""
        elif isinstance(value, bytes):
            return binascii.b2a_hex(value).decode("utf-8")
        return NotImplemented

    env = Environment(finalizer=my_finalizer)

    rv = env.render_str("[{{ foo }}]")
    assert rv == "[]"
    rv = env.render_str("[{{ foo }}]", foo=None)
    assert rv == "[]"
    rv = env.render_str("[{{ foo }}]", foo="test")
    assert rv == "[test]"
    rv = env.render_str("[{{ foo }}]", foo=b"test")
    assert rv == "[74657374]"

    def raising_finalizer(value):
        1 / 0

    env = Environment(finalizer=raising_finalizer)

    with pytest.raises(ZeroDivisionError):
        env.render_str("{{ whatever }}")


def test_globals():
    env = Environment(globals={"x": 23, "y": lambda: 42})
    rv = env.eval_expr("[x, y(), z]", z=11)
    assert rv == [23, 42, 11]


def test_honor_safe():
    env = Environment(auto_escape_callback=lambda x: True)
    rv = env.render_str("{{ x }} {{ y }}", x=safe("<foo>"), y="<bar>")
    assert rv == "<foo> &lt;bar&gt;"


def test_full_object_transfer():
    class X(object):
        def __init__(self):
            self.x = 1
            self.y = 2

    def test_filter(value):
        assert isinstance(value, X)
        return value

    env = Environment(filters=dict(testfilter=test_filter))
    rv = env.eval_expr("x|testfilter", x=X())
    assert isinstance(rv, X)
    assert rv.x == 1
    assert rv.y == 2


def test_markup_transfer():
    env = Environment()
    rv = env.eval_expr("value", value=safe("<foo>"))
    assert hasattr(rv, "__html__")
    assert rv.__html__() == "<foo>"

    rv = env.eval_expr("'<test>'|escape")
    assert hasattr(rv, "__html__")
    assert rv.__html__() == "&lt;test&gt;"


def test_error():
    env = Environment()
    try:
        env.eval_expr("1 +")
    except TemplateError as e:
        assert e.name == "<expression>"
        assert "unexpected end of input" in e.message
        assert "1 > 1 +" not in e.message
        assert "1 > 1 +" in str(e)
        assert e.line == 1
        assert e.kind == "SyntaxError"
        assert e.range == (2, 3)
        assert e.template_source == "1 +"
        assert "unexpected end of input" in e.detail
    else:
        assert False, "expected error"


def test_custom_syntax():
    env = Environment(
        block_start_string="[%",
        block_end_string="%]",
        variable_start_string="{",
        variable_end_string="}",
        comment_start_string="/*",
        comment_end_string="*/",
    )
    rv = env.render_str("[% if true %]{value}[% endif %]/* nothing */", value=42)
    assert rv == "42"


def test_path_join():
    def join_path(name, parent):
        return posixpath.join(posixpath.dirname(parent), name)

    env = Environment(
        path_join_callback=join_path,
        templates={
            "foo/bar.txt": "{% include 'baz.txt' %}",
            "foo/baz.txt": "I am baz!",
        },
    )

    with catch_unraisable_exception() as cm:
        rv = env.render_template("foo/bar.txt")
        assert rv == "I am baz!"
        assert cm.unraisable is None


def test_keep_trailing_newline():
    env = Environment(keep_trailing_newline=False)
    assert env.render_str("foo\n") == "foo"
    env = Environment(keep_trailing_newline=True)
    assert env.render_str("foo\n") == "foo\n"


def test_trim_blocks():
    env = Environment(trim_blocks=False)
    assert env.render_str("{% if true %}\nfoo{% endif %}") == "\nfoo"
    env = Environment(trim_blocks=True)
    assert env.render_str("{% if true %}\nfoo{% endif %}") == "foo"


def test_lstrip_blocks():
    env = Environment(lstrip_blocks=False)
    assert env.render_str("  {% if true %}\nfoo{% endif %}") == "  \nfoo"
    env = Environment(lstrip_blocks=True)
    assert env.render_str("  {% if true %}\nfoo{% endif %}") == "\nfoo"


def test_trim_and_lstrip_blocks():
    env = Environment(lstrip_blocks=False, trim_blocks=False)
    assert env.render_str("  {% if true %}\nfoo{% endif %}") == "  \nfoo"
    env = Environment(lstrip_blocks=True, trim_blocks=True)
    assert env.render_str("  {% if true %}\nfoo{% endif %}") == "foo"


def test_line_statements():
    env = Environment()
    assert env.line_statement_prefix is None
    assert env.line_comment_prefix is None

    env = Environment(line_statement_prefix="#", line_comment_prefix="##")
    assert env.line_statement_prefix == "#"
    assert env.line_comment_prefix == "##"

    rv = env.render_str("# for x in range(3)\n{{ x }}\n# endfor")
    assert rv == "0\n1\n2\n"


def test_custom_delimiters():
    env = Environment(
        variable_start_string="${",
        variable_end_string="}",
        block_start_string="<%",
        block_end_string="%>",
        comment_start_string="<!--",
        comment_end_string="-->",
    )
    rv = env.render_str("<% if true %>${ value }<% endif %><!-- nothing -->", value=42)
    assert rv == "42"
