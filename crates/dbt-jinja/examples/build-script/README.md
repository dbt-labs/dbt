# build-script

This example demonstrates how to use minijinja for build scripts.  It uses
a custom formatter to automatically debug-format all rust values which means
that the template does not need to take care of it.  The `|safe` filter
is then applied to automatically format values as valid rust expressions.

Have a look at [build.rs](build.rs) to see how the template is rendered and
[example.rs.jinja](src/example.rs.jinja) to see the example that is used to
render the Rust code.

```console
$ cargo run
```

The template that generates the Rust code:

```jinja
struct Point {
    pub x: f32,
    pub y: f32,
}

const BUILD_CWD: &str = {{ build_cwd }};
const POINTS: [{{ struct_name|safe }}; {{ points|length }}] = [
    {% for x, y in points %}
    {{ struct_name|safe }} { x: {{ x }}, y: {{ y }} },
    {% endfor %}
];
```

The generated output (with the values passed from the build script):

```rust
struct Point {
    pub x: f32,
    pub y: f32,
}

const BUILD_CWD: &str = "/build/minijinja/examples/build-script";
const POINTS: [Point; 3] = [
    Point { x: 1.0, y: 2.0 },
    Point { x: 2.0, y: 2.5 },
    Point { x: 4.0, y: 1.0 },
];
```
