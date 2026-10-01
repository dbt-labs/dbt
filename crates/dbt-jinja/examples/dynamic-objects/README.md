# dynamic-objects

This example demonstrates how to pass dynamic runtime objects to the
engine for custom behavior.

```jinja
{%- with next_class = cycler(["odd", "even"]) %}
  <ul class="{{ magic.make_class("ul") }}">
  {%- for char in seq %}
    <li class={{ next_class() }}>{{ char }}</li>
  {%- endfor %}
  </ul>
{%- endwith %}"#,
```

```console
$ cargo run
  <ul class="magic-ul">
    <li class=odd>a</li>
    <li class=even>b</li>
    <li class=odd>c</li>
    <li class=even>d</li>
  </ul>
```
