# generate-yaml

This demonstrates how MiniJinja can be used to generate YAML files with automatic escaping.
It renders a YAML template and fills in some values which are automatically formatted to
be valid JSON and YAML syntax.

```jinja
env: {{ env }}
title: {{ title }}
skip: {{ true }}
run: {{ ["bash", "./script.sh"] }}
yaml_value: {{ yaml|safe }}
```

```console
$ cargo run
env: {"PATH": "/tmp"}
title: "Hello World!"
skip: true
run: ["bash","./script.sh"]
yaml_value: [1, 2, 3]
```
