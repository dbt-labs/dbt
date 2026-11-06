use std::fmt;

use dbt_telemetry::StateModifiedDiff;

/// The lines reporting a state modification diff: a header, then each side's value if
/// known.
pub struct StateModifiedDiffLines<'a>(pub &'a StateModifiedDiff);

impl fmt::Display for StateModifiedDiffLines<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let diff = self.0;
        let unique_id = diff.unique_id.as_deref().unwrap_or("unknown");
        write!(
            f,
            "[state_mod_diff] unique_id={}, node_type_or_category={}, check={}",
            unique_id, diff.node_type_or_category, diff.check
        )?;

        if let Some(self_value) = diff.self_value.as_deref() {
            write!(f, "\n   self:  {:?}", self_value)?;
        }

        if let Some(other_value) = diff.other_value.as_deref() {
            write!(f, "\n   other:  {:?}", other_value)?;
        }

        Ok(())
    }
}
