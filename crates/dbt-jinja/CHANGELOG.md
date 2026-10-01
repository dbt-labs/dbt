# Changelog

All notable changes to MiniJinja are documented here.

## 2.6.0

- Added `sum` filter.  #648
- Added `truncate` filter to `minijinja-contrib`.  #647
- Added `wordcount` filter to `minijinja-contrib`.  #649
- Added `wordwrap` filter to `minijinja-contrib`.  #651
- Some tests and filters now pass borrowed values for performance reasons
  and a bug was fixed that caused undefined values in strict undefined
  mode not to work with tests.  #657

## 2.5.0

- `minijinja-cli` now supports preservation of order in maps.  #611
- Fixed an issue where CBOR was not correctly deserialized in
  `minijinja-cli`.  #611
- Added a `lines` filter to split a string into lines.
- Bytes are now better supported in MiniJinja.  They can be created from
  `Value::from_bytes` without having to go via serde, and they are now
  producing a nicer looking debug output.  #616
- Added the missing `string` filter from Jinja2.  #617
- Reversing bytes and convergint them implicitly to strings will now work
  more consistently.  #619
- Added type hints for the Python binding and relaxed maturin constraint.  #590
- `minijinja-cli` now allows the template name to be set to an empty
  string when `--template` is used, to allow suppliying a data file.  #624
- Added the missing `sameas` filter from Jinja2.  #625
- Tests can now support one argument without parentheses like in Jinja2
  (`1 is sameas 1`).  #626
- Added error context for strict undefined errors during template
  rendering.  #627
- Syntax errors caused by the lexer now include the correct position of
  the error.  #630
- `minijinja-cli` now has all features enabled by default as documented
  (that means also shell completion and ini).  #633
- `minijinja-cli` now does not convert INI files to lowercase anymore.  This was
  an unintended behavior.  #633
- Moved up MSRV to 1.63.0 due to indexmap.  #635
- Added argument splatting support (`*args` for variable args and `**kwargs`
  for keyword arguments) and fixed a bug where sometimes maps and keyword
  arguments were created in inverse order.  #642

## 2.4.0

- Updated version of `minijinja-cli` with support for better documentation,
  config file and environment variable support.  #602
- `minijinja-cli` now supports template source passed by parameter for
  simple cases.  #606
- `minijinja-cli` now has a `--syntax-help` argument that prints out the
  primer on the syntax.  #607
- `minijinja-cli` now installs to `~/.local/bin` by default.  #608
- Made the c-bindings compatible with wasm compilation.  #603
- `String`/`Cow<str>` argument types will no longer implicitly convert
  keyword arguments to string form.  This was an unintended foot gun.  #605

## 2.3.1

- Fixes a regression in `PartialEq` / `Eq` in `Value` caused by changes
  in 2.3.0.  #584

## 2.3.0

- Fixes some compiler warnings in Rust 1.81.  #575
- Fixes incorrect ordering of maps when the keys of those maps
  were not in consistent order.  #569
- Implemented the missing `groupby` filter.  #570
- The `unique` filter now is case insensitive by default like in
  Jinja2 and supports an optional flag to make it case sensitive.
  It also now lets one check individual attributes instead of
  values.  #571
- Changed sort order of `Ord` to avoid accidentally non total order
  that could cause panics on Rust 1.81.  #579
- Added a `Value::is_integer` method to allow a user to tell floats
  and true integers apart.  #580

## 2.2.0

- Fixes a bug where some enums did not deserialize correctly when
  used with `ViaDeserialize`.  #554
- Implemented `IntoDeserializer` for `Value` and `&Value`.  #555
- Added `filesizeformat` to minijinja-contrib.  #556
- Added support for the `loop_controls` feature which adds
  `{% break %}` and `{% continue %}`.  #558
- Iterables can now be indexed into.  It was already possible previously
  to slice them.  This improves support for Jinja2 compatibility as Jinja2
  is more likely to create temporary lists when slicing lists.  #565

## 2.1.2

- Flush filter and test cache when processing extended template.
  This fixes a bug that caused the wrong filters to be used in some
  cases. #551

## 2.1.1

- Added `indent` parameter to `tojson` filter.  #546
- Added `randrange`, `lipsum`, `random`, `cycler` and `joiner` to
  `minijinja-contrib`.  #547
- Added the ability to use `&T` and `Arc<T>` as parameters
  to filters and functions if `T` is an `Object`.  #548
- `minijinja-cli` now also enables the datetime, timezone and rand features.  #549
- Aligned the behavior of the `int` filter closer to Jinja2.  #549

## 2.1.0

- minijinja-cli now supports `.ini` files.  #532
- Fixed a bug that caused cycle detection to trigger incorrectly when an included
  template extended from another template.  #538
- Bumped the minimum version of `self_cell` to 1.0.4.  #540
- MiniJinja will now warn if the `serde` feature is disabled.  This is in
  anticipation of removing the serde dependency in the future.  #541
- Improved an edge case with `State::resolve`.  It now can resolve the
  initial template context in macro calls even if no closure has been
  created.  #542

## 2.0.3

- Added new methods to pycompat: `str.endswith`, `str.rfind`,
  `str.isalnum`, `str.isalpha`, `str.isascii`, `str.isdigit`,
  `str.isnumeric`, `str.join`, `str.startswith`.  #522
- Added the missing tests `boolean`, `divisibleby`, `lower` and `upper`.  #592
- minijinja-cli now supports YAML aliases and merge keys.  #531

## 2.0.2

- Implemented sequence (+ some iterator) and string repeating with the `*`
  operator to match Jinja2 behavior.  #519
- Added the new `minijinja::pycompat` module which allows one to register
  an unknown method callback that provides most built-in Python methods.
  This makes things such as `dict.keys` work.  Also adds a new
  `--py-compat` flag to `minijinja-cli` that enables it.  This improves
  the compatibility with Python based templates.  #521
- Added a new `|split` filter that works like the `.split` method in Python.  #517

## 2.0.1

- Fixed an issue that caused custom delimiters to not work in the Python
  binding.  #506

## 2.0.0

This is a major update to MiniJinja that changes a lot of core internals and
cleans up some APIs.  In particular it resolves some limitations in the engine
in relation to working with dynamic objects, unlocks potentials for future
performance improvements and enhancements.

It's very likely that you will need to do changes to your code when upgrading,
particular when implementing dynamic objects.  In short:

- `StructObject` and `SeqObject` are gone.  They have been replaced by improved
  APIs directly on `Object`.  Please refer to the updated documentation to see
  how these objects behave now.  For the most part code should become quite a bit
  clearer during the upgrade.
- `ObjectKind` has been replaced by `ObjectRepr`.  Rather than holding a reference
  to a `StructObject` or `SeqObject` this now is a simple enum that just indicates
  how that object serializes, renders and behaves.
- `Object` no longer uses `fmt::Display` for rendering.  Instead the new
  `Object::render` method is used which has a default implementation.
- The `Object` trait has been completely transformed and the new type-erased type
  `DynObject` has been added to work with unknown objects.  This trait has an
  improved user experience and more flexibility.  It's now possible to implement
  any non-primitive value type including maps with non string keys which was previously
  not possible.
- `ValueKind` is now non exhaustive and got a log of new value types.  This resolves
  various issues in particular in relationship with iterators.  As a result of this
  functions will no longer accidentally serialize into empty objects for example.
- `Value::from_iterator` has been replaced by the new `Value::make_iterable`,
  `Value::make_object_iterable` and `Value::make_one_shot_iterator`.  The direct
  replacement is `Value::make_one_shot_iterator` but for most uses it's strongly
  recommended to use one of the other APIs instead.  This results in a much improved
  user experience as it's possible to iterate over such values more than once.
- The `Syntax` type has been replaced by the `SyntaxConfig` type.  It uses a builder
  pattern to reconfigure the delimiters.

For upgrade instructions read the [UPDATING](UPDATING.md) guide.

**Other Changes:**

- Added a new `Environment::templates` method that iterates over loaded templates.  #471
- Reverse iteration and slicing now return iterables instead of real sequences.
- The engine no longer reports iterable as sequences.
- The value iterator returned by `Value::try_iter` now holds a reference
  to the `Value` internally via reference counting.
- `DynObject` now replaces `Arc<Object>`.
- The debug printing of some objects was simplified.
- Added the `iterable` test.  #475
- The parser no longer panics when using dotted assignments in unexpected places. #479
- The CLI now enables unicode support by default.
- `Value::from_serializable` is now `Value::from_serialize`.
- Ranges are now iterables and no longer sequences and their maximum number of iterations
  was raised to 100000. #493
- `Value::from` is now implemented for `Error` as public API to create invalid values.
  Previously this behavior was hidden internally in the serde support. #495
- `UndefinedBehavior::Strict` now acts more delayed.  This means that now `value.key is defined` will no longer fail.
- Added support for line statements and comments.  #503 
- The CLI now accepts `--syntax` to reconfigure syntax flags such as delimiters.  #504

## 1.0.21

- Fixed an issue where `lstrip_blocks` unintentionally also applied to
  variable expression blocks.  #502

## 1.0.20

- Added support for implicit string concatenation for Jinja2 compatibility.  #489
- Added support for sequence concatenation with the plus operator for Jinja2 compatibility.  #491

## 1.0.19

- Deprecated `Value::from_iterator` and introduced replacement
  `Value::make_one_shot_iterator` API which also exists in 2.x.  #487

## 1.0.18

- Fixed an endless loop in `undeclared_variables`.  #486

## 1.0.17

- Added support for `Option<Into<Value>>` as return value from
  functions.  #452
- Deprecated `Value::from_serializable` for the improved replacement
  method `Value::from_serialize`. #482

## 1.0.16

- Tolerate underscores in number literals.  #443
- Added support for `trim_blocks` and `lstrip_blocks` for Jinja2
  compatibility.  #447
- Changed API of `unstable_machinery`.  The `tokenize` function got an
  extra argument for the `WhitespaceConfig`.  It's however recommended
  to use the new `Tokenizer` struct instead.  `parse_expr` was added,
  the `parse` function now takes a `SyntaxConfig` and `WhitespaceConfig`.  #447

## 1.0.15

- Resolved a compiler warning for Rust 1.77.  #440
- Fixed an incorrect error case in `call_method`.  Now `UnknownMethod`
  is returned instead of `InvalidOperation` correctly. #439
- Added `Environment::set_unknown_method_callback` which allows a user
  to intercept method calls on primitives.  The motivation here is that
  this can be used to implement python like methods to improve the
  compatibility with Jinja2 Python templates.  #441

## 1.0.14

- Fixed a bug with broken closure handling when working with nested
  `{% macro %}` or `{% call %}` blocks.  #435

## 1.0.13

- `minijinja-cli` now supports an `-o` or `--output` parameter to write
  into a target file.  #405
- `minijinja-cli` now accepts the `--safe-path` parameter to disallow
  includes or extends from paths not explicitly allowlisted.  #432
- Added support for `Error::display_debug_info` which displays just the
  debug info, same way as alternative display on the error does.  #420
- Added the `namespace()` function from Jinja2 and the ability to assign
  to it via `{% set %}`.  #422
- `minijinja-autoreload` now supports `on_should_reload_callback` which
  lets one register a callback to be called just before an auto reload
  should be performed.  #424
- Added support for `Value::from_iterator`, `IteratorObject` and
  `ObjectKind::Iterator`.  #426
- Added support for `0b`/`0o`/`0x` prefixed integer literals for
  binary, octal and hexadecimal notation.  #433

## 1.0.12

- The `urlencode` filter now correctly skips over none and undefined.  #394
- The `dict` function now supports merging in of extra arguments.  #395
- Added support for primitive datetimes in the contrib module.  #398

## 1.0.11

- Added `Environment::compile_expression_owned` to allow compiled expressions
  to be held without requiring a reference.  #383
- Added `minijinja-embed` crate which provides a simple way to embed templates
  directly in the binary.  #392

## 1.0.10

- Added `int` and `float` filters.  #372
- Added `integer` and `float` tests.  #373
- Fixed an issue that caused the CLI not to run when the `repl` feature
  was disabled.  #374
- Added `-n` / `--no-newline` option to CLI.  #375

## 1.0.9

- Fixed a memory leak involving macros.  Previously using macros was
  leaking memory due to an undetected cycle.  #359
- The `debug` function in templates can now also be used to print out
  the debug output of a variable.  #356
- Added a new `stacker` feature which allows raising of the recursion
  limits.  It enables monitoring of the call stack via [stacker](https://crates.io/crates/stacker)
  and automatically acquires additional memory when the call stack
  runs out of space.  #354

## 1.0.8

- Large integer literals in templates are now correctly handled.  #353
- Relax the trait bounds of `Value::downcast_object_ref` /
  `Object::downcast_ref` / `Object::is` and added support for downcasting
  of types that were directly created with `Value::from_seq_object`
  and `Value::from_struct_object`.  #349
- Overflowing additions on very large integers now fails rather than
  silently wrapping around.  #350
- Fixed a few overflow panics: dividing integers with an overflow and
- Exposed missing new functionality for the Python binding. #339

## 1.0.7

- Added support for `keep_trailing_newlines` which allows you to disable
  the automatic trimming of trailing newlines.  #334
- Added `minijinja-cli` which lets you render and debug templates from
  the command line.  #331
- Macros called with the wrong state will no longer panic.  #330
- Debug output of `Value::UNDEFINED` and `Value::from(())` is now
  `undefined` and `none` rather than `Undefined` and `None`.  This was
  an accidental inconsistency.
- Fixed a potential panic in debug error printing.
- Added `Environment::set_path_join_callback` and `State::get_template`
  to support path joining.  This is for greater compatibility with Jinja2
  where path joining was overridable.  With this you can configure the
  engine so that paths used by `include` or `extends` can be relative to
  the current template.  #328
- The default auto-escape detection now accepts `.html.j2` as alias for
  `.html` as well as for all other choices.  In general `.j2` as an extension
  is now generally supported.

## 1.0.6

- Fixed iso datetime formatting not handling negative offsets correctly.  #327
- Re-report `Value` directly from the crate root for convenience.
- It's now possible to `deserialize` from a `Value`.  Additionally the
  `ViaDeserialize<T>` argument type was added to support value conversions
  via serde as argument type.  #325

## 1.0.5

- Added the ability to merge multiple values with the `context!`
  macro.  (#317)
- `Option<T>` now accepts `none` in filters.  Previously only
  undefined values were accepted.  This bugfix might have a minor impact
  on code that relied in this behavior.  (#320)
- Fix a compilation error for `minijinja-contrib` if the `timezone`
  feature is not enabled.

## 1.0.4

- Added the `args!` macro which can be used to create an argument
  slice on the stack.  (#311)
- Improved error reporting for missing keyword arguments.
- Added `chrono` support to the time filters in the `minijinja-contrib` crate.

## 1.0.3

- Republished `1.0.2` with fixed docs.

## 1.0.2

- Added `TryFrom` and `ArgType` for `Arc<str>`.
- Added `datetimeformat`, `dateformat`, `timeformat` and `now()` to the
  contrib crate.  (#309)

## 1.0.1

- Fixed a bug that caused `{% raw %}` blocks to accidentally not skip the
  surrounding tags, causing `{% raw %}` and `{% endraw %}` to show up in
  output.  (#307)

## 1.0.0

- Support unicode sorting for filters when the `unicode` feature is enabled.
  This also fixes confusing behavior when mixed types were sorted.  (#299)

- Added `json5` as file extension for JSON formatter.

- The autoreload crate now supports fast reloading by just clearning the
  already templates.  This is enabled via `set_fast_reload` on the
  `Notifier`.

**Note:** This also includes all the changes in the different 1.0.0 alphas.

### Breaking Changes

1.0 includes a lot of changes that are breaking.  However they should with
some minor exceptions be rather trivial changes.

- `Environment::source`, `Environment::set_source` and the `Source` type
  together with the `source` feature were removed.  The replacement is the
  new `loader` feature which adds the `add_template_owned` and `set_loader`
  APIs.  The functionality previously provided by `Source::from_path` is
  now available via `path_loader`.

    Old:

    ```rust
    let mut source = Source::with_loader(|name| ...);
    source.add_template("foo", "...").unwrap();
    let mut env = Environment::new();
    env.set_source(source);
    ```

    New:

    ```rust
    let mut env = Environment::new();
    env.set_loader(|name| ...);
    env.add_template_owned("foo", "...").unwrap();
    ```

    Old:

    ```rust
    let mut env = Environment::new();
    env.set_source(Source::from_path("templates"));
    ```

    New:

    ```rust
    let mut env = Environment::new();
    env.set_loader(path_loader("templates"));
    ```

- `Template::render_block` and `Template::render_block_to_write` were
  replaced with APIs of the same name on the `State` returned by
  `Template::eval_to_state` or `Template::render_and_return_state`:

    Old:

    ```rust
    let rv = tmpl.render_block("name", ctx)?;
    ```

    New:

    ```rust
    let rv = tmpl.eval_to_state(ctx)?.render_block("name")?;
    ```

- `Kwargs::from_args` was removed as API as it's no longer necessary since
  the `from_args` function now provides the same functionality:

    Before:

    ```rust
    // just split
    let (args, kwargs) = Kwargs::from_args(values);

    // split and parse
    let (args, kwargs) = Kwargs::from_args(values);
    let (a, b): (i32, i32) = from_args(args)?;
    ```

    After:

    ```rust
    // just split
    let (args, kwargs): (&[Value], Kwargs) = from_args(values)?;

    // split and parse
    let (a, b, kwargs): (i32, i32, Kwargs) = from_args(values)?;
    ```

- The `testutils` feature and `testutils` module were removed.  Instead you
  can now directly create an empty `State` and use the methods provided
  to invoke filters.

    Before:

    ```rust
    let env = Environment::new();
    let rv = apply_filter(&env, "upper", &["hello world".into()]).unwrap();
    ```

    After:

    ```rust
    let env = Environment::new();
    let rv = env.empty_state().apply_filter("upper", &["hello world".into()]).unwrap();
    ```

    Before:

    ```rust
    let env = Environment::new();
    let rv = perform_test(&env, "even", &[42.into()]).unwrap();
    ```

    After:

    ```rust
    let env = Environment::new();
    let rv = env.empty_state().perform_test("even", &[42.into()]).unwrap();
    ```

    Before:

    ```rust
    let env = Environment::new();
    let rv = format(&env, 42.into()).unwrap();
    ```

    After:

    ```rust
    let env = Environment::new();
    let rv = env.empty_state().format(42.into()).unwrap();
    ```

- `intern` and some APIs that use `Arc<String>` now use `Arc<str>`.  This
  means that for instance `StructObject::fields` returns `Arc<str>` instead
  of `Arc<String>`.  All the type conversions that previously accepted
  `Arc<String>` now only support `Arc<str>`.

- `State::current_call` was removed without replacement.  This information
  was unreliably maintained in the engine and caused issues with recursive
  calls.  If you have a need for this API please reach out on the issue
  tracker.

- `Output::is_discarding` was removed without replacement.  This is
  an implementation detail and was unintentionally exposed.  You should not
  write code that depends on the internal state of the `Output`.

## 1.0.0-alpha.4

- `Value` now implements `Ord`.  This also improves the ability of the engine
  to sort more complex values in filters.  (#295)

- `Arc<String>` was replaced with `Arc<str>` in some of the public APIs where
  this shined through.  Support for more complex key types in maps was added.
  You can now have tuple keys for instance.  (#297)

## 1.0.0-alpha.3

- Removed `char` as a value type.  Characters are now represented as strings
  instead.  This solves a bunch of Jinja2 incompatibilities that resulted by
  indexing into strings.  (#292)

- Marked `ErrorKind` as `#[non_exhaustive]`.

- Correctly handle coercing of characters and strings.  `"w" == "w"[0]` is
  now evaluating to `true` as one would expect.

## 1.0.0-alpha.2

- The `include` block now supports `with context` and `without context`
  modifiers but they are ignored.  This is mostly helpful to render some
  Jinja2 templates that depend on this functionality.  (#288)

- Added tests `true`, `false`, `filter`, `test` and filters
  `pprint` and `unique`.  (#287)

- Added support for indexing into strings.  (#149)

- Added `Error::detail` which returns the detail help string.  (#289)

- Added `Error::template_source` and `Error::range` to better support
  rendering of errors outside of the built-in debug printing.  (#286)

## 1.0.0-alpha.1

- Removed `testutils` feature.  New replacement APIs are directly available
  on the `State`.

- Added `Template::render_and_return_state` to render a template and return the
  resulting `State` to permit introspection.  `Template::render_to_write` now
  also returns the `State`.

- Added `State::fuel_levels` to introspect fuel consumption when the fuel feature
  is in use.

- Removed `Source` and the `source` feature.  The replacement is the new `loader`
  feature and the functionality of the source is moved directly into the
  `Environment`.  This also adds `Environment::clear_templates` to unload
  all already loaded templates.  (#275)

- Added `Environment::template_from_str` and `Environment::template_from_named_str`
  to compile templates for temporary use.  (#274)

- Removed `Kwargs::from_args` as this can now be expressed with just
  `from_args`.  (#273)

- `Output` no longer reveals if it's discarding in the public API.

- Added `Value::call`, `Value::call_method` and `Template::new_state`.  The
  combination of these APIs makes it possible to call values which was
  previously not possible.  (#272)

- Added `Template::eval_to_state`.  This replaces the functionality of the
  previous `Template::render_block` which is now available via `State`.
  It also adds support for accessing globals and macros from a template
  via the `State`.  (#271)

- Removed support for `State::current_call`.  This property wasn't too useful
  and unreliable.  Supporting it properly for nested invocations would require
  calls to take a mutable state or use interior mutability which did not seem
  reasonable for this.  (#269)

## 0.34.0

- Updated `self_cell` and `percent-encoding` dependencies.  (#264)

- Added `Template::render_block` and `Template::render_block_to_write` which
  allows rendering a single block in isolation.  (#262)

## 0.33.0

- Hide accidentally exposed `Syntax::compile` method.
- Added `undeclared_variables` methods to `Template` and `Expression`. (#250)

## 0.32.1

- Fixed an issue with autoreload not working properly on windows. (#249)

## 0.32.0

- Added `Value::is_number`. (#240)
- `TryFrom` for `Value` now converts integers to `f32` and `f64`.
- Added the new `custom_syntax` feature which allows custom delimiters
  to be configured. (#245)
- Added `Kwargs` abstraction to easier handle keyword arguments.
- Fixed an issue that `Option<T>` was incorrectly picking up `none`
  for undefined values.
- The `sort` filter now accepts `reverse`, `attribute` and `case_sensitive`
  by keyword argument and sorts case insensitive by default.
- The `dictsort` filter now supports reversing, by value sorting,
  and is sorting case insensitive by default.

## 0.31.1

- The `in` operator now does not fail if the value is undefined and the
  undefined behavior is not strict. (#235)
- The Python binding now supports finalizers. (#238)

## 0.31.0

- Changed the closure behavior of macros to match the one of Jinja2. (#233)
- `Value::from_serializable` will no longer panic on invalid values.  Instead
  the error is deferred to runtime which makes working with objects possible
  that are only partially valid for the runtime environment. (#234)

## 0.30.7

- Added `testutils` module. (#221)
- Make it more obvious that serde flatten sometimes does not work. (#223)
- Added configurable "undefined" value behavior. (#227)
- Make `render!()` reuse the hidden environment.

## 0.30.6

- Resolve bad closure being generated for `do` blocks. (#219)
- Add support for float number values in scientific notation. (#220)

## 0.30.5

- Small performance improvements for when `preserve_order` is used by
  passing known capacities to the constructor.
- Minor performance improvements to the VM by giving the stack an initial
  capacity.
- Change the internal representation of spans to use `u32` rather than
  `usize` for line and column offsets as a small speed improvement during
  compilation.
- Performance improvements for the key interning.
- Disabled `key_interning` by default.
- Renamed features `multi-template` to `multi_template` and
  `adjacent-loop-items` to `adjacent_loop_items`. The old feature names will
  hang around until 1.x as legacy aliases.

## 0.30.4

- Restore compilation on 32bit targets. (#207)

## 0.30.3

- Added the Jinja2 tests `==`, `!=`, `<`, `<=`, `>`, `>=` and `in` for the
  use with `select` and `reject`. (#205)
- String rendering now uses fewer reallocations by setting an initial
  capacity based on the complexity of the template. (#206)

## 0.30.2

- Fixed Python bindings not allowing to access dict keys prefixed with an
  underscore. (#197)
- Added `min`, `max` and `sort` filters. (#199)

## 0.30.1

- Changed `add_global` to perform `Into<Value>` implicitly.
- Added `source_mut` to retrieve a mutable reference to the source.
- Improved Python bindings by adding support for template reloading, better
  documentation, error reporting.
- Added `pass_state` to the Python binding.

## 0.30.0

- Expose `debug` flag from environment.
- Added experimental Python bindings.

## 0.29.0

- Resolve a runtime panic if `{% extends %}` appears in the template
  but never is executed.
- Fixed recursion detection for macros.
- Enforce a maximum size of 10000 items on the range output.
- Added fuel tracking support.
- Added `none` test. (#185)
- Improved various issues with whitespace trimming. (#187)
- `Value::default` now returns `Undefined` rather than `None`.
- Added support for `loop.previtem` and `loop.nextitem`. (#188)

## 0.28.0

- Added `capitalize` filter. (#163)
- Added support for `{% call %}`. (#164)
- Added support for `{% do %}`. (#167)
- Improved testsuite to execute on wasm32-wasi.

## 0.27.0

- Filters, tests and other functions can now be registered with a dynamically
  allocated name. (#146)
- Added `State::current_call` which exposes the name of the currently called
  item. (#150)
- Introduced revamped object model with `SeqObject` and `StructObject`. (#148)
- `Object` now directly exposes `downcast_ref` and `is`.
- Removed `Value::as_slice`
- Introduced `Value::as_seq` and `Value::as_struct`.
- Introduced `Value::from_seq_object` and `Value::from_struct_object`.
- Added the ability for function arguments to be of type `&dyn SeqObject`.
- Renamed `Iter` to `ValueIter`.
- Added `Environment::render_named_str`. (#149)
- Exposed string interning via the `intern` function.
- Improved support for structs in built-in filters.
- Added `indent` filter. (#151)
- Added the `map`, `select` / `selectattr` and `reject` / `rejectattr` filters.
- Added `safe` / `escaped` test.
- Strings now have the same iteration behavior as in Jinja2. (#152)

### Breaking Changes

- The `Object` model changed significantly in this release.  It's now possible
  for objects to have different shapes (structs or sequences today).  As a result
  `SeqObject` and `StructObject` were added to the API.  For changing your objects
  over have a look at the new documentation for `Object`.
- The `Iter` type is now called `ValueIter`.

## 0.26.0

- Changed `Object::attributes` to being an iterator. (#138)
- `Arc<T: Object>` now implements `Object`. (#139)
- Aligned semantics of top-level template code after `extends` with Jinja2. (#140)
- Exposed value creation from Arcs. (#141)
- Performance improvements for value conversions and object creation. (#142)
- Align iteration behavior of dynamic objects with maps.

### Breaking Changes

- The `attributes` method on objects now returns iterators.  To make the
  transition easy change `[..]` to `Box::new([..].into_iter())`.

## 0.25.0

- Added support for recursive macro invocations. (#133)
- Added optional unicode identifier support. (#134)

## 0.24.0

- Catch divisions by zero.
- Correctly render `inf` floats.
- Enforce a maximum recursion depth during parsing.
- Added `Value::try_iter` to iterate over maps and sequences. (#132)

## 0.23.0

- Added `Value::from_function`. (#121)
- Fixed incorrect location information with blocks.
- Fixed broken nested `super()` calls.
- Improve error reporting for failures in blocks and trying to
  `super()` when there is no parent block.
- Performance improvements.
- Added support for `{% import %}` / `{% from .. import .. %}`
  and `{% macro %}`.  (#123)
- Added `Value::is_kwargs` which disambiguates if an object passed
  to a function or filter is a normal object or if it represents
  keyword arguments.
- Added the ability to call functions stored on objects.
- Added `macros` and `multi_template` features to disable some of
  the heavier pieces of MiniJinja.
- Fixed an issue that caused trailing commas not to work in lists.

## 0.22.1

- Fixed an incorrect manifest for `minijinja-autoreload`.

## 0.22.0

- Add `defined` and `undefined` tests to always be included.
- Removed `Source::load_from_path`.
- Added `Source::from_path`.

### Breaking Changes

- Removed `Source::load_from_path`.  Use `Source::with_loader` instead.

## 0.21.0

- Added custom autoescape settings.
- Added custom formatters.
- Restructured engine internals for greater clarity.
- Added support for rendering to `io::Write`.  (#111)
- Make it impossible to implement `Filter`, `Test` or `Function`
  from outside the crate by sealed the traits.  (#113)
- Added support for remaining arguments with `Rest`.  (#114)
- Filters, tests and functions can now borrow arguments.  (#115)
- Added `Value::as_slice` and `Value::get_get_item_by_index`.
- Added support for span error reporting. (#118)
- Greatly improved handling of nested errors. (#119)
- `ImpossibleOperation` is now `InvalidOperation`.
- Added support for slice syntax. (#120)

### Breaking Changes

- `Filter`, `Test` and `Function` are now sealed traits.
- `ErrorKind::ImpossibleOperation` is now `ErrorKind::InvalidOperation`.
- Moved up MSRV to 1.61.0 due to bugs on older rust versions related to
  HRTBs.

## 0.20.0

- Remove internal refcounts from environment.
- Changed `Object` and `FunctionArgs` interface to take
  arguments by reference. (#101)
- `sync` mode is now always enabled. (#104)
- Removed meta API. (#105)
- Error no longer implements `PartialEq`.
- Simplified the function interface.  Filters, tests and global
  functions can now directly return values instead of results. (#107)
- MiniJinja no longer silently iterates over non iterable values.

### Breaking Changes

- The `meta` API is gone without replacement.
- `Object::call_method` and `Object::call` now take the arguments
  as `&[Value]` instead of `Vec<Value>`.
- `Object::call_method`, `Object::call` and `FunctionArgs::from_values`
  now take the arguments as `&[Value]` instead of `Vec<Value>`.
- The error object used to implement `PartialEq` but this was implemented
  by comparing the error kind instead.  Explicitly use the `.kind()`
  method of the error if you want the same behavior.
- `DebugInfo` is no longer exposed.  This might come back if a better
  API can be found.

## 0.19.1

- Fixed binary subtraction operator requiring a space. (#94)
- Fixed trailing commas not working in function calls. (#95)

## 0.19.0

- Small internal improvements to context creation. (#79)
- Add support for JSON/YAML/JavaScript Escaping.  (#82)
- Add missing escape support for single quotes (`'`).  (#81) 
- Added support for newlines in string literals.  (#85)
- Added support for block assignment syntax.  (#86)
- Added string concatenation with `+` for Jinja compat.  (#87)
- Enable debug mode by default in debug builds.  (#88)
- Added `render!` macro and `render_str`.  (#89)
- Fixed an issue where trailing whitespace removal did not work on blocks.  (#90)
- Added `loop.changed()` method.  (#91)

## 0.18.1

- Fixed a bad dependency declaration.

## 0.18.0

- Improved debug printing of context.
- Added `-`, `_` and `.` to set of unescaped characters in `urlencode`. (#72)
- Bumped `v_htmlescape` dependency. (#74)

## 0.17.0

- Added support for `{% raw %}`. (#67)
- Minimum Rust version moved up to 1.45.
- Added support for `{% set %}`. (#70)

## 0.16.0

- Added support for unpacking in `with` blocks. (#65)

## 0.15.0

- Bumped minimum version requirement to 1.43.
- Internal refactorings.
- Added support for fully dynamic loading via `Source::with_loader`.
- Renamed `get_source` to `source`.

## 0.14.1

- Fixed `or` expressions not working properly.

## 0.14.0

- Added `bool` filter.
- Added `meta` API. (#55)
- Added support for `ignore missing` in include tags. (#56)
- Added support for choices in include tags. (#57)

## 0.13.0

- Removed deprecated functionality.
- Fix an panic in debug error printing. (#49)

## 0.12.0

- Deprecated `Primitive` and `as_primitive`.
- Deprecated `as_f64`.
- Truthiness of values is now checking container length.  Previously containers
  were always true, now they are only true if they are not empty.
- Strings and safe strings no longer compare the same.
- Changed list and map string output to have delimiters and debug printing.
- Added `batch` and `slice` filter.
- Added the new `items` filter.
- Removed value internal distinction between maps and structs.
- Added `list` filter.
- Added `first` and `last` filters.
- Added `round` and `abs` filters.
- Implemented integer division operator (``//``) and changed division to always
  return floats like documented and to match the Jinja2 implementation.  To
  make this more convenient whole integer floats are now handled like integers in
  some situations.
- Added `recursive` support to for loops.
- Merged `builtin_filters`, `builtin_tests` and `builtin_functions` features
  into `builtins`.
- Added `value::serializing_for_value` to check if serialization is taking place
  for MiniJinja.
- The `Value` type now supports deserialization.  This feature can be disabled
  by removing the default `deserialization` feature.
- Removed optional `memchr` dependency as it does not appear to be useful.

## 0.11.0

*Yanked* — this was a release from the wrong branch

## 0.10.0

- Restructured the value type internally to be simpler and not use unsafe at the
  cost of slightly larger memory footprint. (#30)
- Added `debug` support.  If the debug mode is enabled, errors now carry a lot of
  useful debug information and an alternative representation when formatted into
  strings that show the context of the template where it went all wrong. (#31)
- Added automatic string interning of object/map keys in values. This feature can
  be disabled by removing the default `key_interning` feature. (#35)
- Removed deprecated `Single` type.

## 0.9.0

- Remove one trailing newline to be consistent with Jinja2.
- Resolved a bug where borrowed keys on dynamic objects could not be looked up. (#29)

## 0.8.2

- Restored unconditional compatibility with 1.42.

## 0.8.1

- Turned on all features for the docs on docs.rs

## 0.8.0

- Added `context!` and deprecate `Single`.
- Correctly report template file names in errors.
- Added the `source` method on templates.
- Introduced `State` type and changed parameter to functions from
  `&Environment` to `&State`.
- Added `debug` global.
- Added `tojson` filter.
- Added `urlencode` filter.

## 0.7.0

- Made the `source` method on error be bound to `Send` and `Sync`.

## 0.6.0

- Added `default` filter.
- Added `startingwith` and `endingwith` tests.
- Added global variables and function support.
- Added `range` function.
- Added `dict` function.
- Fixed panic caused by `super()` calls outside of blocks.
- Added `Error::with_source` method.
- Added `object` abstraction.
- Added keyword arguments to function and filter invocations.
- Added Jinja2 filter aliases `e`, `d` and `count`.

## 0.5.0

- Added support for rustc 1.41.0
- Added `v_htmlescape` feature to turn on a faster HTML escaping.
- Export `HtmlEscape` helper.
- Also escape `/` in HTML escaping like `v_htmlescape` does.
- Changed return value type of `get_template` to be a result rather than an
  option.
- Added `Source` behind the `source` feature to support loading of templates
  at runtime without lifetime complications.
- Initial auto escaping decision is now made when the template is loaded from
  the environment and not when they are added.
- The environment can now be cloned.
- Added `sync` feature that can be disabled to disable the ability to send
  objects to other threads where that comes at a cost.
- `safe` and `escape` are now always provided as filters.
- Added support for `self.block_name()`.
- Fixed incorrect behavior where `super()` did not allow filters.
- Added `{% filter %}` blocks.
- Added `value::Single` type to render simple templates with a single value passed.

## 0.4.0

- Added the ability to roundtrip arbitrary values via the serialize interface.
- Added support for tuple unpacking in lists.
- Added dictsort filter.
- Introduced a new trait `ArgType` to handle argument conversions for filters
  and tests so optional arguments can exist.
- Renamed `ValueArgs` trait to `FunctionArgs`.
- Added `reverse` filter.
- Added `trim` filter.
- Added `join` filter.
- Added `number` test.
- Added `string` test.
- Added `sequence` test.
- Added `mapping` test.
- Added `builtin_filters` and `builtin_tests` features to disable the built-in
  filter and test functions.
- Added `is not` syntax for negated tests.
- Added `else` block to for loops.
- Added `if` condition expression to for loops.
- Fixed a bug that caused or/and not to evaluate correctly in certain situations.
- Added `in` and `not in` expressions.
- Added inline `if` expressions.

## 0.3.0

- Added support for `{% include %}`
- Resolved a bug that caused `with` blocks to fully shadow the outer scope.
- Improved documentation in the crate.

## 0.2.0

- Added support for rustc versions down to 1.42.0

## 0.1.0

- Initial release of the library
