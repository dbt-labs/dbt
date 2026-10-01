//! [`LoadCtx`]: the env globals registered on the **load** `JinjaEnv`.

use std::collections::BTreeMap;

use chrono::DateTime;
use chrono_tz::Tz;
use minijinja::Value as MinijinjaValue;
use minijinja_contrib::modules::py_datetime::datetime::PyDateTime;
use minijinja_contrib::modules::pytz::PytzTimezone;
use schemars::JsonSchema;
use serde::Serialize;

use crate::TargetContextMap;
use crate::core::GlobalCore;
use crate::jinja_object::JinjaObject;

/// Env globals for the load phase. Today this is `GlobalCore` verbatim; the
/// outer struct exists so future load-only fields can be added without
/// disturbing the shared core, and so phase-specific JsonSchema names
/// (`LoadCtx`) appear in the generated schema artefacts.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct LoadCtx {
    #[serde(flatten)]
    pub global_core: GlobalCore,
}

impl LoadCtx {
    /// Construct a `LoadCtx` from the same primitive inputs the previous
    /// `BTreeMap`-based load init consumed. The `flags` map is wrapped via
    /// `Value::from_serialize` so the field carries the same `MinijinjaValue`
    /// shape as the parse-phase `Flags` Object — keeping `GlobalCore`
    /// genuinely shared across phases.
    pub fn new(
        run_started_at: DateTime<Tz>,
        target: TargetContextMap,
        flags: BTreeMap<String, MinijinjaValue>,
    ) -> Self {
        let run_started_at = JinjaObject::new(PyDateTime::new_aware(
            run_started_at,
            Some(PytzTimezone::new(Tz::UTC)),
        ));
        Self {
            global_core: GlobalCore {
                run_started_at,
                target,
                flags: MinijinjaValue::from_serialize(flags),
            },
        }
    }
}
