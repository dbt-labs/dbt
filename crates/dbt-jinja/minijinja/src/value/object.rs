use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::hash::Hash;
use std::rc::Rc;
use std::sync::Arc;

use crate::error::{Error, ErrorKind};
use crate::listener::RenderingEventListener;
use crate::utils::UndefinedBehavior;
use crate::value::introspective::IntrospectiveValue;
use crate::value::{intern, intern_into_value, Value};
use crate::vm::State;

/// A trait that represents a dynamic object.
///
/// There is a type erased wrapper of this trait available called
/// [`DynObject`] which is what the engine actually holds internally.
///
/// # Basic Struct
///
/// The following example shows how to implement a dynamic object which
/// represents a struct.  All that's needed is to implement
/// [`get_value`](Self::get_value) to look up a field by name as well as
/// [`enumerate`](Self::enumerate) to return an enumerator over the known keys.
/// The [`repr`](Self::repr) defaults to `Map` so nothing needs to be done here.
///
/// ```
/// use std::sync::Arc;
/// use minijinja::value::{Value, Object, Enumerator};
///
/// #[derive(Debug)]
/// struct Point(f32, f32, f32);
///
/// impl Object for Point {
///     fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
///         match key.as_str()? {
///             "x" => Some(Value::from(self.0)),
///             "y" => Some(Value::from(self.1)),
///             "z" => Some(Value::from(self.2)),
///             _ => None,
///         }
///     }
///
///     fn enumerate(self: &Arc<Self>) -> Enumerator {
///         Enumerator::Str(&["x", "y", "z"])
///     }
/// }
///
/// let value = Value::from_object(Point(1.0, 2.5, 3.0));
/// ```
///
/// # Basic Sequence
///
/// The following example shows how to implement a dynamic object which
/// represents a sequence.  All that's needed is to implement
/// [`repr`](Self::repr) to indicate that this is a sequence,
/// [`get_value`](Self::get_value) to look up a field by index, and
/// [`enumerate`](Self::enumerate) to return a sequential enumerator.
/// This enumerator will automatically call `get_value` from `0..length`.
///
/// ```
/// use std::sync::Arc;
/// use minijinja::value::{Value, Object, ObjectRepr, Enumerator};
///
/// #[derive(Debug)]
/// struct Point(f32, f32, f32);
///
/// impl Object for Point {
///     fn repr(self: &Arc<Self>) -> ObjectRepr {
///         ObjectRepr::Seq
///     }
///
///     fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
///         match key.as_usize()? {
///             0 => Some(Value::from(self.0)),
///             1 => Some(Value::from(self.1)),
///             2 => Some(Value::from(self.2)),
///             _ => None,
///         }
///     }
///
///     fn enumerate(self: &Arc<Self>) -> Enumerator {
///         Enumerator::Seq(3)
///     }
/// }
///
/// let value = Value::from_object(Point(1.0, 2.5, 3.0));
/// ```
///
/// # Iterables
///
/// If you have something that is not quite a sequence but is capable of yielding
/// values over time, you can directly implement an iterable.  This is somewhat
/// uncommon as you can normally directly use [`Value::make_iterable`].  Here
/// is how this can be done though:
///
/// ```
/// use std::sync::Arc;
/// use minijinja::value::{Value, Object, ObjectRepr, Enumerator};
///
/// #[derive(Debug)]
/// struct Range10;
///
/// impl Object for Range10 {
///     fn repr(self: &Arc<Self>) -> ObjectRepr {
///         ObjectRepr::Iterable
///     }
///
///     fn enumerate(self: &Arc<Self>) -> Enumerator {
///         Enumerator::Iter(Box::new((1..10).map(Value::from)))
///     }
/// }
///
/// let value = Value::from_object(Range10);
/// ```
///
/// Iteration is encouraged to fail immediately (object is not iterable) or not at
/// all.  However this is not always possible, but the iteration interface itself
/// does not support fallible iteration.  It is however possible to accomplish the
/// same thing by creating an [invalid value](index.html#invalid-values).
///
/// # Map As Context
///
/// Map can also be used as template rendering context.  This has a lot of
/// benefits as it means that the serialization overhead can be largely to
/// completely avoided.  This means that even if templates take hundreds of
/// values, MiniJinja does not spend time eagerly converting them into values.
///
/// Here is a very basic example of how a template can be rendered with a dynamic
/// context.  Note that the implementation of [`enumerate`](Self::enumerate)
/// is optional for this to work.  It's in fact not used by the engine during
/// rendering but it is necessary for the [`debug()`](crate::functions::debug)
/// function to be able to show which values exist in the context.
///
/// ```
/// # fn main() -> Result<(), minijinja::Error> {
/// # use minijinja::Environment;
/// use std::sync::Arc;
/// use minijinja::value::{Value, Object};
/// use std::rc::Rc;
/// use minijinja::listener::DefaultRenderingEventListener;
///
/// #[derive(Debug)]
/// pub struct DynamicContext {
///     magic: i32,
/// }
///
/// impl Object for DynamicContext {
///     fn get_value(self: &Arc<Self>, field: &Value) -> Option<Value> {
///         match field.as_str()? {
///             "pid" => Some(Value::from(std::process::id())),
///             "env" => Some(Value::from_iter(std::env::vars())),
///             "magic" => Some(Value::from(self.magic)),
///             _ => None,
///         }
///     }
/// }
///
/// # let env = Environment::new();
/// let tmpl = env.template_from_str("HOME={{ env.HOME }}; PID={{ pid }}; MAGIC={{ magic }}")?;
/// let ctx = Value::from_object(DynamicContext { magic: 42 });
/// let rv = tmpl.render(ctx, &[Rc::new(DefaultRenderingEventListener::default())])?;
/// # Ok(()) }
/// ```
///
/// One thing of note here is that in the above example `env` would be re-created every
/// time the template needs it.  A better implementation would cache the value after it
/// was created first.
pub trait Object: fmt::Debug + Send + Sync {
    /// Indicates the natural representation of an object.
    ///
    /// The default implementation returns [`ObjectRepr::Map`].
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Map
    }

    /// Given a key, looks up the associated value.
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let _ = key;
        None
    }

    /// Enumerates the object.
    ///
    /// The engine uses the returned enumerator to implement iteration and
    /// the size information of an object.  For more information see
    /// [`Enumerator`].  The default implementation returns `Empty` for
    /// all object representations other than [`ObjectRepr::Plain`] which
    /// default to `NonEnumerable`.
    ///
    /// When wrapping other objects you might want to consider using
    /// [`ObjectExt::mapped_enumerator`] and [`ObjectExt::mapped_rev_enumerator`].
    fn enumerate(self: &Arc<Self>) -> Enumerator {
        match self.repr() {
            ObjectRepr::Plain => Enumerator::NonEnumerable,
            ObjectRepr::Iterable | ObjectRepr::Map | ObjectRepr::Seq => Enumerator::Empty,
        }
    }

    /// Returns the length of the enumerator.
    ///
    /// By default the length is taken by calling [`enumerate`](Self::enumerate) and
    /// inspecting the [`Enumerator`].  This means that in order to determine
    /// the length, an iteration is started.  If you think this is a problem for your
    /// uses, you can manually implement this.  This might for instance be
    /// needed if your type can only be iterated over once.
    fn enumerator_len(self: &Arc<Self>) -> Option<usize> {
        self.enumerate().query_len()
    }

    /// Returns `true` if this object is considered true for if conditions.
    ///
    /// The default implementation checks if the [`enumerator_len`](Self::enumerator_len)
    /// is not `Some(0)` which is the recommended behavior for objects.
    fn is_true(self: &Arc<Self>) -> bool {
        self.enumerator_len() != Some(0)
    }

    /// Returns `true` if this object is internally mutable.
    ///
    /// Objects are immutable by default. Mutable objects need to be
    /// specifically crafted.
    ///
    /// Mutability affects the rendering of sequence (`ObjectRepr::Seq`)
    /// objects: immutable sequences are rendered as tuples (e.g. "(1, 2)"),
    /// while mutable sequences are rendered as lists (e.g. "[1, 2]").
    fn is_mutable(self: &Arc<Self>) -> bool {
        false
    }

    /// Returns `true` if this value is a stand-in for an unknowable value
    /// produced by an introspective (warehouse-dependent) call evaluated
    /// without a real connection.
    ///
    /// Defaults to `false` for every object. A single wrapper type is
    /// expected to override this to `true`; the VM checks this flag at a
    /// small number of choke points (emit, loop iteration, branching, binary
    /// operators) to decide whether to substitute a hole marker instead of
    /// treating the value as real.
    fn is_introspective_stub(self: &Arc<Self>) -> bool {
        false
    }

    /// Compares this object with another object by value when supported.
    fn custom_cmp(self: &Arc<Self>, other: &DynObject) -> Option<Ordering> {
        let _ = other;
        None
    }

    /// The engine calls this to invoke the object itself.
    ///
    /// The default implementation returns an
    /// [`InvalidOperation`](crate::ErrorKind::InvalidOperation) error.
    fn call(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        let (_, _) = (state, args);
        Err(Error::new(
            ErrorKind::InvalidOperation,
            "object is not callable",
        ))
    }

    /// The engine calls this to invoke a method on the object.
    ///
    /// The default implementation returns an
    /// [`UnknownMethod`](crate::ErrorKind::UnknownMethod) error.  When this error
    /// is returned the engine will invoke the
    /// [`unknown_method_callback`](crate::Environment::set_unknown_method_callback) of
    /// the environment.
    fn call_method(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        method: &str,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        if let Some(value) = self.get_value(&Value::from(method)) {
            return value.call(state, args, listeners);
        }

        Err(Error::from(ErrorKind::UnknownMethod))
    }

    /// Try to get a property from this object dynamically.
    ///
    /// Override this method if your type supports dynamic property access.
    /// Returns `None` by default, meaning the normal attribute lookup path will be used.
    fn get_property(
        self: &Arc<Self>,
        _state: &State<'_, '_>,
        _name: &str,
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        Err(Error::new(
            ErrorKind::InvalidOperation,
            "object does not support get_property",
        ))
    }

    /// Formats the object for stringification.
    ///
    /// The default implementation is specific to the behavior of
    /// [`repr`](Self::repr) and usually does not need modification.
    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result
    where
        Self: Sized + 'static,
    {
        match self.repr() {
            ObjectRepr::Map => {
                let mut dbg = f.debug_map();
                for (key, value) in self.try_iter_pairs().into_iter().flatten() {
                    dbg.entry(&key, &value);
                }
                dbg.finish()
            }
            // for either sequences or iterables, a length is needed, otherwise we
            // don't want to risk iteration during printing and fall back to the
            // debug print.
            ObjectRepr::Seq | ObjectRepr::Iterable
                if self.is_mutable() && self.enumerator_len().is_some() =>
            {
                let mut dbg = f.debug_list();
                for value in self.try_iter().into_iter().flatten() {
                    dbg.entry(&value);
                }
                dbg.finish()
            }
            ObjectRepr::Seq | ObjectRepr::Iterable
                if !self.is_mutable() && self.enumerator_len().is_some() =>
            {
                // Check if the tuple is empty
                if self.enumerator_len() == Some(0) {
                    return write!(f, "()");
                }

                let mut dbg = f.debug_tuple("");
                for value in self.try_iter().into_iter().flatten() {
                    dbg.field(&value);
                }
                dbg.finish()
            }
            _ => {
                write!(f, "{self:?}")
            }
        }
    }
}

macro_rules! impl_object_helpers {
    ($vis:vis $self_ty: ty) => {
        /// Iterates over this object.
        ///
        /// If this returns `None` then the default object iteration as defined by
        /// the object's `enumeration` is used.
        $vis fn try_iter(self: $self_ty) -> Option<Box<dyn Iterator<Item = Value> + Send + Sync>>
        where
            Self: 'static,
        {
            match self.enumerate() {
                Enumerator::NonEnumerable => None,
                Enumerator::Empty => Some(Box::new(None::<Value>.into_iter())),
                Enumerator::Seq(l) => {
                    let self_clone = self.clone();
                    Some(Box::new((0..l).map(move |idx| {
                        self_clone.get_value(&Value::from(idx)).unwrap_or_default()
                    })))
                }
                Enumerator::Iter(iter) => Some(iter),
                Enumerator::RevIter(iter) => Some(Box::new(iter)),
                Enumerator::Str(s) => Some(Box::new(s.iter().copied().map(intern_into_value))),
                Enumerator::Values(v) => Some(Box::new(v.into_iter())),
            }
        }

        /// Iterate over key and value at once.
        $vis fn try_iter_pairs(
            self: $self_ty,
        ) -> Option<Box<dyn Iterator<Item = (Value, Value)> + Send + Sync>> {
            let iter = some!(self.try_iter());
            let repr = self.repr();
            let self_clone = self.clone();
            Some(Box::new(iter.enumerate().map(move |(idx, item)| {
                match repr {
                    ObjectRepr::Map => {
                        let value = self_clone.get_value(&item);
                        (item, value.unwrap_or_default())
                    }
                    _ => (Value::from(idx), item)
                }
            })))
        }
    };
}

/// Provides utility methods for working with objects.
pub trait ObjectExt: Object + Send + Sync + 'static {
    /// Creates a new iterator enumeration that projects into the given object.
    ///
    /// It takes a method that is passed a reference to `self` and is expected
    /// to return an [`Iterator`].  This iterator is then wrapped in an
    /// [`Enumerator::Iter`].  This allows one to create an iterator that borrows
    /// out of the object.
    ///
    /// # Example
    ///
    /// ```
    /// # use std::collections::HashMap;
    /// use std::sync::Arc;
    /// use minijinja::value::{Value, Object, ObjectExt, Enumerator};
    ///
    /// #[derive(Debug)]
    /// struct CustomMap(HashMap<usize, i64>);
    ///
    /// impl Object for CustomMap {
    ///     fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
    ///         self.0.get(&key.as_usize()?).copied().map(Value::from)
    ///     }
    ///
    ///     fn enumerate(self: &Arc<Self>) -> Enumerator {
    ///         self.mapped_enumerator(|this| {
    ///             Box::new(this.0.keys().copied().map(Value::from))
    ///         })
    ///     }
    /// }
    /// ```
    fn mapped_enumerator<F>(self: &Arc<Self>, maker: F) -> Enumerator
    where
        F: for<'a> FnOnce(&'a Self) -> Box<dyn Iterator<Item = Value> + Send + Sync + 'a>
            + Send
            + Sync
            + 'static,
        Self: Sized,
    {
        struct IterObject<T> {
            iter: Box<dyn Iterator<Item = Value> + Send + Sync + 'static>,
            _object: Arc<T>,
        }

        impl<T> Iterator for IterObject<T> {
            type Item = Value;

            fn next(&mut self) -> Option<Self::Item> {
                self.iter.next()
            }

            fn size_hint(&self) -> (usize, Option<usize>) {
                self.iter.size_hint()
            }
        }

        // SAFETY: this is safe because the `IterObject` will keep our object alive.
        let iter = unsafe {
            std::mem::transmute::<
                Box<dyn Iterator<Item = _>>,
                Box<dyn Iterator<Item = _> + Send + Sync>,
            >(maker(self))
        };
        let object = self.clone();
        Enumerator::Iter(Box::new(IterObject {
            iter,
            _object: object,
        }))
    }

    /// Creates a new reversible iterator enumeration that projects into the given object.
    ///
    /// It takes a method that is passed a reference to `self` and is expected
    /// to return a [`DoubleEndedIterator`].  This iterator is then wrapped in an
    /// [`Enumerator::RevIter`].  This allows one to create an iterator that borrows
    /// out of the object and is reversible.
    ///
    /// # Example
    ///
    /// ```
    /// # use std::collections::HashMap;
    /// use std::sync::Arc;
    /// use std::ops::Range;
    /// use minijinja::value::{Value, Object, ObjectExt, ObjectRepr, Enumerator};
    ///
    /// #[derive(Debug)]
    /// struct VecView(Vec<usize>);
    ///
    /// impl Object for VecView {
    ///     fn repr(self: &Arc<Self>) -> ObjectRepr {
    ///         ObjectRepr::Iterable
    ///     }
    ///
    ///     fn enumerate(self: &Arc<Self>) -> Enumerator {
    ///         self.mapped_enumerator(|this| {
    ///             Box::new(this.0.iter().cloned().map(Value::from))
    ///         })
    ///     }
    /// }
    /// ```
    fn mapped_rev_enumerator<F>(self: &Arc<Self>, maker: F) -> Enumerator
    where
        F: for<'a> FnOnce(
                &'a Self,
            )
                -> Box<dyn DoubleEndedIterator<Item = Value> + Send + Sync + 'a>
            + Send
            + Sync
            + 'static,
        Self: Sized,
    {
        struct IterObject<T> {
            iter: Box<dyn DoubleEndedIterator<Item = Value> + Send + Sync + 'static>,
            _object: Arc<T>,
        }

        impl<T> Iterator for IterObject<T> {
            type Item = Value;

            fn next(&mut self) -> Option<Self::Item> {
                self.iter.next()
            }

            fn size_hint(&self) -> (usize, Option<usize>) {
                self.iter.size_hint()
            }
        }

        impl<T> DoubleEndedIterator for IterObject<T> {
            fn next_back(&mut self) -> Option<Self::Item> {
                self.iter.next_back()
            }
        }

        // SAFETY: this is safe because the `IterObject` will keep our object alive.
        let iter = unsafe {
            std::mem::transmute::<
                Box<dyn DoubleEndedIterator<Item = _>>,
                Box<dyn DoubleEndedIterator<Item = _> + Send + Sync>,
            >(maker(self))
        };
        let object = self.clone();
        Enumerator::RevIter(Box::new(IterObject {
            iter,
            _object: object,
        }))
    }

    impl_object_helpers!(&Arc<Self>);
}

impl<T: Object + Send + Sync + 'static> ObjectExt for T {}

/// Enumerators help define iteration behavior for [`Object`]s.
///
/// When Jinja wants to know the length of an object, if it's empty or
/// not or if it wants to iterate over it, it will ask the [`Object`] to
/// enumerate itself with the [`enumerate`](Object::enumerate) method.  The
/// returned enumerator has enough information so that the object can be
/// iterated over, but it does not necessarily mean that iteration actually
/// starts or that it has the data to yield the right values.
///
/// In fact, you should never inspect an enumerator.  You can create it or
/// forward it.  For actual iteration use [`ObjectExt::try_iter`] etc.
#[non_exhaustive]
pub enum Enumerator {
    /// Marks non enumerable objects.
    ///
    /// Such objects cannot be iterated over, the length is unknown which
    /// means they are not considered empty by the engine.  This is a good
    /// choice for plain objects.
    ///
    /// | Iterable | Length  |
    /// |----------|---------|
    /// | no       | unknown |
    NonEnumerable,

    /// The empty enumerator.  It yields no elements.
    ///
    /// | Iterable | Length      |
    /// |----------|-------------|
    /// | yes      | known (`0`) |
    Empty,

    /// A slice of static strings.
    ///
    /// This is a useful enumerator to enumerate the attributes of an
    /// object or the keys in a string hash map.
    ///
    /// | Iterable | Length       |
    /// |----------|--------------|
    /// | yes      | known        |
    Str(&'static [&'static str]),

    /// A dynamic iterator over values.
    ///
    /// The length is known if the [`Iterator::size_hint`] has matching lower
    /// and upper bounds.  The logic used by the engine is the following:
    ///
    /// ```
    /// # let iter = Some(1).into_iter();
    /// let len = match iter.size_hint() {
    ///     (lower, Some(upper)) if lower == upper => Some(lower),
    ///     _ => None
    /// };
    /// ```
    ///
    /// Because the engine prefers repeatable iteration, it will keep creating
    /// new enumerators every time the iteration should restart.  Sometimes
    /// that might not always be possible (eg: you stream data in) in which
    /// case
    ///
    /// | Iterable | Length          |
    /// |----------|-----------------|
    /// | yes      | sometimes known |
    Iter(Box<dyn Iterator<Item = Value> + Send + Sync>),

    /// Like `Iter` but supports efficient reversing.
    ///
    /// This means that the iterator has to be of type [`DoubleEndedIterator`].
    ///
    /// | Iterable | Length          |
    /// |----------|-----------------|
    /// | yes      | sometimes known |
    RevIter(Box<dyn DoubleEndedIterator<Item = Value> + Send + Sync>),

    /// Indicates sequential iteration.
    ///
    /// This instructs the engine to iterate over an object by enumerating it
    /// from `0` to `n` by calling [`Object::get_value`].  This is essentially the
    /// way sequences are supposed to be enumerated.
    ///
    /// | Iterable | Length          |
    /// |----------|-----------------|
    /// | yes      | known           |
    Seq(usize),

    /// A vector of known values to iterate over.
    ///
    /// The iterator will yield each value in the vector one after another.
    ///
    /// | Iterable | Length          |
    /// |----------|-----------------|
    /// | yes      | known           |
    Values(Vec<Value>),
}

/// Defines the natural representation of this object.
///
/// An [`ObjectRepr`] is a reduced form of
/// [`ValueKind`](crate::value::ValueKind) which only contains value which can
/// be represented by objects.  For instance an object can never be a primitive
/// and as such those kinds are unavailable.
///
/// The representation influences how values are serialized, stringified or
/// what kind they report.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ObjectRepr {
    /// An object that has no reasonable representation.
    ///
    /// - **Default Render:** [`Debug`]
    /// - **Collection Behavior:** none
    /// - **Iteration Behavior:** none
    /// - **Serialize:** [`Debug`] / [`render`](Object::render) output as string
    Plain,

    /// Represents a map or object.
    ///
    /// - **Default Render:** `{key: value,...}` pairs
    /// - **Collection Behavior:** looks like a map, can be indexed by key, has a length
    /// - **Iteration Behavior:** iterates over keys
    /// - **Serialize:** Serializes as map
    Map,

    /// Represents a sequence (eg: array/list).
    ///
    /// - **Default Render:** `[value,...]`
    /// - **Collection Behavior:** looks like a list, can be indexed by index, has a length
    /// - **Iteration Behavior:** iterates over values
    /// - **Serialize:** Serializes as list
    Seq,

    /// Represents a non indexable, iterable object.
    ///
    /// - **Default Render:** `[value,...]` (if length is known), `"<iterator>"` otherwise.
    /// - **Collection Behavior:** looks like a list if length is known, cannot be indexed
    /// - **Iteration Behavior:** iterates over values
    /// - **Serialize:** Serializes as list
    Iterable,
}

type_erase! {
    pub trait Object => DynObject {
        fn repr(&self) -> ObjectRepr;

        fn get_value(&self, key: &Value) -> Option<Value>;

        fn enumerate(&self) -> Enumerator;

        fn is_true(&self) -> bool;

        fn is_mutable(&self) -> bool;

        fn is_introspective_stub(&self) -> bool;

        fn custom_cmp(&self, other: &DynObject) -> Option<Ordering>;

        fn enumerator_len(&self) -> Option<usize>;

        fn call(
            &self,
            state: &State<'_, '_>,
            args: &[Value],
            listeners: &[Rc<dyn RenderingEventListener>]
        ) -> Result<Value, Error>;

        fn call_method(
            &self,
            state: &State<'_, '_>,
            method: &str,
            args: &[Value],
            listeners: &[Rc<dyn RenderingEventListener>]
        ) -> Result<Value, Error>;

        fn get_property(
            &self,
            state: &State<'_, '_>,
            name: &str,
            listeners: &[Rc<dyn RenderingEventListener>]
        ) -> Result<Value, Error>;

        fn render(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result;

        impl fmt::Debug {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result;
        }
    }
}

unsafe impl Send for DynObject {}
unsafe impl Sync for DynObject {}

impl DynObject {
    impl_object_helpers!(pub &Self);

    /// Checks if this dyn object is the same as another.
    pub(crate) fn is_same_object(&self, other: &DynObject) -> bool {
        self.ptr == other.ptr && self.vtable == other.vtable
    }

    pub(crate) fn identity_cmp(&self, other: &DynObject) -> Ordering {
        self.type_name()
            .cmp(other.type_name())
            .then_with(|| self.ptr.cmp(&other.ptr))
    }
}

impl Hash for DynObject {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        if let Some(iter) = self.try_iter_pairs() {
            for (key, value) in iter {
                key.hash(state);
                value.hash(state);
            }
        }
    }
}

impl fmt::Display for DynObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.render(f)
    }
}

impl Enumerator {
    fn query_len(&self) -> Option<usize> {
        Some(match self {
            Enumerator::Empty => 0,
            Enumerator::Values(v) => v.len(),
            Enumerator::Str(v) => v.len(),
            Enumerator::Iter(i) => match i.size_hint() {
                (a, Some(b)) if a == b => a,
                _ => return None,
            },
            Enumerator::RevIter(i) => match i.size_hint() {
                (a, Some(b)) if a == b => a,
                _ => return None,
            },
            Enumerator::Seq(v) => *v,
            Enumerator::NonEnumerable => return None,
        })
    }
}

macro_rules! impl_value_vec {
    ($vec_type:ident) => {
        impl<T> Object for $vec_type<T>
        where
            T: Into<Value> + Clone + Send + Sync + fmt::Debug + 'static,
        {
            fn repr(self: &Arc<Self>) -> ObjectRepr {
                ObjectRepr::Seq
            }

            fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
                self.get(some!(key.as_usize())).cloned().map(|v| v.into())
            }

            fn enumerate(self: &Arc<Self>) -> Enumerator {
                Enumerator::Seq(self.len())
            }
        }

        impl<T> From<$vec_type<T>> for Value
        where
            T: Into<Value> + Clone + Send + Sync + fmt::Debug + 'static,
        {
            fn from(val: $vec_type<T>) -> Self {
                Value::from_object(val)
            }
        }
    };
}

macro_rules! impl_value_iterable {
    ($iterable_type:ident, $enumerator:ident) => {
        impl<T> Object for $iterable_type<T>
        where
            T: Into<Value> + Clone + Send + Sync + fmt::Debug + 'static,
        {
            fn repr(self: &Arc<Self>) -> ObjectRepr {
                ObjectRepr::Iterable
            }

            fn enumerate(self: &Arc<Self>) -> Enumerator {
                self.clone()
                    .$enumerator(|this| Box::new(this.iter().map(|x| x.clone().into())))
            }
        }

        impl<T> From<$iterable_type<T>> for Value
        where
            T: Into<Value> + Clone + Send + Sync + fmt::Debug + 'static,
        {
            fn from(val: $iterable_type<T>) -> Self {
                Value::from_object(val)
            }
        }
    };
}

/// Method names that must never be resolved via dict key-lookup on Map values.
///
/// Python treats dict methods (`.items()`, `.keys()`, `.values()`, `.get()`) as
/// attributes on the class, so user keys with the same name cannot shadow them.
/// MiniJinja's default `call_method` does key-lookup-as-method-dispatch, which
/// is convenient for Plain "namespace" objects (function/macro dispatch) but
/// breaks dict parity. The bundled Map `Object` impls and `MutableMap` call
/// `is_reserved_dict_method` to defer reserved names to the environment's
/// `unknown_method_callback` (typically `pycompat::unknown_method_callback`).
///
/// `reserved_dict_method_name` is the same lookup but returns the interned
/// `'static` name so callers (e.g. `Value::get_attr_fast` constructing a
/// [`BoundMethod`]) can avoid allocating.
pub(crate) const fn reserved_dict_method_name(name: &str) -> Option<&'static str> {
    match name.as_bytes() {
        b"items" => Some("items"),
        b"keys" => Some("keys"),
        b"values" => Some("values"),
        b"get" => Some("get"),
        _ => None,
    }
}

pub(crate) const fn is_reserved_dict_method(name: &str) -> bool {
    reserved_dict_method_name(name).is_some()
}

/// A bound dict method (e.g. `d.items` accessed as a bare attribute).
///
/// Python parity: `d.items` is the bound method on the dict class, distinct
/// from `d['items']` (the user value). MiniJinja's default attribute access
/// on a Map does `get_value(name)`, which would shadow the method with the
/// user value. When a reserved name is accessed via `.attr` syntax on a Map,
/// `Value::get_attr_fast` returns a `BoundMethod` whose `call` dispatches
/// back through `receiver.call_method(...)` — which, for reserved names,
/// falls through to the environment's `unknown_method_callback` (pycompat).
#[derive(Debug)]
pub(crate) struct BoundMethod {
    receiver: Value,
    method: &'static str,
}

impl BoundMethod {
    pub(crate) fn new(receiver: Value, method: &'static str) -> Self {
        Self { receiver, method }
    }
}

impl Object for BoundMethod {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    fn call(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        self.receiver
            .call_method(state, self.method, args, listeners)
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Mirrors Python's `<built-in method items of dict object at ...>` debug
        // string at a coarse level. Templates that stringify a method object
        // are doing something weird; this gives a recognizable hint.
        write!(
            f,
            "<bound method {} of {}>",
            self.method,
            self.receiver.kind()
        )
    }
}

macro_rules! impl_str_map_helper {
    ($map_type:ident, $key_type:ty, $enumerator:ident) => {
        impl<V> Object for $map_type<$key_type, V>
        where
            V: Into<Value> + Clone + Send + Sync + fmt::Debug + 'static,
        {
            fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
                self.get(some!(key.as_str())).cloned().map(|v| v.into())
            }

            fn enumerate(self: &Arc<Self>) -> Enumerator {
                self.$enumerator(|this| {
                    Box::new(this.keys().map(|x| intern_into_value(x.as_ref())))
                })
            }

            fn enumerator_len(self: &Arc<Self>) -> Option<usize> {
                Some(self.len())
            }

            fn call_method(
                self: &Arc<Self>,
                state: &State<'_, '_>,
                method: &str,
                args: &[Value],
                listeners: &[Rc<dyn RenderingEventListener>],
            ) -> Result<Value, Error> {
                // Reserved Python dict methods (items / keys / values / get) take
                // precedence over user keys. Any other name still falls back to
                // key lookup so Jinja macro imports and namespace-style dispatch
                // (where macros are stored by name) keep working.
                if is_reserved_dict_method(method) {
                    return Err(Error::from(ErrorKind::UnknownMethod));
                }
                if let Some(value) = self.get_value(&Value::from(method)) {
                    return value.call(state, args, listeners);
                }
                Err(Error::from(ErrorKind::UnknownMethod))
            }
        }
    };
}

macro_rules! impl_str_map {
    ($map_type:ident, $enumerator:ident) => {
        impl_str_map_helper!($map_type, String, $enumerator);
        impl_str_map_helper!($map_type, Arc<str>, $enumerator);

        impl<V> From<$map_type<String, V>> for Value
        where
            V: Into<Value> + Send + Sync + Clone + fmt::Debug + 'static,
        {
            fn from(val: $map_type<String, V>) -> Self {
                Value::from_object(val)
            }
        }

        impl<V> From<$map_type<Arc<str>, V>> for Value
        where
            V: Into<Value> + Send + Sync + Clone + fmt::Debug + 'static,
        {
            fn from(val: $map_type<Arc<str>, V>) -> Self {
                Value::from_object(val)
            }
        }

        impl<'a, V> From<$map_type<&'a str, V>> for Value
        where
            V: Into<Value> + Send + Sync + Clone + fmt::Debug + 'static,
        {
            fn from(val: $map_type<&'a str, V>) -> Self {
                Value::from(
                    val.into_iter()
                        .map(|(k, v)| (intern(k), v))
                        .collect::<$map_type<Arc<str>, V>>(),
                )
            }
        }

        impl<'a, V> From<$map_type<Cow<'a, str>, V>> for Value
        where
            V: Into<Value> + Send + Sync + Clone + fmt::Debug + 'static,
        {
            fn from(val: $map_type<Cow<'a, str>, V>) -> Self {
                Value::from(
                    val.into_iter()
                        .map(|(k, v)| {
                            (
                                match k {
                                    Cow::Borrowed(s) => intern(s),
                                    Cow::Owned(s) => Arc::<str>::from(s),
                                },
                                v,
                            )
                        })
                        .collect::<$map_type<Arc<str>, V>>(),
                )
            }
        }
    };
}

macro_rules! impl_value_map {
    ($map_type:ident, $enumerator:ident) => {
        impl<V> Object for $map_type<Value, V>
        where
            V: Into<Value> + Clone + Send + Sync + fmt::Debug + 'static,
        {
            fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
                self.get(key).cloned().map(|v| v.into())
            }

            fn enumerate(self: &Arc<Self>) -> Enumerator {
                self.$enumerator(|this| Box::new(this.keys().cloned()))
            }

            fn enumerator_len(self: &Arc<Self>) -> Option<usize> {
                Some(self.len())
            }

            fn call_method(
                self: &Arc<Self>,
                state: &State<'_, '_>,
                method: &str,
                args: &[Value],
                listeners: &[Rc<dyn RenderingEventListener>],
            ) -> Result<Value, Error> {
                // Reserved Python dict methods (items / keys / values / get) take
                // precedence over user keys. Any other name still falls back to
                // key lookup so Jinja macro imports and namespace-style dispatch
                // (where macros are stored by name) keep working.
                if is_reserved_dict_method(method) {
                    return Err(Error::from(ErrorKind::UnknownMethod));
                }
                if let Some(value) = self.get_value(&Value::from(method)) {
                    return value.call(state, args, listeners);
                }
                Err(Error::from(ErrorKind::UnknownMethod))
            }
        }

        impl<V> From<$map_type<Value, V>> for Value
        where
            V: Into<Value> + Send + Sync + Clone + fmt::Debug + 'static,
        {
            fn from(val: $map_type<Value, V>) -> Self {
                Value::from_object(val)
            }
        }
    };
}

impl_value_vec!(Vec);
impl_value_map!(BTreeMap, mapped_rev_enumerator);
impl_str_map!(BTreeMap, mapped_rev_enumerator);

#[cfg(feature = "std_collections")]
mod std_collections_impls {
    use super::*;
    use std::collections::{BTreeSet, HashMap, HashSet, LinkedList, VecDeque};

    impl_value_iterable!(LinkedList, mapped_rev_enumerator);
    impl_value_iterable!(HashSet, mapped_enumerator);
    impl_value_iterable!(BTreeSet, mapped_rev_enumerator);
    impl_str_map!(HashMap, mapped_enumerator);
    impl_value_map!(HashMap, mapped_enumerator);
    impl_value_vec!(VecDeque);
}

#[cfg(feature = "preserve_order")]
mod preserve_order_impls {
    use super::*;
    use indexmap::IndexMap;

    impl_value_map!(IndexMap, mapped_rev_enumerator);
    impl_str_map!(IndexMap, mapped_rev_enumerator);
}

/// This module contains a mutable vector implementation that can be used as a
/// drop-in replacement for a [Vec<Value>] to allow mutation in user macros.
pub mod mutable_vec {
    use std::sync::RwLock;

    use crate::value::ValueRepr;

    use super::*;

    macro_rules! lock_write {
        ($self:ident) => {
            $self.inner.write().expect("lock poisoned")
        };
    }

    macro_rules! lock_read {
        ($self:ident) => {
            $self.inner.read().expect("lock poisoned")
        };
    }

    /// An interior-mutable vector.
    ///
    /// A [MutableVec<Value>] can be used as a drop-in replacement for a
    /// [Vec<Value>] to allow mutation in user macros. However, please note that
    /// due to locking, this object incurs a constant overhead on *all*
    /// operations, including read-only ones. Therefore, it is recommended to
    /// use this only when necessary. In particular, try to avoid sharing this
    /// object across thread boundaries, even though it is `Send` and `Sync`.
    #[derive(Debug)]
    pub struct MutableVec<T> {
        inner: RwLock<Vec<T>>,
        // Set once an introspective-stub value (see `Object::is_introspective_stub`)
        // is ever pushed/inserted into this vector, and never cleared. A
        // container built up across a loop (`{% do include_cols.append(col)
        // %}`) is otherwise indistinguishable from an ordinary one once even
        // a single tainted element has been absorbed into it -- the taint on
        // that one element doesn't make the *container* report tainted on
        // its own, so a later `{{ include_cols }}`/`.join()`/hash of the
        // whole container would silently look fully-known. This flag is
        // read by `is_introspective_stub` below.
        tainted: std::sync::atomic::AtomicBool,
    }

    impl<T> MutableVec<T>
    where
        T: Into<Value> + Clone + Send + Sync + fmt::Debug + 'static,
    {
        /// Creates a new empty [MutableVec].
        pub fn new() -> Self {
            MutableVec {
                inner: RwLock::new(Vec::new()),
                tainted: std::sync::atomic::AtomicBool::new(false),
            }
        }

        /// Creates a new empty [MutableVec], pre-allocating the specified
        /// capacity.
        pub fn with_capacity(capacity: usize) -> Self {
            MutableVec {
                inner: RwLock::new(Vec::with_capacity(capacity)),
                tainted: std::sync::atomic::AtomicBool::new(false),
            }
        }

        /// Append a value to the end of the vector.
        pub fn push(&self, value: T) {
            lock_write!(self).push(value);
        }

        /// Extend the vector with the contents of an iterable.
        pub fn extend(&self, iter: impl IntoIterator<Item = T>) {
            lock_write!(self).extend(iter);
        }

        /// Reverse the vector in place.
        pub fn reverse(&self) {
            lock_write!(self).reverse();
        }

        /// Pop a value from the end of the vector.
        pub fn pop(&self, args: &[Value]) -> Option<T> {
            let mut inner = lock_write!(self);
            if args.is_empty() {
                return inner.pop();
            }
            let raw_idx = some!(args[0].as_i64());
            let len = inner.len();
            let idx = if raw_idx < 0 {
                if len == 0 {
                    return None;
                }
                (len as i64 + raw_idx) as usize
            } else if raw_idx > len as i64 {
                return None;
            } else {
                raw_idx as usize
            };
            if idx < len {
                Some(inner.remove(idx))
            } else {
                None
            }
        }

        /// Insert a value at a given index, moving subsequent elements to the
        /// right.
        pub fn insert(&self, idx: usize, value: T) {
            lock_write!(self).insert(idx, value);
        }

        /// Remove a value at a given index, shifting subsequent elements to the
        /// left.
        pub fn remove(&self, value: &T) -> Result<T, Error>
        where
            T: PartialEq<T>,
        {
            let mut inner = lock_write!(self);
            if let Some(idx) = inner.iter().position(|x| x == value) {
                Ok(inner.remove(idx))
            } else {
                Err(Error::new(
                    ErrorKind::InvalidOperation,
                    "remove() value not found in list",
                ))
            }
        }

        /// Remove all elements from the vector.
        pub fn clear(&self) {
            lock_write!(self).clear();
        }

        /// Check if the vector contains a value.
        pub fn contains(&self, value: &T) -> bool
        where
            T: PartialEq<T>,
        {
            lock_read!(self).contains(value)
        }
    }

    impl Object for MutableVec<Value> {
        fn repr(self: &Arc<Self>) -> ObjectRepr {
            ObjectRepr::Seq
        }

        fn is_mutable(self: &Arc<Self>) -> bool {
            true
        }

        fn is_introspective_stub(self: &Arc<Self>) -> bool {
            self.tainted.load(std::sync::atomic::Ordering::Relaxed)
        }

        fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
            lock_read!(self).get(some!(key.as_usize())).cloned()
        }

        fn enumerate(self: &Arc<Self>) -> Enumerator {
            // This is a simple naive implementation which incurs a read-lock
            // overhead *on each element*, as Enumerator::Seq would call into
            // get_value for each index.  This is not ideal, however since we
            // don't expect the lock to be contentious, the overhead (one atomic
            // read) should be acceptable.
            //
            // Furthermore, this implementation is, in fact, unsound -- the
            // length of the vector could get changed from under our feet before
            // the Enumerator is fully consumed. One may be tempted to hold a
            // read-lock for the duration of Enumerator, but that could easily
            // cause deadlocks if the user code tries to mutate the vector while
            // iterating, a much worse outcome than potentially getting some
            // Value::Undefined values in the iteration. Note that this current
            // behavior is also *no worse* than the mutation semantics in
            // Jinja/Python.
            //
            // Fundamentally, the idea of unconstrained mutation is just
            // incompatible with the design of Minijinja. The eventual solution
            // to this is to disallow, or at least greatly constrain the
            // mutation semantics at the language level.
            Enumerator::Seq(lock_read!(self).len())
        }

        fn call_method(
            self: &Arc<Self>,
            _state: &State<'_, '_>,
            method: &str,
            args: &[Value],
            _listeners: &[Rc<dyn RenderingEventListener>],
        ) -> Result<Value, Error> {
            match method {
                "append" => append_impl(self, args),
                "extend" => extend_impl(self, args),
                "pop" => {
                    if args.len() > 1 {
                        return Err(Error::new(
                            ErrorKind::TooManyArguments,
                            "pop() takes at most 1 argument",
                        ));
                    }
                    Ok(self.pop(args).unwrap_or_default())
                }
                "insert" => insert_impl(self, args),
                "remove" => remove_impl(self, args),
                "clear" => {
                    if !args.is_empty() {
                        return Err(Error::new(
                            ErrorKind::TooManyArguments,
                            "clear() takes no arguments",
                        ));
                    }
                    self.clear();
                    Ok(Value::default())
                }
                "sort" => sort_impl(self, args),
                "copy" => copy_impl(self, args),
                _ => Err(Error::new(
                    ErrorKind::UnknownMethod,
                    format!("MutableVec has no method named {method}"),
                )),
            }
        }
    }

    impl<T> Default for MutableVec<T>
    where
        T: Into<Value> + Clone + Send + Sync + fmt::Debug + 'static,
    {
        fn default() -> Self {
            Self::new()
        }
    }

    impl From<MutableVec<Value>> for Value {
        fn from(val: MutableVec<Value>) -> Self {
            Value::from_object(val)
        }
    }

    impl<T> From<Vec<T>> for MutableVec<Value>
    where
        T: Into<Value> + Clone + Send + Sync + fmt::Debug + 'static,
    {
        fn from(val: Vec<T>) -> Self {
            let values: Vec<Value> = val.into_iter().map(Into::into).collect();
            let tainted = values.iter().any(|v| v.is_introspective_stub());
            MutableVec {
                inner: RwLock::new(values),
                tainted: std::sync::atomic::AtomicBool::new(tainted),
            }
        }
    }

    impl<T> FromIterator<T> for MutableVec<Value>
    where
        T: Into<Value> + Clone + Send + Sync + fmt::Debug + 'static,
    {
        fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
            let values: Vec<Value> = iter.into_iter().map(Into::into).collect();
            let tainted = values.iter().any(|v| v.is_introspective_stub());
            MutableVec {
                inner: RwLock::new(values),
                tainted: std::sync::atomic::AtomicBool::new(tainted),
            }
        }
    }

    impl<T> From<MutableVec<T>> for Vec<T> {
        fn from(val: MutableVec<T>) -> Self {
            val.inner.into_inner().expect("lock poisoned")
        }
    }

    fn append_impl(vec: &Arc<MutableVec<Value>>, args: &[Value]) -> Result<Value, Error> {
        match args {
            [value] => {
                if value.is_introspective_stub() {
                    vec.tainted
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
                vec.push(value.clone());
                Ok(ValueRepr::None.into())
            }
            _ if args.len() > 1 => Err(Error::new(
                ErrorKind::TooManyArguments,
                format!(
                    "append() takes exactly one argument, but {} were given",
                    args.len()
                ),
            )),
            _ => Err(Error::new(
                ErrorKind::MissingArgument,
                "append() takes exactly one argument, but none were given",
            )),
        }
    }

    fn extend_impl(vec: &Arc<MutableVec<Value>>, args: &[Value]) -> Result<Value, Error> {
        match args {
            [iter] => {
                if iter.is_introspective_stub() {
                    vec.tainted
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
                let iter = ok!(iter
                    .as_object()
                    .and_then(|x| x.try_iter())
                    .ok_or_else(||
                        Error::new(
                            ErrorKind::CannotUnpack,
                            "extend() expects an iterable as argument, but given argument is not iterable"
                    )));

                let items: Vec<Value> = iter.collect();
                if items.iter().any(|v| v.is_introspective_stub()) {
                    vec.tainted
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
                vec.extend(items);
                Ok(Value::from_dyn_object(vec.clone()))
            }
            _ if args.len() > 1 => Err(Error::new(
                ErrorKind::TooManyArguments,
                format!(
                    "extend() takes exactly one argument, but {} were given",
                    args.len()
                ),
            )),
            _ => Err(Error::new(
                ErrorKind::MissingArgument,
                "extend() takes exactly one argument, but none were given",
            )),
        }
    }

    fn insert_impl(vec: &Arc<MutableVec<Value>>, args: &[Value]) -> Result<Value, Error> {
        match args {
            [idx, value] => {
                let idx = ok!(idx.as_usize().ok_or_else(|| Error::new(
                    ErrorKind::InvalidOperation,
                    "insert() expects an integer as first argument"
                )));
                let mut inner = lock_write!(vec);
                let idx = idx.min(inner.len());
                if value.is_introspective_stub() {
                    vec.tainted
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
                inner.insert(idx, value.clone());
                drop(inner);
                Ok(Value::from_dyn_object(vec.clone()))
            }
            _ if args.len() > 2 => Err(Error::new(
                ErrorKind::TooManyArguments,
                format!(
                    "insert() takes exactly two arguments, but {} were given",
                    args.len()
                ),
            )),
            _ => Err(Error::new(
                ErrorKind::MissingArgument,
                format!(
                    "insert() takes exactly two arguments, but only {} were given",
                    args.len()
                ),
            )),
        }
    }

    fn remove_impl(vec: &Arc<MutableVec<Value>>, args: &[Value]) -> Result<Value, Error> {
        match args {
            [value] => match vec.remove(value) {
                Ok(removed) => Ok(removed),
                Err(_)
                    if vec.tainted.load(std::sync::atomic::Ordering::Relaxed)
                        || value.is_introspective_stub() =>
                {
                    // `append`/`insert`/`extend` mark the container tainted the
                    // moment a fabricated (introspective-stub) item enters it
                    // (see e.g. `append_impl` above), because from then on the
                    // container's real contents can't be trusted to match what
                    // template logic assumes is in there. A `remove()` the
                    // caller "knows" should succeed can therefore legitimately
                    // find nothing to remove -- degrade like every other
                    // introspective-stub operation (see
                    // `IntrospectiveValue::call`/`call_method`) instead of
                    // raising a hard render error over fabricated data.
                    Ok(IntrospectiveValue::wrap_leaf(Value::UNDEFINED))
                }
                Err(err) => Err(err),
            },
            _ if args.len() > 1 => Err(Error::new(
                ErrorKind::TooManyArguments,
                format!(
                    "remove() takes exactly one argument, but {} were given",
                    args.len()
                ),
            )),
            _ => Err(Error::new(
                ErrorKind::MissingArgument,
                "remove() takes exactly one argument, but none were given",
            )),
        }
    }

    fn sort_impl(vec: &Arc<MutableVec<Value>>, args: &[Value]) -> Result<Value, Error> {
        // example
        // {% do table_cols.sort() %}
        // {% do table_cols.sort(True) %}
        // {% do table_cols.sort(reverse=True) %}

        if args.len() > 1 {
            return Err(Error::new(
                ErrorKind::TooManyArguments,
                format!(
                    "sort() takes at most one argument, but {} were given",
                    args.len()
                ),
            ));
        }
        let reverse = if !args.is_empty() && args[0].is_kwargs() {
            let kwargs = args[0].as_object().unwrap();
            if let Some(value) = kwargs.get_value(&Value::from("reverse")) {
                value.is_true()
            } else {
                Err(Error::new(
                    ErrorKind::InvalidOperation,
                    "sort() expects a boolean value for reverse",
                ))?
            }
        } else {
            args.get(0).is_some_and(|v| v.is_true())
        };

        vec.inner.write().expect("lock poisoned").sort_by(|a, b| {
            if reverse {
                b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
            }
        });

        Ok(Value::default())
    }

    fn copy_impl(vec: &Arc<MutableVec<Value>>, args: &[Value]) -> Result<Value, Error> {
        if !args.is_empty() {
            return Err(Error::new(
                ErrorKind::TooManyArguments,
                "copy() takes no arguments",
            ));
        }
        let copy = vec.inner.read().expect("lock poisoned").clone();
        Ok(Value::from_object(MutableVec::from(copy)))
    }
}

/// This module contains a mutable map implementation that can be used as a
/// drop-in replacement for a [ValueMap] to allow mutation in user macros.
pub mod mutable_map {
    use std::sync::RwLock;

    use crate::arg_utils::ArgParser;
    use crate::value::{value_map_with_capacity, ValueKind, ValueMap};

    use super::*;

    macro_rules! lock_write {
        ($self:ident) => {
            $self.inner.write().expect("lock poisoned")
        };
    }

    macro_rules! lock_read {
        ($self:ident) => {
            $self.inner.read().expect("lock poisoned")
        };
    }

    /// An interior-mutable map.
    ///
    /// A [MutableMap] can be used as a drop-in replacement for a
    /// [ValueMap] to allow mutation in user macros. However, please
    /// note that due to locking, this object incurs a constant overhead on *all*
    /// operations, including read-only ones. Therefore, it is recommended to use
    /// this only when necessary. In particular, try to avoid sharing this object
    /// across thread boundaries, even though it is `Send` and `Sync`.
    #[derive(Debug)]
    pub struct MutableMap {
        inner: RwLock<ValueMap>,
        // See `MutableVec`'s `tainted` field for why this exists: a value
        // absorbed via `.update()`/`.insert()`/`setdefault()` can be
        // individually tainted without the map itself reporting tainted,
        // silently hiding unknowable data from a later `{{ map }}`/hash/etc.
        tainted: std::sync::atomic::AtomicBool,
    }

    impl Clone for MutableMap {
        fn clone(&self) -> Self {
            MutableMap {
                inner: RwLock::new(lock_read!(self).clone()),
                tainted: std::sync::atomic::AtomicBool::new(
                    self.tainted.load(std::sync::atomic::Ordering::Relaxed),
                ),
            }
        }
    }

    impl MutableMap {
        /// Creates a new empty [MutableMap].
        pub fn new() -> Self {
            MutableMap {
                inner: RwLock::new(ValueMap::new()),
                tainted: std::sync::atomic::AtomicBool::new(false),
            }
        }

        /// Creates a new empty [MutableMap], pre-allocating the specified capacity.
        pub fn with_capacity(capacity: usize) -> Self {
            MutableMap {
                inner: RwLock::new(value_map_with_capacity(capacity)),
                tainted: std::sync::atomic::AtomicBool::new(false),
            }
        }

        /// Get a value corresponding to the given `key`` from the map.
        pub fn get(&self, key: &Value) -> Option<Value> {
            lock_read!(self).get(key).cloned()
        }

        /// Check if the map contains the given `key`.
        pub fn contains(&self, key: &Value) -> bool {
            lock_read!(self).contains_key(key)
        }

        /// Return all keys in the map.
        pub fn keys(&self) -> Vec<Value> {
            lock_read!(self).keys().cloned().collect()
        }

        /// Insert a key-value pair into the map.
        pub fn insert(&self, key: Value, value: Value) {
            if key.is_introspective_stub() || value.is_introspective_stub() {
                self.tainted
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            lock_write!(self).insert(key, value);
        }

        /// Update the map with the contents of another map.
        pub fn update(&self, other: &ValueMap) {
            if other
                .iter()
                .any(|(k, v)| k.is_introspective_stub() || v.is_introspective_stub())
            {
                self.tainted
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            lock_write!(self).extend(other.clone());
        }

        #[cfg(feature = "preserve_order")]
        /// Remove a key-value pair from the map.
        pub fn remove(&self, key: &Value) -> Option<Value> {
            lock_write!(self).swap_remove(key)
        }

        #[cfg(not(feature = "preserve_order"))]
        /// Remove a key-value pair from the map.
        pub fn remove(&self, key: &Value) -> Option<Value> {
            lock_write!(self).remove(key)
        }

        /// Clear all key-value pairs from the map.
        pub fn clear(&self) {
            lock_write!(self).clear();
        }
    }

    impl Object for MutableMap {
        fn repr(self: &Arc<Self>) -> ObjectRepr {
            ObjectRepr::Map
        }

        fn is_mutable(self: &Arc<Self>) -> bool {
            true
        }

        fn is_introspective_stub(self: &Arc<Self>) -> bool {
            self.tainted.load(std::sync::atomic::Ordering::Relaxed)
        }

        fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
            self.get(key)
        }

        fn enumerate(self: &Arc<Self>) -> Enumerator {
            // Same limitations as [MutableVec::enumerate] applies here. See
            // comments there for more details.
            self.mapped_rev_enumerator(|this| Box::new(this.keys().into_iter()))
        }

        fn enumerator_len(self: &Arc<Self>) -> Option<usize> {
            Some(self.keys().len())
        }

        fn call_method(
            self: &Arc<Self>,
            state: &State<'_, '_>,
            method: &str,
            args: &[Value],
            listeners: &[Rc<dyn RenderingEventListener>],
        ) -> Result<Value, Error> {
            match method {
                "update" => update_impl(state, self, args),
                "pop" => pop_impl(self, args),
                "popitem" => popitem_impl(self, args),
                "clear" => {
                    if !args.is_empty() {
                        return Err(Error::new(
                            ErrorKind::TooManyArguments,
                            "clear() takes no arguments",
                        ));
                    }
                    self.clear();
                    Ok(Value::default())
                }
                "copy" => {
                    if !args.is_empty() {
                        return Err(Error::new(
                            ErrorKind::TooManyArguments,
                            "copy() takes no arguments",
                        ));
                    }
                    let copy = lock_read!(self).clone();
                    Ok(Value::from_object(MutableMap::from(copy)))
                }
                "setdefault" => setdefault_impl(self, args),
                _ => {
                    // Reserved Python dict methods bypass key lookup so the
                    // environment's `unknown_method_callback` (pycompat) can
                    // handle them. Other names still fall back to key lookup so
                    // Jinja macro imports keep working (macros are stored in the
                    // namespace by name and invoked via this path).
                    if !is_reserved_dict_method(method) {
                        if let Some(value) = self.get(&Value::from(method)) {
                            return value.call(state, args, listeners);
                        }
                    }
                    Err(Error::new(
                        ErrorKind::UnknownMethod,
                        format!("MutableMap has no method named {method}"),
                    ))
                }
            }
        }
    }

    impl Default for MutableMap {
        fn default() -> Self {
            Self::new()
        }
    }

    impl From<MutableMap> for Value {
        fn from(val: MutableMap) -> Self {
            Value::from_object(val)
        }
    }

    impl From<ValueMap> for MutableMap {
        fn from(val: ValueMap) -> Self {
            let tainted = val
                .iter()
                .any(|(k, v)| k.is_introspective_stub() || v.is_introspective_stub());
            MutableMap {
                inner: RwLock::new(val),
                tainted: std::sync::atomic::AtomicBool::new(tainted),
            }
        }
    }

    impl From<MutableMap> for ValueMap {
        fn from(val: MutableMap) -> Self {
            val.inner.into_inner().expect("lock poisoned")
        }
    }

    fn update_impl(
        state: &State<'_, '_>,
        map: &Arc<MutableMap>,
        args: &[Value],
    ) -> Result<Value, Error> {
        let parser = ArgParser::new(args, None);
        if parser.positional_len() > 1 {
            return Err(Error::new(
                ErrorKind::TooManyArguments,
                format!(
                    "update() takes at most one positional argument, but {} were given",
                    parser.positional_len()
                ),
            ));
        }

        let mut entries = ValueMap::new();
        if let Some(other) = parser.get_args_as_vec_of_values().first() {
            let is_allow_all = state.env().undefined_behavior() == UndefinedBehavior::AllowAll;
            if other.kind() == ValueKind::Map {
                if let Some(pairs) = other.as_object().and_then(|obj| obj.try_iter_pairs()) {
                    entries.extend(pairs);
                }
            } else if other.kind() == ValueKind::Undefined && is_allow_all {
                // Under `AllowAll` (dbt parse), dbt-common's Undefined is an
                // empty iterable that chains attribute/call access, so
                // `d.update(missing, owner='x')` applies the kwargs without
                // error instead of raising like Python's `dict.update(None)`.
            } else if matches!(other.kind(), ValueKind::None | ValueKind::Undefined) {
                // `Value::try_iter` treats None/Undefined as an empty
                // iterator for template convenience, but Python's
                // `dict.update(None)` raises TypeError rather than no-op.
                return Err(Error::new(
                    ErrorKind::CannotUnpack,
                    format!(
                        "update() expects a mapping or iterable of pairs, not {}",
                        other.kind()
                    ),
                ));
            } else {
                let iter = other.try_iter().map_err(|_| {
                    Error::new(
                        ErrorKind::CannotUnpack,
                        "update() expects a mapping or iterable of pairs",
                    )
                })?;
                for item in iter {
                    let mut pair = item.try_iter().map_err(|_| {
                        Error::new(
                            ErrorKind::CannotUnpack,
                            "update() sequence element is not iterable",
                        )
                    })?;
                    let key = pair.next().ok_or_else(|| {
                        Error::new(
                            ErrorKind::CannotUnpack,
                            "update() sequence element must have length 2",
                        )
                    })?;
                    let value = pair.next().ok_or_else(|| {
                        Error::new(
                            ErrorKind::CannotUnpack,
                            "update() sequence element must have length 2",
                        )
                    })?;
                    if pair.next().is_some() {
                        return Err(Error::new(
                            ErrorKind::CannotUnpack,
                            "update() sequence element must have length 2",
                        ));
                    }
                    entries.insert(key, value);
                }
            }
        }

        // `ArgParser` collects kwargs into a `BTreeMap`, which would sort them
        // alphabetically; Python (and dbt-common) preserves the caller's
        // keyword order, so pull the trailing kwargs value's pairs directly
        // instead of going through the parser for this part.
        if let Some(kwargs) = args.iter().find(|arg| arg.is_kwargs()) {
            if let Some(pairs) = kwargs.as_object().and_then(|obj| obj.try_iter_pairs()) {
                entries.extend(pairs);
            }
        }
        map.update(&entries);
        Ok(Value::from_dyn_object(map.clone()))
    }

    fn pop_impl(map: &Arc<MutableMap>, args: &[Value]) -> Result<Value, Error> {
        match args {
            [key] => Ok(map.remove(key).unwrap_or_default()),
            [key, default] => Ok(map.remove(key).unwrap_or_else(|| default.clone())),
            _ if args.len() > 2 => Err(Error::new(
                ErrorKind::TooManyArguments,
                format!(
                    "pop() takes one or two arguments, but {} were given",
                    args.len()
                ),
            )),
            _ => Err(Error::new(
                ErrorKind::MissingArgument,
                "pop() takes one or two arguments, but none were given",
            )),
        }
    }

    fn popitem_impl(map: &Arc<MutableMap>, args: &[Value]) -> Result<Value, Error> {
        if !args.is_empty() {
            return Err(Error::new(
                ErrorKind::TooManyArguments,
                "popitem() takes no arguments",
            ));
        }

        let key = map
            .keys()
            .last()
            .cloned()
            .ok_or_else(|| Error::new(ErrorKind::InvalidOperation, "no items to pop"))?;

        let value = map.remove(&key).unwrap_or_default();

        Ok(Value::from_iter([key, value]))
    }

    fn setdefault_impl(map: &Arc<MutableMap>, args: &[Value]) -> Result<Value, Error> {
        match args {
            [key, value] => {
                if let Some(existing_value) = map.get(key) {
                    return Ok(existing_value);
                }
                map.insert(key.clone(), value.clone());
                Ok(value.clone())
            }
            _ => Err(Error::new(
                ErrorKind::MissingArgument,
                "set_default() takes exactly two arguments, but none were given",
            )),
        }
    }
}

/// This module contains a mutable set implementation for Python-like set operations.
pub mod mutable_set {
    use std::collections::BTreeSet;
    use std::sync::RwLock;

    use super::*;
    use crate::value::from_args;

    macro_rules! lock_write {
        ($self:ident) => {
            $self.inner.write().expect("lock poisoned")
        };
    }

    macro_rules! lock_read {
        ($self:ident) => {
            $self.inner.read().expect("lock poisoned")
        };
    }

    /// An interior-mutable set.
    ///
    /// A [MutableSet] provides Python-like set operations with interior mutability.
    /// It wraps a `BTreeSet<Value>` and provides methods like `add`, `remove`,
    /// `union`, `intersection`, etc.
    #[derive(Debug)]
    pub struct MutableSet {
        inner: RwLock<BTreeSet<Value>>,
    }

    impl Clone for MutableSet {
        fn clone(&self) -> Self {
            MutableSet {
                inner: RwLock::new(lock_read!(self).clone()),
            }
        }
    }

    impl MutableSet {
        /// Creates a new empty [MutableSet].
        pub fn new() -> Self {
            MutableSet {
                inner: RwLock::new(BTreeSet::new()),
            }
        }

        /// Add an element to the set.
        pub fn add(&self, value: Value) {
            lock_write!(self).insert(value);
        }

        /// Remove an element from the set. Raises error if not present.
        pub fn remove(&self, value: &Value) -> Result<(), Error> {
            lock_write!(self)
                .remove(value)
                .then_some(())
                .ok_or_else(|| {
                    Error::new(ErrorKind::InvalidOperation, format!("KeyError: {value}"))
                })
        }

        /// Remove an element from the set if present. Does not raise error.
        pub fn discard(&self, value: &Value) {
            lock_write!(self).remove(value);
        }

        /// Remove and return an arbitrary element from the set.
        pub fn pop(&self) -> Result<Value, Error> {
            let mut inner = lock_write!(self);
            inner
                .pop_first()
                .ok_or_else(|| Error::new(ErrorKind::InvalidOperation, "pop from an empty set"))
        }

        /// Remove all elements from the set.
        pub fn clear(&self) {
            lock_write!(self).clear();
        }

        /// Return a shallow copy of the set.
        pub fn copy(&self) -> MutableSet {
            MutableSet {
                inner: RwLock::new(lock_read!(self).clone()),
            }
        }

        /// Check if the set contains a value.
        pub fn contains(&self, value: &Value) -> bool {
            lock_read!(self).contains(value)
        }

        /// Return the number of elements in the set.
        pub fn len(&self) -> usize {
            lock_read!(self).len()
        }

        /// Check if the set is empty.
        pub fn is_empty(&self) -> bool {
            lock_read!(self).is_empty()
        }

        /// Return the union of the set and another iterable.
        pub fn union(&self, other: impl IntoIterator<Item = Value>) -> MutableSet {
            let self_set = lock_read!(self);
            let result = self_set
                .union(&other.into_iter().collect())
                .cloned()
                .collect();
            MutableSet {
                inner: RwLock::new(result),
            }
        }

        /// Return the intersection of the set and another iterable.
        pub fn intersection(&self, other: impl IntoIterator<Item = Value>) -> MutableSet {
            let other_set: BTreeSet<Value> = other.into_iter().collect();
            let self_set = lock_read!(self);
            let result = self_set.intersection(&other_set).cloned().collect();
            MutableSet {
                inner: RwLock::new(result),
            }
        }

        /// Return the difference of the set and another iterable.
        pub fn difference(&self, other: impl IntoIterator<Item = Value>) -> MutableSet {
            let other_set: BTreeSet<Value> = other.into_iter().collect();
            let self_set = lock_read!(self);
            let result = self_set.difference(&other_set).cloned().collect();
            MutableSet {
                inner: RwLock::new(result),
            }
        }

        /// Return the symmetric difference of the set and another iterable.
        pub fn symmetric_difference(&self, other: impl IntoIterator<Item = Value>) -> MutableSet {
            let other_set: BTreeSet<Value> = other.into_iter().collect();
            let self_set = lock_read!(self);
            let result = self_set.symmetric_difference(&other_set).cloned().collect();
            MutableSet {
                inner: RwLock::new(result),
            }
        }

        /// Update the set with the union of itself and another iterable.
        pub fn update(&self, other: impl IntoIterator<Item = Value>) {
            lock_write!(self).extend(other);
        }

        /// Update the set with the intersection of itself and another iterable.
        pub fn intersection_update(&self, other: impl IntoIterator<Item = Value>) {
            let other_set: BTreeSet<Value> = other.into_iter().collect();
            lock_write!(self).retain(|v| other_set.contains(v));
        }

        /// Update the set with the difference of itself and another iterable.
        pub fn difference_update(&self, other: impl IntoIterator<Item = Value>) {
            let other_set: BTreeSet<Value> = other.into_iter().collect();
            lock_write!(self).retain(|v| !other_set.contains(v));
        }

        /// Update the set with the symmetric difference of itself and another iterable.
        pub fn symmetric_difference_update(&self, other: impl IntoIterator<Item = Value>) {
            let other_set: BTreeSet<Value> = other.into_iter().collect();
            let mut inner = lock_write!(self);
            let to_remove: Vec<Value> = inner
                .iter()
                .filter(|v| other_set.contains(v))
                .cloned()
                .collect();
            for v in &to_remove {
                inner.remove(v);
            }
            for v in other_set {
                if !to_remove.contains(&v) {
                    inner.insert(v);
                }
            }
        }

        /// Return True if the set is a subset of another.
        pub fn issubset(&self, other: impl IntoIterator<Item = Value>) -> bool {
            let other_set: BTreeSet<Value> = other.into_iter().collect();
            lock_read!(self).is_subset(&other_set)
        }

        /// Return True if the set is a superset of another.
        pub fn issuperset(&self, other: impl IntoIterator<Item = Value>) -> bool {
            let other_set: BTreeSet<Value> = other.into_iter().collect();
            lock_read!(self).is_superset(&other_set)
        }

        /// Return True if the set has no elements in common with another.
        pub fn isdisjoint(&self, other: impl IntoIterator<Item = Value>) -> bool {
            let other_set: BTreeSet<Value> = other.into_iter().collect();
            lock_read!(self).is_disjoint(&other_set)
        }
    }

    impl Default for MutableSet {
        fn default() -> Self {
            Self::new()
        }
    }

    impl<T> FromIterator<T> for MutableSet
    where
        T: Into<Value>,
    {
        fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
            MutableSet {
                inner: RwLock::new(iter.into_iter().map(Into::into).collect()),
            }
        }
    }

    impl From<MutableSet> for Value {
        fn from(val: MutableSet) -> Self {
            Value::from_object(val)
        }
    }

    impl Object for MutableSet {
        fn repr(self: &Arc<Self>) -> ObjectRepr {
            ObjectRepr::Seq
        }

        fn is_mutable(self: &Arc<Self>) -> bool {
            true
        }

        fn get_value(self: &Arc<Self>, _key: &Value) -> Option<Value> {
            // Sets don't support indexing
            None
        }

        fn enumerate(self: &Arc<Self>) -> Enumerator {
            Enumerator::Iter(Box::new(
                lock_read!(self)
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .into_iter(),
            ))
        }

        fn call_method(
            self: &Arc<Self>,
            _state: &State<'_, '_>,
            method: &str,
            args: &[Value],
            _listeners: &[Rc<dyn RenderingEventListener>],
        ) -> Result<Value, Error> {
            match method {
                "add" => {
                    let (value,): (Value,) = from_args(args)?;
                    self.add(value);
                    Ok(Value::default())
                }
                "remove" => {
                    let (value,): (&Value,) = from_args(args)?;
                    self.remove(value)?;
                    Ok(Value::default())
                }
                "discard" => {
                    let (value,): (&Value,) = from_args(args)?;
                    self.discard(value);
                    Ok(Value::default())
                }
                "pop" => {
                    let () = from_args(args)?;
                    self.pop()
                }
                "clear" => {
                    let () = from_args(args)?;
                    self.clear();
                    Ok(Value::default())
                }
                "copy" => {
                    let () = from_args(args)?;
                    Ok(Value::from_object(self.copy()))
                }
                "union" => {
                    // Support multiple arguments like Python's set.union(*others)
                    let mut result = lock_read!(self).clone();
                    for arg in args {
                        let iter = arg.try_iter().map_err(|_| {
                            Error::new(
                                ErrorKind::InvalidOperation,
                                "union() argument must be iterable",
                            )
                        })?;
                        result.extend(iter);
                    }
                    Ok(Value::from_object(MutableSet {
                        inner: RwLock::new(result),
                    }))
                }
                "intersection" => {
                    let (other,): (&Value,) = from_args(args)?;
                    let iter = other.try_iter().map_err(|_| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            "intersection() argument must be iterable",
                        )
                    })?;
                    Ok(Value::from_object(self.intersection(iter)))
                }
                "difference" => {
                    let (other,): (&Value,) = from_args(args)?;
                    let iter = other.try_iter().map_err(|_| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            "difference() argument must be iterable",
                        )
                    })?;
                    Ok(Value::from_object(self.difference(iter)))
                }
                "symmetric_difference" => {
                    let (other,): (&Value,) = from_args(args)?;
                    let iter = other.try_iter().map_err(|_| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            "symmetric_difference() argument must be iterable",
                        )
                    })?;
                    Ok(Value::from_object(self.symmetric_difference(iter)))
                }
                "update" => {
                    let (other,): (&Value,) = from_args(args)?;
                    let iter = other.try_iter().map_err(|_| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            "update() argument must be iterable",
                        )
                    })?;
                    self.update(iter);
                    Ok(Value::default())
                }
                "intersection_update" => {
                    let (other,): (&Value,) = from_args(args)?;
                    let iter = other.try_iter().map_err(|_| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            "intersection_update() argument must be iterable",
                        )
                    })?;
                    self.intersection_update(iter);
                    Ok(Value::default())
                }
                "difference_update" => {
                    let (other,): (&Value,) = from_args(args)?;
                    let iter = other.try_iter().map_err(|_| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            "difference_update() argument must be iterable",
                        )
                    })?;
                    self.difference_update(iter);
                    Ok(Value::default())
                }
                "symmetric_difference_update" => {
                    let (other,): (&Value,) = from_args(args)?;
                    let iter = other.try_iter().map_err(|_| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            "symmetric_difference_update() argument must be iterable",
                        )
                    })?;
                    self.symmetric_difference_update(iter);
                    Ok(Value::default())
                }
                "issubset" => {
                    let (other,): (&Value,) = from_args(args)?;
                    let iter = other.try_iter().map_err(|_| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            "issubset() argument must be iterable",
                        )
                    })?;
                    Ok(Value::from(self.issubset(iter)))
                }
                "issuperset" => {
                    let (other,): (&Value,) = from_args(args)?;
                    let iter = other.try_iter().map_err(|_| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            "issuperset() argument must be iterable",
                        )
                    })?;
                    Ok(Value::from(self.issuperset(iter)))
                }
                "isdisjoint" => {
                    let (other,): (&Value,) = from_args(args)?;
                    let iter = other.try_iter().map_err(|_| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            "isdisjoint() argument must be iterable",
                        )
                    })?;
                    Ok(Value::from(self.isdisjoint(iter)))
                }
                _ => Err(Error::new(
                    ErrorKind::UnknownMethod,
                    format!("'set' has no method named '{}'", method),
                )),
            }
        }
    }
}
