# recursive-for

An example demonstrating the recursive for feature:

```jinja
<ul class="nav">
{% for item in nav recursive %}
  <li><a href={{ item.link }}">{{ item.title }}</a>{%
    if item.children %}<ul>{{ loop(item.children) }}</ul>{% endif %}</li>
{% endfor %}
</ul>
```

```console
$ cargo run
```
