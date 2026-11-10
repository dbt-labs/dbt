use std::sync::Mutex;

use minijinja::{Error, ErrorKind, Value};

/// Defer fixture metadata work until a render requests it, then reuse it only
/// within that render. An undefined result lets normal metadata lookup proceed.
pub fn lazy_fixture_columns(
    infer: impl Fn() -> Result<Value, Error> + Send + Sync + 'static,
) -> Value {
    let cached = Mutex::new(None::<Value>);
    Value::from_function(move || -> Result<Value, Error> {
        let mut cached = cached.lock().map_err(|_| {
            Error::new(
                ErrorKind::InvalidOperation,
                "Fixture column lookup was interrupted",
            )
        })?;
        if let Some(value) = cached.as_ref() {
            return Ok(value.clone());
        }
        let value = infer()?;
        *cached = Some(value.clone());
        Ok(value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn fixture_column_inference_is_lazy_and_reused() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let lookup = lazy_fixture_columns(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(Value::from(vec!["ALPHA"]))
        });
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let env = minijinja::Environment::new();
        let state = minijinja::State::new_for_env(&env);
        for _ in 0..2 {
            let columns = lookup.call(&state, &[], &[]).unwrap();
            assert_eq!(
                columns.get_item(&Value::from(0)).unwrap().as_str(),
                Some("ALPHA")
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
