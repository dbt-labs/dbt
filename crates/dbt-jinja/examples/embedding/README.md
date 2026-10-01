# embedding

This example shows how to embed templates in the binary
configured by a feature flag.  This example uses
[`minijinja-embed`](https://docs.rs/minijinja-embed).

Load the templates at runtime:

```console
$ cargo run
```

Load the templates at compiled time into the binary:

```
$ cargo run --features=bundled
```
