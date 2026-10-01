# minijinja-cabi

[![Build Status](https://github.com/mitsuhiko/minijinja/workflows/Tests/badge.svg?branch=main)](https://github.com/mitsuhiko/minijinja/actions?query=workflow%3ATests)
[![License](https://img.shields.io/github/license/mitsuhiko/minijinja)](https://github.com/mitsuhiko/minijinja/blob/main/LICENSE)
[![rustc 1.63.0](https://img.shields.io/badge/rust-1.63%2B-orange.svg)](https://img.shields.io/badge/rust-1.63%2B-orange.svg)

`minijinja-cabi` is a crate that wraps
[MiniJinja](https://github.com/mitsuhiko/minijinja) into a C library.
This is an experimental and not published crate.

For an example look into [hello.c](example/hello.c).

```c
#include <minijinja.h>
#include <stdio.h>

int main()
{
    mj_env *env = mj_env_new();

    bool ok = mj_env_add_template(env, "hello", "Hello {{ name }}!");
    mj_value ctx = mj_value_new_object();
    mj_value_set_string_key(&ctx, "name", mj_value_new_string("C-Lang"));

    char *rv = mj_env_render_template(env, "hello", ctx);
    if (!rv) {
        mj_err_print();
    } else {
        printf("%s\n", rv);
        mj_str_free(rv);
    }

    mj_env_free(env);

    return 0;
}
```

## Sponsor

If you like the project and find it useful you can [become a
sponsor](https://github.com/sponsors/mitsuhiko).

## License and Links

- [Issue Tracker](https://github.com/mitsuhiko/minijinja/issues)
- License: [Apache-2.0](https://github.com/mitsuhiko/minijinja/blob/main/LICENSE)