# self-referential-context

A context wrapper that acts self-referential.  This allows variables passed to
`render` to be also reachable via a `CONTEXT` alias.  In short: a template
can render a variable `name` via `{{ name }}` or also `{{ CONTEXT.name }}`.
This would permit things such as `{{ CONTEXT|tojson }}`.

```console
$ cargo run
```
