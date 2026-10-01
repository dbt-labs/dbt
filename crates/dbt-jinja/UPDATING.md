# Updating to MiniJinja 2

MiniJinja 2.0 is a major update to MiniJinja that changes a lot of core
internals and cleans up some APIs.  In particular it resolves some limitations
in the engine in relation to working with dynamic objects, unlocks potentials
for future performance improvements and enhancements.  This document helps with
upgrading to that version.

## Syntax Config

If you want to use custom delimiters, the way to configure this was slightly
changed to enable future improvements to this feature.

**Old:**

```rust
use minijinja::{Environment, Syntax};

let mut env = Environment::new();
env.set_syntax(minijinja::Syntax {
    block_start: "{".into(),
    block_end: "}".into(),
    variable_start: "${".into(),
    variable_end: "}".into(),
    comment_start: "{*".into(),
    comment_end: "*}".into(),
})
.unwrap();
```

**New:**

```rust
use minijinja::{Environment, syntax::SyntaxConfig};

let mut env = Environment::new();
env.set_syntax(
    SyntaxConfig::builder()
        .block_delimiters("{", "}")
        .variable_delimiters("${", "}")
        .comment_delimiters("{*", "*}")
        .build()
        .unwrap(),
);
```

## Iterators

In MiniJinja 1.x you could create iterators with the `Value::from_iterator`
function.  That same function is now called `Value::make_one_shot_iterator` and
the use is _discouraged_.  Instead most uses should instead use
`Value::make_iterable` which takes a function returning an iterator.  This has
the advantage that the value can be iterated over multiple times.

**Old:**

```rust
let value = Value::from_iterator(1..10);
```

**New Preferred:**

```rust
let value = Value::make_iterable(|| 1..10);
```

Additionally you can now also make iterables that borrow from other values
by using `Value::make_object_iterable`:

```rust
let value = Value::make_iterable(vec![1, 2, 3], |obj| {
    Box::new(obj.iter().map(|x| Value::from(*x * 2)))
});
```

## Objects

The largest change is the new object systems.  In MiniJinja 2, Objects are now
using an entire new trait.  (`Object`, `SeqObject` and `StructObject`) were
replaced by a single trait called `Object`.  It is however a completely new trait
unrelated to the old one, though it retains some common ideas.

In a nutshell:

* All method use `&Arc<Self>` instead of `&self` as receiver.  This allows one
  to clone out of the object when needed.
* `Object::kind` is gone and was replaced with `Object::repr` in spirit
* All trait methods are directly on the `Object` trait and the `StructObject`
  and `SeqObject` functionality is moved onto the object trait.
* Formatting is done via `Object::render` rather than `fmt::Display`.
* `fmt::Debug` is required in all cases now.
* Iteration now is implemented via `Object::enumerate`.

When working with objects of an unknown type, you can use the new `DynObject`
struct which is a type erased box over `Arc<Object>`.  `Value::as_object` now
returns an `Option<&DynObject>` compared to previously an `Option<&dyn Object>`
as an example.  The `DynObject` can be cheaply cloned which bumps the reference
count.

On the value type, the object related APIs were changed a bit to better
accommodate for the new trait:

* `Value::as_struct` was removed, use `Value::as_object` instead.
* `Value::as_seq` was removed, use `Value::as_object` instead.
* `Value::as_object` now returns a `Option<&DynObject>`.
* `Value::downcast_object` was added which returns an `Option<Arc<T>>`
* `ValueKind` is now non exhaustive and has more variants.

### Structs and Maps

Objects can now directly implement structs and maps.  That gives them greater
flexibility.  Because the receiver is an `Arc<Self>` we can also efficiently
borrow from them.

```rust
#[derive(Debug)]
struct User {
    username: String,
    roles: Vec<String>,
}
```

**Old:**

```rust
use minijinja::value::{Value, StructObject};

impl StructObject for User {
    fn get_field(&self, field: &str) -> Option<Value> {
        Some(match field {
            "username" => Value::from(&self.username),
            "roles" => Value::from(self.roles.clone()),
            _ => return None,
        })
    }

    fn static_fields(&self) -> Option<&'static [&'static str]> {
        Some(&["username", "roles"])
    }
}

let user = Value::from_struct_object(User { ... });
```

**New:**

The big changes are that `get_value` is now used instead of `get_field` and the
field that is looked up is a `&Value`.  To match on a string we need to call
`as_str()` on it.  For the `roles` here we can keep using the old pattern, or
use the more efficient `Value::make_object_iterable` which can borrow from the
object and make a lazy iterable.  For iteration an `Enumerator::Str` over all
keys is returned from `Object::enumerate`.

```rust
use std::sync::Arc;
use minijinja::value::{Value, Object, Enumerator};

impl Object for User {
    fn get_value(self: &Arc<Self>, field: &Value) -> Option<Value> {
        Some(match field.as_str()? {
            "username" => Value::from(&self.username),
            "roles" => Value::make_object_iterable(self.clone(), |o| {
                Box::new(o.roles.iter().map(Value::from))
            }),
            _ => return None,
        })
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Str(&["foo", "bar"])
    }
}

let value = Value::from_object(User { ... });
```

### Sequences

Sequences are now also just an `Object`.

```rust
#[derive(Debug)]
struct SimpleDynamicSeq([char; 4]);
```

**Old:**

```rust
use minijinja::value::SeqObject;

impl SeqObject for SimpleDynamicSeq {
    fn get_item(&self, idx: usize) -> Option<Value> {
        self.0.get(idx).copied().map(Value::from)
    }

    fn item_count(&self) -> usize {
        4
    }
}

let value = Value::from_seq_object(SimpleDynamicSeq(...));
```

**New:**

Because the default object representation is a `Map`, we need to
change it to `ObjectRepr::seq` in the `repr` method.  As sequences
iterate over their values, we can use the convenient `Enumerator::Seq`
enumerator which instructs the engine to sequentially iterate over
the object from `0` to the given `length`.  Otherwise the interface
is the same as with the map above, which means that rather than
implementing `get_item` you now also implement `get_value` which
replaces it.  To match over the index, use `as_usize()` on the value.

```rust
use minijinja::value::{Object, ObjectRepr, Enumerator, Value};

#[derive(Debug)]
struct SimpleDynamicSeq([char; 4]);

impl Object for SimpleDynamicSeq {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Seq
    }

    fn get_value(self: &Arc<Self>, idx: &Value) -> Option<Value> {
        self.0.get(idx.as_usize()?).copied().map(Value::from)
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Seq(self.0.len())
    }
}

let value = Value::from_object(SimpleDynamicSeq(...));
```

### Methods, Callables and Rendering

The interface for callables is largely unchanged other than the new
receiver.

**Old:**

```rust
use minijinja::{Error, ErrorKind};
use minijinja::value::Object;

#[derive(Debug)]
struct Markdown(String);

impl fmt::Display for Markdown {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", &self.0)
    }
}

impl Object for Markdown {
    fn call_method(
        &self,
        _state: &State,
        name: &str,
        args: &[Value],
    ) -> Result<Value, Error> {
        if name == "render" {
            // assert no arguments
            from_args(args)?;
            Ok(Value::from(render_markdown(&self.0)))
        } else {
            Err(Error::new(
                ErrorKind::UnknownMethod,
                format!("object has no method named {name}"),
            ))
        }
    }
}
```

**New:**

The replacement for `fmt::Display` is the new `Object::render` method.
If you implement it, it overrides the implied default.  Additionally
if you leave out the error message in the `UnknownMethod` error the
engine provides a useful one by default.

```rust
use minijinja::{Error, ErrorKind};
use minijinja::value::Object;

#[derive(Debug)]
struct Markdown(String);

impl Object for Markdown {
    fn call_method(
        self: &Arc<Self>,
        _state: &State,
        name: &str,
        args: &[Value],
    ) -> Result<Value, Error> {
        if name == "render" {
            from_args(args)?;
            Ok(Value::from(render_markdown(&self.0)))
        } else {
            Err(Error::from(ErrorKind::UnknownMethod))
        }
    }

    fn render(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", &self.0)
    }
}
```

## Stack Ref

The old `minijinja-stack-ref` module was removed as it can no longer accommodate the
new object model.  However that module largely is no longer useful as the new object
system is powerful enough to support it _for the most part_.  While it's not possible
any more to return references to objects on the stack, you can now trivially work with
reference counted externally held objects which should resolve a lot of the needs for
the stack-ref module.

For examples of how to do that, look at the new
[`object-ref`](https://github.com/mitsuhiko/minijinja/tree/main/examples/object-ref)
example that is modelled after the old
[`stack-ref`](https://github.com/mitsuhiko/minijinja/tree/1.0.16/examples/stack-ref)
example where you can see the differences between the two.

## Lazy Iterables

With MiniJinja 2 various things that were previously sequences, are now just iterables.
For instance using `|reverse` will only return an iterable, not a sequence.  This means
that you cannot index into this for instance.  On the other hand it performs better
and more efficiently.  The same is now true for slicing into things that are not strings
with the `[:]` operator.

If you do still want a list, you can force it into a list with the `|list` operator.
