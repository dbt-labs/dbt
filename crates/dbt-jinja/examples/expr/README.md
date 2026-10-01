# expr

This demonstrates how MiniJinja can be used to evaluate expressions.  It accepts a single
argument which is an expression that should be evaluated and the result is printed in JSON
format to stdout.  A single variable is provided to the script (`env`) which contains the
environment variables.

```console
$ cargo run -- 'env.HOME ~ "/.bashrc"'
"/Users/mitsuhiko/.bashrc"
```