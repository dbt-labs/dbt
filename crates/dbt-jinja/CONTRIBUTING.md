# Contributing to MiniJinja

Thanks for your interest in contributing to MiniJinja! MiniJinja welcomes
contribution from everyone in the form of suggestions, bug reports, pull
requests, and feedback. This document gives some guidance if you are thinking of
helping out.

## Submitting Bug Reports and Feature Requests

When reporting a bug or asking for help, please include enough details so that
others helping you can reproduce the behavior you are seeing.

Opening an issue is as easy.
Just [follow this link](https://github.com/mitsuhiko/minijinja/issues/new/choose)
and fill out the fields in the appropriate provided template.

When making a feature request, please make it clear what problem you intend to
solve with the feature and maybe provide some ideas for how to go about that.

## MiniJinja 1

The main branch is pointing to the not yet released major version of MiniJinja1.  For
the old 1.x versions see head over to the `minijinja-1.x` branch:
[`minijinja-1.x`](https://github.com/mitsuhiko/minijinja/tree/minijinja-1.x)

## Rust toolchain
MiniJinja targets [Rust 1.63.0](https://blog.rust-lang.org/2022/08/11/Rust-1.63.0.html) as it's MSRV (Minimum Supported Rust Version).

If you use nightly Rust, you might be using features that aren't supported yet by stable.

Using [rustup](https://rustup.rs/) is a straight forward way to manage your installed toolchains, you have a couple of options
to run `1.63.0` locally in the MiniJinja project:

1. You can also create a [rust-toolchain.toml](https://rust-lang.github.io/rustup/concepts/toolchains.html) file in the root directory:

```toml
[toolchain]
channel = "1.63.0"
```

Then running `rustup update` will ensure you have the latest stable toolchain.

2. Using [directory overrides](https://rust-lang.github.io/rustup/overrides.html#directory-overrides) will
set the MiniJinja directory to use the stable toolchain:

```sh
rustup override set 1.63.0
```

To verify you are on 1.63.0, you can use `rustc --version`.

You can also use `rustup toolchain list`, which will show the installed and currently used toolchain.

## Running the Tests

When you want to contribute directly please make sure to run the tests and
format the code before making a pull request. Tests are also run in CI, but
it's typically easier to run them locally.

To run all tests a makefile is provided

```sh
make test
```

To run a single test in a test file, for example [test_vm](./minijinja/tests/test_templates.rs), you will
need to ensure you are passing `--all-features`:

```sh
cargo test test_vm --all-features
```

MiniJinja tests use the [Insta](https://insta.rs) testing framework. While not
required, it is recommended to use
the [`cargo insta review`](https://insta.rs/docs/cli/#review) command to review
and verify changes to the test results.  This can be automated by using
`make snapshot-tests`.

## Formatting the Code

If you want to format the code you can quickly run this command:

```sh
make format
```

The Github Actions CI will also run a check to ensure the code is formatted correctly when
submitting a pull request.

## Linting the code

MiniJinja uses [clippy](https://github.com/rust-lang/rust-clippy) to lint the codebase.

To run clippy you can use the following command, which will ensure that clippy is installed for you:

```sh
make lint
```

Alternatively, you can use what lint does, if you don't have make:
```sh
cargo clippy --all -- -F clippy::dbg-macro -D warnings
```

The Github Actions CI will also run a check to ensure the code is linted correctly with clippy
when submitting a pull request.

## Conduct

This issue tracker follows the [Rust Code of Conduct]. For escalation or
moderation issues please contact Armin (armin.ronacher@active-4.com) instead of
the Rust moderation team.

[rust code of conduct]: https://www.rust-lang.org/policies/code-of-conduct
