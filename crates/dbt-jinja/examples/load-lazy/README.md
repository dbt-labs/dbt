# load-lazy

This example loads data automatically as it's needed.  The `site` object in the
engine is providing data whenever attributes are accessed.  This example is similar
to the [load-resource](../load-resource) example but instead of loading data on a
function call, the data is lazily loaded when attributes of an object are accessed.

Also see [dynamic-context](../dynamic-context) for an example where
the entire context is lazily computed.

```jinja
<ul>
  {%- for item in site.nav %}
    <li><a href="{{ item.href }}">{{ item.title }}</a>
  {%- endfor %}
</ul>
```

```console
$ cargo run
<ul>
    <li><a href="&#x2f;">Index</a>
    <li><a href="&#x2f;downloads">Downloads</a>
    <li><a href="&#x2f;contact">Contact</a>
</ul>
```
