use tracing::{Event, field::ValueSet, span::Record};

pub(super) enum Recordable<'a> {
    ValueSet(&'a ValueSet<'a>),
    Record(&'a Record<'a>),
    Event(&'a Event<'a>),
}

impl<'a> From<&'a ValueSet<'a>> for Recordable<'a> {
    fn from(value: &'a ValueSet<'a>) -> Self {
        Recordable::ValueSet(value)
    }
}

impl<'a> From<&'a Record<'a>> for Recordable<'a> {
    fn from(value: &'a Record<'a>) -> Self {
        Recordable::Record(value)
    }
}

impl<'a> From<&'a Event<'a>> for Recordable<'a> {
    fn from(value: &'a Event<'a>) -> Self {
        Recordable::Event(value)
    }
}

impl<'a> Recordable<'a> {
    pub fn record(&self, visitor: &mut dyn tracing::field::Visit) {
        match self {
            Recordable::ValueSet(values) => values.record(visitor),
            Recordable::Record(record) => record.record(visitor),
            Recordable::Event(event) => event.record(visitor),
        }
    }
}
