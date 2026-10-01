<div align="center">
  <img src="https://github.com/mitsuhiko/minijinja/raw/main/artwork/logo.png" alt="" width=320>
  <p><strong>MiniJinja for Python: a powerful template engine for Rust and Python</strong></p>

[![Build Status](https://github.com/mitsuhiko/minijinja/workflows/Tests/badge.svg?branch=main)](https://github.com/mitsuhiko/minijinja/actions?query=workflow%3ATests)
[![License](https://img.shields.io/github/license/mitsuhiko/minijinja)](https://github.com/mitsuhiko/minijinja/blob/main/LICENSE)
[![Crates.io](https://img.shields.io/crates/d/minijinja.svg)](https://crates.io/crates/minijinja)
[![rustc 1.63.0](https://img.shields.io/badge/rust-1.63%2B-orange.svg)](https://img.shields.io/badge/rust-1.63%2B-orange.svg)
[![Documentation](https://docs.rs/minijinja/badge.svg)](https://docs.rs/minijinja)

</div>

`minijinja-py` is an experimental binding of
[MiniJinja](https://github.com/mitsuhiko/minijinja) to Python.  It has somewhat
limited functionality compared to the Rust version.  These bindings use
[maturin](https://www.maturin.rs/) and [pyo3](https://pyo3.rs/).

You might want to use MiniJinja instead of Jinja2 when the full feature set
of Jinja2 is not required and you want to have the same rendering experience
of a data set between Rust and Python.

With these bindings MiniJinja can render some Python objects and values
that are passed to templates, but there are clear limitations with regards
to what can be done.

To install MiniJinja for Python you can fetch the package [from PyPI](https://pypi.org/project/minijinja/):

```
$ pip install minijinja
```

## Basic API

The basic API is hidden behind the `Environment` object.  It behaves almost entirely
like in `minijinja` with some Python specific changes.  For instance instead of
`env.set_debug(True)` you use `env.debug = True`.  Additionally instead of using
`add_template` or attaching a `source` you either pass a dictionary of templates
directly to the environment or a `loader` function.

```python
from minijinja import Environment

env = Environment(templates={
    "template_name": "Template source"
})
```

To render a template you can use the `render_template` method:

```python
result = env.render_template('template_name', var1="value 1", var2="value 2")
print(result)
```

## Purpose

MiniJinja attempts a reasonably high level of compatibility with Jinja2, but it
does not try to achieve this at all costs.  As a result you will notice that
quite a few templates will refuse to render with MiniJinja despite the fact that
they probably look quite innocent.  It is however possible to write templates
that render to the same results for both Jinja2 and MiniJinja.  This raises the
question why you might want to use MiniJinja.

The main benefit would be to achieve the exact same results in both Rust and Python.
Additionally MiniJinja has a stronger sandbox than Jinja2 and might perform ever so
slightly better in some situations.  However you should be aware that due to the
marshalling that needs to happen in either direction there is a certain amount of
loss of information.

## Dynamic Template Loading

MiniJinja's Python bindings inherit the underlying behavior of how MiniJinja loads
templates.  Templates are loaded on first use and then cached.  The templates are
loaded via a loader.  To trigger a reload you can call `env.reload()` or
alternatively set `env.reload_before_render` to `True`.

```python
def my_loader(name):
    segments = []
    for segment in name.split("/"):
        if "\\" in segment or segment in (".", ".."):
            return None
        segments.append(segment)
    try:
        with open(os.path.join(TEMPLATES, *segments)) as f:
            return f.read()
    except (IOError, OSError):
        pass

env = Environment(loader=my_loader)
env.reload_before_render = True
print(env.render_template("index.html"))
```

Alternatively templates can manually be loaded and unloaded with `env.add_template`
and `env.remove_template`.

## Auto Escaping

The default behavior is to use auto escaping file files ending in `.html`.  You can
customize this behavior by overriding the `auto_escape_callback`:

```python
env = Environment(auto_escape_callback=lambda x: x.endswith((".html", ".foo")))
```

MiniJinja uses [markupsafe](https://github.com/pallets/markupsafe) if it's available
on the Python side.  It will honor `__html__`.

## Finalizers

Instead of custom formatters like in MiniJinja, you can define a finalizer instead
which is similar to how it works in Jinja2.  It's passed a value (or optional also
the state as first argument when `pass_state` is used) and can return a new value.
If the special `NotImplemented` value is returned, the original value is rendered
without any modification:

```
from minijinja import Environment

def finalizer(value):
    if value is None:
	return ""
    return NotImplemented

env = Environment(finalizer=finalizer)
assert env.render_str("{{ none }}") == ""
```

## State Access

Functions passed to the environment such as filters or global functions can
optionally have the template state passed by using the `pass_state` parameter.
This is similar to `pass_context` in Jinja2.  It can be used to look at the
name of the template or to look up variables in the context.

```python
from minijinja import pass_state

@pass_state
def my_filter(state, value):
    return state.lookup("a_variable") + value

env.add_filter("add_a_variable", my_filter)
```

## Runtime Behavior

MiniJinja uses it's own runtime model which is not matching the Python runtime
model.  As a result there are gaps in behavior between the two but some
limited effort is made to bridge them.  For instance you will be able to call
some methods of types, but for instance builtins such as dicts and lists do not
expose their methods on the MiniJinja side in all cases.  A natively generated
MiniJinja map (such as with the `dict` global function) will not have an `.items()`
method, whereas a Python dict passed to MiniJinja will.

Here is what this means for some basic types:

* Python dictionaries and lists (as well as other objects that behave as sequences)
  appear in the MiniJinja side very similar to how they do in Python.
* Tuples on the MiniJinja side are represented as lists, but will appear again as
  tuples if passed back to Python.
* Python objects are represented in MiniJinja similarly to dicts, but they retain all
  their meaningful Python APIs.  This means they stringify via `__str__` and they
  allow the MiniJinja code to call their non-underscored methods.  Note that there is
  no extra security layer in use at the moment so take care of what you pass there.
* MiniJinja's python binding understand what `__html__` is when it exists on a string
  subclass.  This means that a `markupsafe.Markup` object will appear as safe string in
  MiniJinja.  This information can also flow back to Python again.
* Stringification of objects uses `__str__` which is why mixed Python and MiniJinja
  objects can be a bit confusing at times.
* Where in Jinja2 there is a difference between `foo["bar"]` and `foo.bar` which can
  be used to disambiugate properties and keys, in MiniJinja there is no such difference.
  However methods are disambiugated so `foo.items()` works and will correctly call
  the method in all cases.

## Sponsor

If you like the project and find it useful you can [become a
sponsor](https://github.com/sponsors/mitsuhiko).

## License and Links

- [Documentation](https://docs.rs/minijinja/)
- [Examples](https://github.com/mitsuhiko/minijinja/tree/main/examples)
- [Issue Tracker](https://github.com/mitsuhiko/minijinja/issues)
- [MiniJinja Playground](https://mitsuhiko.github.io/minijinja-playground/)
- License: [Apache-2.0](https://github.com/mitsuhiko/minijinja/blob/main/LICENSE)
