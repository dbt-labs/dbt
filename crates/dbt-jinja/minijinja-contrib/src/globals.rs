use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use minijinja::listener::RenderingEventListener;
use minijinja::value::mutable_map::MutableMap;
#[allow(unused)]
use minijinja::value::Value;
use minijinja::value::{from_args, Kwargs, Object, ObjectRepr, Rest, ValueKind};
use minijinja::{Error, ErrorKind, State};

/// Returns the current time in UTC as unix timestamp.
///
/// To format this timestamp, use the [`datetimeformat`](crate::filters::datetimeformat) filter.
#[cfg(feature = "datetime")]
#[cfg_attr(docsrs, doc(cfg(feature = "datetime")))]
pub fn now() -> Value {
    let now = time::OffsetDateTime::now_utc();
    Value::from(((now.unix_timestamp_nanos() / 1000) as f64) / 1_000_000.0)
}

/// Returns a cycler.
///
/// Similar to `loop.cycle`, but can be used outside loops or across
/// multiple loops. For example, render a list of folders and files in a
/// list, alternating giving them "odd" and "even" classes.
///
/// ```jinja
/// {% set row_class = cycler("odd", "even") %}
/// <ul class="browser">
/// {% for folder in folders %}
///   <li class="folder {{ row_class.next() }}">{{ folder }}
/// {% endfor %}
/// {% for file in files %}
///   <li class="file {{ row_class.next() }}">{{ file }}
/// {% endfor %}
/// </ul>
/// ```
pub fn cycler(items: Rest<Value>) -> Result<Value, Error> {
    #[derive(Debug)]
    pub struct Cycler {
        items: Vec<Value>,
        pos: AtomicUsize,
    }

    impl Object for Cycler {
        fn repr(self: &Arc<Self>) -> ObjectRepr {
            ObjectRepr::Plain
        }

        fn call_method(
            self: &Arc<Self>,
            _state: &State<'_, '_>,
            method: &str,
            args: &[Value],
            _listeners: &[Rc<dyn RenderingEventListener>],
        ) -> Result<Value, Error> {
            match method {
                "next" => {
                    let () = from_args(args)?;
                    let idx = self.pos.load(Ordering::Relaxed);
                    self.pos
                        .store((idx + 1) % self.items.len(), Ordering::Relaxed);
                    Ok(self.items[idx].clone())
                }
                _ => Err(Error::new(
                    ErrorKind::UnknownMethod,
                    format!("Cycler object has no method named {method}"),
                )),
            }
        }
    }

    if items.is_empty() {
        Err(Error::new(
            ErrorKind::InvalidOperation,
            "at least one value required",
        ))
    } else {
        Ok(Value::from_object(Cycler {
            items: items.0,
            pos: AtomicUsize::new(0),
        }))
    }
}

/// Build the callable `dict` namespace so templates can write
/// `{{ dict(...) }}` and `{{ dict.fromkeys(...) }}` the same way they would
/// in Python's Jinja2.
///
/// Exposes:
/// - `dict()` / `dict([(k, v), ...])` / `dict({...})` / `dict(**kwargs)` — constructors.
/// - `dict.fromkeys(iterable, value=None)` — class method.
pub fn create_dict_namespace() -> Value {
    Value::from_object(DictNamespace)
}

#[derive(Debug)]
struct DictNamespace;

impl Object for DictNamespace {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            "fromkeys" => Some(Value::from_function(dict_fromkeys)),
            _ => None,
        }
    }

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
        self.call(state, args, listeners)
    }

    fn call(
        self: &Arc<Self>,
        _state: &State<'_, '_>,
        args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        // Mirrors CPython's `dict(*args, **kwargs)`: at most one positional
        // arg (a mapping or an iterable of key/value pairs), then keyword
        // arguments, with kwargs overriding any keys from the positional.
        let (data, kwargs): (Option<&Value>, Kwargs) = from_args(args)?;
        let map = MutableMap::new();

        if let Some(data) = data {
            match data.kind() {
                ValueKind::Map => {
                    if let Some(pairs) = data.as_object().and_then(|o| o.try_iter_pairs()) {
                        for (k, v) in pairs {
                            map.insert(k, v);
                        }
                    }
                }
                _ => {
                    let iter = data.try_iter().map_err(|_| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            format!("'{}' object is not iterable", data.python_type_name()),
                        )
                    })?;
                    for (idx, item) in iter.enumerate() {
                        let pair_iter = item.try_iter().map_err(|_| {
                            Error::new(
                                ErrorKind::InvalidOperation,
                                format!(
                                    "cannot convert dictionary update sequence element #{idx} to a sequence"
                                ),
                            )
                        })?;
                        let pair: Vec<Value> = pair_iter.collect();
                        if pair.len() != 2 {
                            return Err(Error::new(
                                ErrorKind::InvalidOperation,
                                format!(
                                    "dictionary update sequence element #{idx} has length {}; 2 is required",
                                    pair.len()
                                ),
                            ));
                        }
                        let mut pair = pair.into_iter();
                        let k = pair.next().unwrap();
                        let v = pair.next().unwrap();
                        map.insert(k, v);
                    }
                }
            }
        }

        let kwarg_keys: Vec<String> = kwargs.args().map(|s| s.to_string()).collect();
        for key in kwarg_keys {
            let v: Value = kwargs.get(&key)?;
            map.insert(Value::from(key), v);
        }
        kwargs.assert_all_used()?;

        Ok(Value::from_object(map))
    }
}

/// `dict.fromkeys(iterable, value=None)` — build a dict whose keys come from
/// `iterable`, all mapped to `value` (or `None` if omitted). Mirrors Python,
/// including CPython's `TypeError: '<typename>' object is not iterable`
/// on a non-iterable argument so SLT goldens cross-validate against Mantle.
fn dict_fromkeys(args: &[Value]) -> Result<Value, Error> {
    let (keys, default): (&Value, Option<&Value>) = from_args(args)?;
    let iter = keys.try_iter().map_err(|_| {
        Error::new(
            ErrorKind::InvalidOperation,
            format!("'{}' object is not iterable", keys.python_type_name()),
        )
    })?;
    let default = default.cloned().unwrap_or_else(|| Value::from(()));
    let map = MutableMap::new();
    for key in iter {
        map.insert(key, default.clone());
    }
    Ok(Value::from_object(map))
}

/// A tiny helper that can be used to “join” multiple sections.  A
/// joiner is passed a string and will return that string every time
/// it’s called, except the first time (in which case it returns an
/// empty string). You can use this to join things:
///
/// ```jinja
/// {% set pipe = joiner("|") %}
/// {% if categories %} {{ pipe() }}
/// Categories: {{ categories|join(", ") }}
/// {% endif %}
/// {% if author %} {{ pipe() }}
/// Author: {{ author() }}
/// {% endif %}
/// {% if can_edit %} {{ pipe() }}
/// <a href="?action=edit">Edit</a>
/// {% endif %}
/// ```
pub fn joiner(sep: Option<Value>) -> Value {
    #[derive(Debug)]
    struct Joiner {
        sep: Value,
        used: AtomicBool,
    }

    impl Object for Joiner {
        fn repr(self: &Arc<Self>) -> ObjectRepr {
            ObjectRepr::Plain
        }

        fn call(
            self: &Arc<Self>,
            _state: &State<'_, '_>,
            args: &[Value],
            _listeners: &[Rc<dyn RenderingEventListener>],
        ) -> Result<Value, Error> {
            let () = from_args(args)?;
            let used = self.used.swap(true, Ordering::Relaxed);
            if used {
                Ok(self.sep.clone())
            } else {
                Ok(Value::from(""))
            }
        }
    }

    Value::from_object(Joiner {
        sep: sep.unwrap_or_else(|| Value::from(", ")),
        used: AtomicBool::new(false),
    })
}

/// Returns the rng for the state
#[cfg(feature = "rand")]
pub(crate) fn get_rng(state: &State) -> rand::rngs::SmallRng {
    use rand::rngs::SmallRng;
    use rand::SeedableRng;

    if let Some(seed) = state
        .lookup("RAND_SEED", &[])
        .and_then(|x| u64::try_from(x).ok())
    {
        SmallRng::seed_from_u64(seed)
    } else {
        SmallRng::from_entropy()
    }
}

/// Returns a random number in a given range.
///
/// If only one parameter is provided it's taken as exclusive upper
/// bound with 0 as lower bound, otherwise two parameters need to be
/// passed for the lower and upper bound.  Only integers are permitted.
///
/// The random number generated can be seeded with the `RAND_SEED`
/// global context variable.
#[cfg(feature = "rand")]
#[cfg_attr(docsrs, doc(cfg(feature = "rand")))]
pub fn randrange(state: &State, n: i64, m: Option<i64>) -> i64 {
    use rand::Rng;

    let (lower, upper) = match m {
        None => (0, n),
        Some(m) => (n, m),
    };

    get_rng(state).gen_range(lower..upper)
}

/// Generates a random lorem ipsum.
///
/// The random number generated can be seeded with the `RAND_SEED`
/// global context variable.
///
/// The function accepts various keyword arguments:
///
/// * `n`: number of paragraphs to generate.
/// * `min`: minimum number of words to generate per paragraph.
/// * `max`: maximum number of words to generate per paragraph.
/// * `html`: set to `true` to generate HTML paragraphs instead.
#[cfg(feature = "rand")]
#[cfg_attr(docsrs, doc(cfg(feature = "rand")))]
pub fn lipsum(
    state: &State,
    n: Option<usize>,
    kwargs: minijinja::value::Kwargs,
) -> Result<Value, Error> {
    use rand::seq::SliceRandom;
    use rand::Rng;

    #[rustfmt::skip]
    const LIPSUM_WORDS: &[&str] = &[
        "a", "ac", "accumsan", "ad", "adipiscing", "aenean", "aliquam",
        "aliquet", "amet", "ante", "aptent", "arcu", "at", "auctor", "augue",
        "bibendum", "blandit", "class", "commodo", "condimentum", "congue",
        "consectetuer", "consequat", "conubia", "convallis", "cras", "cubilia",
        "cum", "curabitur", "curae", "cursus", "dapibus", "diam", "dictum",
        "dictumst", "dignissim", "dis", "dolor", "donec", "dui", "duis",
        "egestas", "eget", "eleifend", "elementum", "elit", "enim", "erat",
        "eros", "est", "et", "etiam", "eu", "euismod", "facilisi", "facilisis",
        "fames", "faucibus", "felis", "fermentum", "feugiat", "fringilla",
        "fusce", "gravida", "habitant", "habitasse", "hac", "hendrerit",
        "hymenaeos", "iaculis", "id", "imperdiet", "in", "inceptos", "integer",
        "interdum", "ipsum", "justo", "lacinia", "lacus", "laoreet", "lectus",
        "leo", "libero", "ligula", "litora", "lobortis", "lorem", "luctus",
        "maecenas", "magna", "magnis", "malesuada", "massa", "mattis", "mauris",
        "metus", "mi", "molestie", "mollis", "montes", "morbi", "mus", "nam",
        "nascetur", "natoque", "nec", "neque", "netus", "nibh", "nisi", "nisl",
        "non", "nonummy", "nostra", "nulla", "nullam", "nunc", "odio", "orci",
        "ornare", "parturient", "pede", "pellentesque", "penatibus", "per",
        "pharetra", "phasellus", "placerat", "platea", "porta", "porttitor",
        "posuere", "potenti", "praesent", "pretium", "primis", "proin",
        "pulvinar", "purus", "quam", "quis", "quisque", "rhoncus", "ridiculus",
        "risus", "rutrum", "sagittis", "sapien", "scelerisque", "sed", "sem",
        "semper", "senectus", "sit", "sociis", "sociosqu", "ssociis",
        "sociosqu", "ssociis", "sociosqu", "ssociis", "sociosqu", "ssociis",
        "sociosqu", "ssoincidusociis", "sociosqu", "ssociis", "sociosqu",
        "ssociis", "sociosqu", "ssociis", "sociosqu", "ssociis", "vsociis",
        "sociosqu", "ssociis", "sociosqu", "ssociis", "sociosqu", "ssociis",
        "sociosqu", "ssociis", "s", "vulputate",
    ];

    let n_kwargs: Option<usize> = kwargs.get("n")?;
    let min: Option<usize> = kwargs.get("min")?;
    let min = min.unwrap_or(20);
    let max: Option<usize> = kwargs.get("max")?;
    let max = max.unwrap_or(100);
    let html: Option<bool> = kwargs.get("html")?;
    let html = html.unwrap_or(false);
    let n = n.or(n_kwargs).unwrap_or(5);
    let mut rv = String::new();

    let mut rng = get_rng(state);

    for _ in 0..n {
        let mut next_capitalized = true;
        let mut last_fullstop = 0;
        let mut last = "";

        for idx in 0..rng.gen_range(min..max) {
            if idx > 0 {
                rv.push(' ');
            } else if html {
                rv.push_str("<p>");
            }
            let word = loop {
                let word = LIPSUM_WORDS.choose(&mut rng).copied().unwrap_or("");
                if word != last {
                    last = word;
                    break word;
                }
            };

            if next_capitalized {
                for (idx, c) in word.char_indices() {
                    if idx == 0 {
                        use std::fmt::Write;
                        write!(rv, "{}", c.to_uppercase()).ok();
                    } else {
                        rv.push(c);
                    }
                }
                next_capitalized = false;
            } else {
                rv.push_str(word);
            }

            if idx - last_fullstop > rng.gen_range(10..20) {
                rv.push('.');
                last_fullstop = idx;
                next_capitalized = true;
            }
        }

        if !rv.ends_with('.') {
            rv.push('.');
        }
        if html {
            rv.push_str("</p>");
        }
        rv.push_str("\n\n");
    }

    if html {
        Ok(Value::from_safe_string(rv))
    } else {
        Ok(Value::from(rv))
    }
}
