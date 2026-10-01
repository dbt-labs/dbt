# MiniJinja-Autoreload

[![Build Status](https://github.com/mitsuhiko/minijinja/workflows/Tests/badge.svg?branch=main)](https://github.com/mitsuhiko/minijinja/actions?query=workflow%3ATests)
[![License](https://img.shields.io/github/license/mitsuhiko/minijinja)](https://github.com/mitsuhiko/minijinja/blob/main/LICENSE)
[![Crates.io](https://img.shields.io/crates/d/minijinja-autoreload.svg)](https://crates.io/crates/minijinja-autoreload)
[![rustc 1.63.0](https://img.shields.io/badge/rust-1.63%2B-orange.svg)](https://img.shields.io/badge/rust-1.63%2B-orange.svg)
[![Documentation](https://docs.rs/minijinja-autoreload/badge.svg)](https://docs.rs/minijinja-autoreload)

MiniJinja-Autoreload is a utility crate for [MiniJinja](https://github.com/mitsuhiko/minijinja)
that adds an abstraction layer that provides auto reloading functionality of environments.

This simplifies fast development cycles without writing custom code.

```rust
use minijinja_autoreload::AutoReloader;
use minijinja::{Source, Environment};

let reloader = AutoReloader::new(|notifier| {
    let mut env = Environment::new();
    let template_path = "path/to/templates";
    notifier.watch_path(template_path, true);
    env.set_source(Source::from_path(template_path));
    Ok(env)
});

let env = reloader.acquire_env()?;
let tmpl = env.get_template("index.html")?;
```

For an example have a look at the [autoreload example](https://github.com/mitsuhiko/minijinja/tree/main/examples/autoreload).

## Sponsor

If you like the project and find it useful you can [become a
sponsor](https://github.com/sponsors/mitsuhiko).

## License and Links

- [Documentation](https://docs.rs/minijinja-autoreload/)
- [Issue Tracker](https://github.com/mitsuhiko/minijinja/issues)
- License: [Apache-2.0](https://github.com/mitsuhiko/minijinja/blob/main/LICENSE)
