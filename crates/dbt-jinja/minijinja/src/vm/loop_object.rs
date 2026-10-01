use std::fmt;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::{Error, ErrorKind};
use crate::listener::RenderingEventListener;
use crate::value::{Enumerator, Object, Value};
use crate::vm::state::State;

pub(crate) struct Loop {
    pub len: Option<usize>,
    pub idx: AtomicUsize,
    pub depth: usize,
    #[cfg(feature = "adjacent_loop_items")]
    pub value_triple: Mutex<(Option<Value>, Option<Value>, Option<Value>)>,
    pub last_changed_value: Mutex<Option<Vec<Value>>>,
}

impl fmt::Debug for Loop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Loop")
            .field("len", &self.len)
            .field("idx", &self.idx)
            .field("depth", &self.depth)
            .finish()
    }
}

impl Object for Loop {
    fn call(
        self: &Arc<Self>,
        _state: &State,
        _args: &[Value],
        _listener: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        Err(Error::new(
            ErrorKind::InvalidOperation,
            "loop cannot be called if reassigned to different variable",
        ))
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &State,
        name: &str,
        args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        if name == "changed" {
            let mut last_changed_value = self.last_changed_value.lock().unwrap();
            let value = args.to_owned();
            let changed = last_changed_value.as_ref() != Some(&value);
            if changed {
                *last_changed_value = Some(value);
                Ok(Value::from(true))
            } else {
                Ok(Value::from(false))
            }
        } else if name == "cycle" {
            let idx = self.idx.load(Ordering::Relaxed);
            match args.get(idx % args.len()) {
                Some(arg) => Ok(arg.clone()),
                None => Ok(Value::UNDEFINED),
            }
        } else {
            Err(Error::from(ErrorKind::UnknownMethod))
        }
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Str(&[
            "index0",
            "index",
            "length",
            "revindex",
            "revindex0",
            "first",
            "last",
            "depth",
            "depth0",
            #[cfg(feature = "adjacent_loop_items")]
            "previtem",
            #[cfg(feature = "adjacent_loop_items")]
            "nextitem",
        ])
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let key = some!(key.as_str());
        let idx = self.idx.load(Ordering::Relaxed) as u64;
        // if we never iterated, then all attributes are undefined.
        // this can happen in some rare circumstances where the engine
        // did not manage to iterate
        if idx == !0 {
            return Some(Value::UNDEFINED);
        }
        let len = self.len.map(|x| x as u64);
        match key {
            "index0" => Some(Value::from(idx)),
            "index" => Some(Value::from(idx + 1)),
            "length" => Some(len.map(Value::from).unwrap_or(Value::UNDEFINED)),
            "revindex" => Some(
                len.map(|len| Value::from(len.saturating_sub(idx)))
                    .unwrap_or(Value::UNDEFINED),
            ),
            "revindex0" => Some(
                len.map(|len| Value::from(len.saturating_sub(idx).saturating_sub(1)))
                    .unwrap_or(Value::UNDEFINED),
            ),
            "first" => Some(Value::from(idx == 0)),
            "last" => Some(len.map_or_else(
                || Value::from(false),
                |len| Value::from(len == 0 || idx == len - 1),
            )),
            "depth" => Some(Value::from(self.depth + 1)),
            "depth0" => Some(Value::from(self.depth)),
            #[cfg(feature = "adjacent_loop_items")]
            "previtem" => Some(
                self.value_triple
                    .lock()
                    .unwrap()
                    .0
                    .clone()
                    .unwrap_or(Value::UNDEFINED),
            ),
            #[cfg(feature = "adjacent_loop_items")]
            "nextitem" => Some(
                self.value_triple
                    .lock()
                    .unwrap()
                    .2
                    .clone()
                    .unwrap_or(Value::UNDEFINED),
            ),
            _ => None,
        }
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "<loop {}/{}>",
            self.idx.load(Ordering::Relaxed),
            match self.len {
                Some(ref len) => len as &dyn fmt::Display,
                None => &"?" as &dyn fmt::Display,
            },
        )
    }
}
