# Fuzzing

This is a pretty barebones fuzzing setup for MiniJinja.  Right now there are two things
which can be fuzzed: rendering and adding templates to the environment (parse + compile).

For this to work you need to have `cargo-fuzz` installed:

```
$ cargo install cargo-fuzz
```

To run the fuzzers one of the following two commands can be used:

```
$ make fuzz-add-template
$ make fuzz-render
```

The render fuzzer is slightly more tricky to work with as part of what it's fuzzing is
input to the template render function.  The template adding fuzzer upon crashing will
dump the input as raw text into the artifacts directory which makes it trivial to
understand what is going on.

To repro a crash to iterate on it, use `make repro` with the right crash file:

```
$ make repro ARTIFACT=artifacts/render/crash-XXXX
```
