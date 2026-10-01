from minijinja import Environment

INDEX = """{% extends "layout.html" %}
{% block title %}{{ page.title }}{% endblock %}
{% block body %}
  <ul>
  {%- for item in items %}
    <li>{{ item }}
  {%- endfor %}
  </ul>
{% endblock %}
"""
LAYOUT = """<!doctype html>
<title>{% block title %}{% endblock %}</title>
<body>
  {% block body %}{% endblock %}
</body>
"""

env = Environment(templates={
    "index.html": INDEX,
    "layout.html": LAYOUT,
})

print(env.render_template(
    'index.html',
    page={"title": "The Page Title"},
    items=["Peter", "Paul", "Mary"]
))
