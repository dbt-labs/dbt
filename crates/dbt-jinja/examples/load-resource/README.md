# load-resource

This example loads data at runtime through a function call from JSON files.

```jinja
{% set nav = load_data("nav.json") %}
<ul>
  {%- for item in nav %}
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
