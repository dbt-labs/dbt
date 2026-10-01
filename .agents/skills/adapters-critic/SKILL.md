---
name: adapters-critic
description: Use in a reviewer agent in agent loops or when reviewing your own code.
when_to_use: Use before editing any code in Adapters or related crates.
argument: [tentative-edit]
context: fork
allowed-tools: Bash(git log *) Bash(git diff *) Bash(grep) Bash(ls) Bash(cat)
---

# Your task

You are a code critic agent: your job is to find flaws in code written by other agents or humans, according to the project's intended architecture principles.

Step by step review process follows.

## Load the patch

If the patch body:
- Is empty, use `git diff HEAD` instead
- Is a path to a file, read that file and use the contents instead
- Is a verbatim git patch, use it as-is

## Identify which adapter is being worked on

Usually, a patch relates to a specific `AdapterType`, like `DuckDB`, `Bigquery` or `Snowflake`.

Identify which one we're talking about. From now on, we'll refer to that as `FoofingAdapter` (or `foofing`).


## Code review

You will perform a few sub-steps, each looking for a specific pattern or code smell that is considered bad according to the architecture goals. You will generate a comment with a succint description and suggest an edit if you find that that code is bad.

These are the substeps:

### Code review - step 1: Find duplicated code

Look for code that is a plain copy of other code, for example
```
fn my_function() {
    // ...
    if adapter_type == AdapterType::FoofingAdapter {
        // duplicate code, just added a wrapper call
        return a(b(c));
    }
    b(c)
}
```

Suggest compressing the code so that it works generically across platforms. In the example above, you'd suggest sharing as most of the code paths as possible, by sharing the call to `b(c)` and not just copying it
```
fn my_function() {
    // ...
    let result = b(c);
    let result = match adapter_type {
        AdapterType::FoofingAdapter => a(result),
        _ => {},
    }
    result
}
```

Another example of bad code:
```
let relation = match adapter_type {
    AdapterType::FoofingAdapter => Relation::new(adapter_type)
            .with_quoting(Policy::trues()),
    AdapterType::Another => Relation::new(adapter_type)
            .with_quoting(Policy::falses()),
}
```

Replace with a single callsite to `new()` and `with_quoting()`
```
let quoting = match adapter_type {
    AdapterType::FoofingAdapter => Policy::trues(),
    AdapterType::Another => Policy::falses(),
};
let relation = Relation::new(adapter_type).with_quoting(quoting);
```

### Code review - step 2: Avoid target-specific free functions

Look for platform-specific free functions, structs or enums outside of platform-specific modules, this is a red flag. Code such as the following should be avoided in generic modules:
```
enum ClickhouseSomething {
   // ...
}

struct FabricSomethingElse {
   // ...
}

fn foofing_do_something() {
}

fn duckdb_foo_bar() {
}

fn lala_bigquery_lele() {
}

fn do_something_in_snowflake() {
}
```

In generic modules such as `relation_impl.rs`, `adapter_impl.rs` and `dbt-adbc`, there should be only generic code with `match adapter_type` statements that call entrypoints to the platform-specific modules.

Suggest replacing with something along the lines of:

```
// In the `foofing` module
fn foofing_do_something() {
    // platform-specific code goes here
}

// In the generic module
use crate::submodule::foofing::foofing_do_something()

fn do_something() {
    match adapter_type {
        AdapterType::FoofingAdapter => foofing_do_something(),
        AdapterType::Bigquery => panic!("Bigquery does not support do_something()")
        _ => unimplemented!("do_something() unimplemented for this adapter_type")
    }
}
```

Note that it is OK to do `panic!()` in other code if the other platform does not implement that feature, or `unimplemented!()` if we haven't done it for that platform yet.

**Real example:** `adapter/mod.rs` used to register two Jinja-dispatched entrypoints, each hardcoded to one Snowflake relation type, sitting right next to `describe_relation` — a generic, adapter-agnostic name already used by BigQuery:

```rust
"describe_dynamic_table" => self.describe_dynamic_table(state, args),
"describe_interactive_table" => self.describe_interactive_table(state, args),
```

Fixed (PR #13201, following up on #12664 review feedback) by keeping one Jinja-facing name and dispatching internally on adapter type and relation type, instead of on the Jinja method name:

```rust
if adapter.adapter_type() == AdapterType::Snowflake {
    match relation.relation_type() {
        Some(RelationType::DynamicTable) => adapter.describe_dynamic_table(/* ... */),
        Some(RelationType::InteractiveTable) => adapter.describe_interactive_table(/* ... */),
        other => Err(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("describe_relation is not supported for relation type {other:?} on Snowflake"),
        )),
    }
} else {
    Ok(adapter.describe_relation(conn.as_mut(), &relation, Some(state))?
        .map(Value::from_object)
        .unwrap_or_else(none_value))
}
```

The old per-type Rust methods stayed as private helpers called from the match arms — only the Jinja-facing dispatch surface needed to become generic, not the method bodies themselves.

### Code review - step 3: No SQL-processing utilities outside of `dbt-adapter-sql` or `dbt-adapter-keywords`

If you find SQL keyword normalization/sanitization, statement splitting, keyword detection etc outside of the `dbt-adapter-sql` and `dbt-adapter-keywords` crates, suggest moving those there. You may explore those crates to give better suggestions of how to port the code. Example code that should live in these crates.

```
fn format_identifier() {
    // do SQL identifier formatting
}

fn sanitize_identifier() {
    // do SQL identifier sanitization
}

fn requires_quotes() {
    // check if an identifier requires quotes
}
```

If those are free functions, apply the same rules as in "step 2" and suggest a generic function that does `match adapter_type`. You can look at the existing functions in `dbt-adapter-sql` and `dbt-adapter-keywords` to see if code can be consolidated.


### Code review - step 4: Replace `if adapter_type` with `match`

What to look for:

```
if adapter_type == AdapterType::FoofingAdapter {
    // target-specific code 
    return;
}
// generic code
```

```
if adapter_type == AdapterType::FoofingAdapter {
    // target-specific code 
} else {
    // generic code
}
```

Suggest replacing with:

```
match adapter_type {
    AdapterType::FoofingAdapter => {
        // target-specific code
    }
    _ => {
        // generic code
    }
}
```

### Code review - step 5: No useless comments

Find obvious comments that directly comment what the following code does, for example
```
let db = match adapter_type {
    // THE COMMENT BELOW IS BAD
    // database is uppercased in Foofing
    AdapterType::Foofing => raw_db.to_uppercase(),
    _ => raw_db.to_uppercase(),
}
```

Instead, suggest simply deleting the the comment:
```
let db = match adapter_type {
    AdapterType::Foofing => raw_db.to_uppercase(),
    _ => raw_db.to_uppercase(),
}
```

The one exception is if the comment carries meaningful information that is not expressed in the code. In such cases, the code should ALWAYS be annotated with reference links to more context.

For example, the following comment is okay:
```
let db = match adapter_type {
    // For historical reasons, Foofing does database uppercasing to avoid identifier conflicts when foo bars.
    // Original Python adapter implementation: <link to python code>
    // Documentation for Foofing identifier matching: <link to python code>
    AdapterType::Foofing => raw_db.to_uppercase(),
    _ => raw_db.to_uppercase(),
}
```

### Code review - step 6: Final questions

Ask yourself: 
- Does this function/struct/enum belong here? Is there any other code in the codebase that does the same/similar thing? Where does that code live? Consider whether that might be a more appropriate module for that code.
- Are there any free functions returning booleans that drive one-off control flow? That is bad.

## Output

After gathering all the steps above, you will output your findings in markdown consisting of "header with short explanation, file:lineno, patch". DO NOT output your reasoning or long prose, only the necessary patch, file name/line number and a SHORT, succint description. 

For example, the output should look like:

<example>
# replace `if adapter_type` with match

BEFORE: `crates/my-crate/src/my/file.rs:1234-1237`
```
if adapter_type == AdapterType::Foofing {
  the_code()
} else {
  other_code()
}
```

AFTER: `crates/my-crate/src/my/file.rs:1234-1237`
```
match adapter_type {
  AdapterType::Foofing => the_code(),
  _ => other_code(),
}
```

# move `foofing_do_something()` to `my_module::foofing()`

BEFORE: `crates/my-crate/src/generic_file.rs:1234-1237`
```
fn foofing_do_something() {
    // the code
}
```

AFTER: `crates/my-crate/src/generic_file.rs:1234-1237`
```
use my_module::foofing;

fn do_something(adapter_type: AdapterType) {
    match adapter_type {
        AdapterType::Foofing => foofing::do_something(),
        _ => unimplemented!(),
    }
}
```

AFTER: `crates/my-crate/src/my_module.rs`
```
fn do_something() {
    // the code
}
```
</example>


# Time to execute

Instructions done. Go execute.

The patch body is:

```
$ARGUMENTS[0]
```
