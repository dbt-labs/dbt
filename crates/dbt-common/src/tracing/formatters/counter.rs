use std::fmt;

/// `{value} {label}`, with the label singular for a value of one.
pub(super) struct CountText {
    value: u64,
    label: &'static str,
}

impl CountText {
    pub(super) fn new(value: u64, label_single: &'static str, label_plural: &'static str) -> Self {
        let label = if value == 1 {
            label_single
        } else {
            label_plural
        };
        Self { value, label }
    }
}

impl fmt::Display for CountText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.value, self.label)
    }
}
