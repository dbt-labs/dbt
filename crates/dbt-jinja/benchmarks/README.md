# Benchmarks

This is the beginning of a basic benchmarking suite.  So far it doesn't do much.
These use [criterion.rs](https://github.com/bheisler/criterion.rs) for testing.

There are some benchmarks for the engine itself to track changes over time and
comparison benchmarks against `handlebars`, `liquid` and `tera`.

To run the benchmarks:

```
$ cargo bench
```

## Comparison Results

These are the results run on a MacBook Pro 16" 2021:

```
cmp_compile/handlebars  time:   [47.043 µs 47.163 µs 47.295 µs]
cmp_compile/liquid      time:   [28.669 µs 28.776 µs 28.919 µs]
cmp_compile/minijinja   time:   [4.6921 µs 4.7003 µs 4.7092 µs]
cmp_compile/tera        time:   [35.145 µs 35.227 µs 35.315 µs]

cmp_render/askama       time:   [1.5744 µs 1.5793 µs 1.5846 µs]
cmp_render/handlebars   time:   [6.3056 µs 6.3221 µs 6.3399 µs]
cmp_render/liquid       time:   [11.364 µs 11.394 µs 11.426 µs]
cmp_render/minijinja    time:   [5.0475 µs 5.0600 µs 5.0739 µs]
cmp_render/tera         time:   [7.1101 µs 7.1285 µs 7.1482 µs]
```

Note that Askama compiles templates as part of the Rust build
process and uses static typing, as such it does not have a compile
time benchmark.
