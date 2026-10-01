# Jinja2 Compatibility

This document tracks the differences between Jinja2 and MiniJinja and what
the state of compatibility and future direction is.

## Syntax Differences

MiniJinja does not support line statements and custom delimiters are an
optional feature that is largely discouraged.  For custom delimiters the
`custom_syntax` feature needs to be enabled.

MiniJinja by default does not allow unicode identifiers.  These need to be
turned on with the `unicode` feature to achieve parity with Jinja2.

## Runtime Differences

The biggest differences between MiniJinja and Jinja2 stem from the different
runtime environments.  Jinja2 leaks out a lot of the underlying Python engine
whereas MiniJinja implements its own runtime data model.

The most significant differences are documented here:

### Python Methods

MiniJinja does not implement _any_ Python methods.  In particular this means
you cannot do `x.items()` to iterate over items.  For this particular case
both Jinja2 and MiniJinja now support `|items` for iteration instead.  Other
methods are rarely useful and filters should be used instead.

Support for these Python methods can however be loaded by registering the
`unknown_method_callback` from the `pycompat` module in the `minijinja-contrib`
crate.

### Tuples

MiniJinja does not implement tuples.  The creation of tuples with tuple syntax
instead creates lists.

### Keyword Arguments

MiniJinja maps keyword arguments to the creation of dictionaries which are passed
as last argument.  This is done as keyword arguments are not native to Rust and
mapping them to filter functions is tricky.  This also means that some filters in
MiniJinja do not accept the parameters with keyword arguments whereas in Jinja2
they do.

### Variadic Calls

MiniJinja does not support the `*args` and `**kwargs` syntax for calls.

### Undefined

The Jinja2 undefined type tracks the origin of creation, in MiniJinja the undefined
type is a singleton without further information.

### Context

The context in Jinja2 is a data source and the runtime system pulls some pieces of
data from the context as necessary.  This optimization is not particularly useful
in MiniJinja and as such is not performed.  This also means that in MiniJinja the
default behavior is to pass the current state of the context everywhere.

### Escaping

Jinja2 only supports HTML escaping, in MiniJinja it's intended to support other
forms of auto escaping as well.

## Blocks

### `{% for %}`

`for` has feature parity with Jinja2.

### `{% if %}`

`if` has feature parity with Jinja2.

### `{% extends %}`

`extends` has feature parity with Jinja2.

### `{% block %}`

`block` has feature parity with Jinja2.

### `{% include %}`

`include` mostly has feature parity with Jinja2 but some deliberate
distinctions were made.  The `without context` and `with context`
modifiers are intentionally not supported.  The context system of
MiniJinja is different as mentioned above.

### `{% import %}`

This tag is supported but the returned item is a map of the exported local
variables.  This means that the rendered content of the template is lost.

### `{% macro %}`

The macro tag works very similar to Jinja2 but with some differences.  Most
importantly the special `varargs` and `kwargs` arguments are not supported.
The external introspectable attributes `catch_kwargs`, `catch_varargs` and
are not supported.

### `{% call %}`

`call` has feature parity with Jinja2.

### `{% do %}`

`do` has feature parity with Jinja2.

### `{% with %}`

`with` has feature parity with Jinja2.

### `{% set %}`

`set` has feature parity with Jinja2.

### `{% filter %}`

`filter` has feature parity with Jinja2.

### `{% autoescape %}`

`autoescape` has feature parity with Jinja2 with an undocumented extension.
Currently it's possible to provide the intended form of auto escaping to
the tag.  This is not documented because it's unclear if this behavior is
useful.

### `{% raw %}`

`raw` has feature parity with Jinja2.

### `{% continue %}`

`continue` is supported only if the `loop_controls` feature is enabled.

### `{% break %}`

`break` is supported only if the `loop_controls` feature is enabled.

## Expressions

Most expressions are supported from Jinja2.  The main difference for expressions
is that `foo["bar"]` and `foo.bar` have the same priority in MiniJinja whereas
in Jinja2 they are used to disambiguate against attributes of the underlying
Python objects.

Differences with expressions mostly stem from the underlying data model.  For
instance Jinja2 templates tend to use `{{ "string" % variable }}` to perform
string formatting which is not supported in MiniJinja.  Likewise not all filters
are available in MiniJinja or behave the same.

## Filters

MiniJinja supports many common Jinja2 filters but leaves out some.  For instance
some string formatting filters like `|xmlattr` or `|urlize` are missing.  Additionally
some filters do not support all the same arguments or only support some arguments
as positional ones.

It's a soft goal to increase the number of filters that are supported and to
match the behavior of Jinja2 as close as possible but there are some situations
where it might be acceptable to deviate.  For instance there are some filters
which in Jinja2 support an `attribute` argument.  Most of those filters currently
do not support that argument in MiniJinja.
