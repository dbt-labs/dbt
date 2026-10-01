use core::fmt;
use std::rc::Rc;
use std::sync::Arc;

use crate::table::TableRepr;
use crate::{MappedSequence, Tuple, TupleRepr};
use arrow_array::{Array, StringViewArray};
use minijinja::listener::RenderingEventListener;
use minijinja::value::{Enumerator, Object, ObjectRepr};
use minijinja::{State, Value};

#[derive(Debug)]
pub struct RowNamesAsTuple {
    row_names_array: Arc<StringViewArray>,
}

impl RowNamesAsTuple {
    pub fn new(row_names_array: Arc<StringViewArray>) -> Self {
        Self { row_names_array }
    }
}

impl TupleRepr for RowNamesAsTuple {
    fn get_item_by_index(&self, idx: isize) -> Option<Value> {
        let idx = if idx < 0 {
            self.row_names_array.len() as isize + idx
        } else {
            idx
        };
        if idx < 0 || (idx as usize) >= self.row_names_array.len() {
            None
        } else {
            let value = Value::from(self.row_names_array.value(idx as usize).to_string());
            Some(value)
        }
    }

    fn len(&self) -> usize {
        self.row_names_array.len()
    }

    fn count_occurrences_of(&self, needle: &Value) -> usize {
        if let Some(name) = needle.as_str() {
            self.row_names_array
                .iter()
                .filter(|opt| *opt == Some(name))
                .count()
        } else {
            0
        }
    }

    fn index_of(&self, needle: &Value) -> Option<usize> {
        if let Some(name) = needle.as_str() {
            self.row_names_array
                .iter()
                .position(|opt| opt == Some(name))
        } else {
            None
        }
    }

    fn clone_repr(&self) -> Box<dyn TupleRepr> {
        Box::new(RowNamesAsTuple {
            row_names_array: Arc::clone(&self.row_names_array),
        })
    }
}

#[derive(Debug)]
struct RowsAsTuple {
    of_table: Arc<TableRepr>,
}

impl TupleRepr for RowsAsTuple {
    fn get_item_by_index(&self, idx: isize) -> Option<Value> {
        self.of_table.row_by_index(idx)
    }

    fn len(&self) -> usize {
        self.of_table.num_rows()
    }

    fn count_occurrences_of(&self, value: &Value) -> usize {
        self.of_table.count_occurrences_of_row(value)
    }

    fn index_of(&self, value: &Value) -> Option<usize> {
        self.of_table.index_of_row(value)
    }

    fn clone_repr(&self) -> Box<dyn TupleRepr> {
        Box::new(RowsAsTuple {
            of_table: Arc::clone(&self.of_table),
        })
    }
}

/// Iterator for Rows that maintains its own cursor state.
#[derive(Debug)]
pub struct RowsIterator {
    of_table: Arc<TableRepr>,
    num_rows: usize,
    cursor: usize,
}

impl IntoIterator for Rows {
    type Item = Value;
    type IntoIter = RowsIterator;

    fn into_iter(self) -> Self::IntoIter {
        RowsIterator {
            of_table: Arc::clone(&self.of_table),
            num_rows: self.of_table.num_rows(),
            cursor: 0,
        }
    }
}

impl Iterator for RowsIterator {
    type Item = Value;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor < self.num_rows {
            let row = self.of_table.row_by_index(self.cursor as isize);
            self.cursor += 1;
            row
        } else {
            None
        }
    }
}

/// Represents an instance of a `MappedSequence` populated by a list of rows.
///
/// https://github.com/wireservice/agate/blob/7023e35b51e8abfe9784fe292a23dd4d7d983c63/agate/table/__init__.py#L168
#[derive(Debug)]
pub struct Rows {
    /// Internal representation of the list of rows is the table representation itself.
    of_table: Arc<TableRepr>,
}

impl Rows {
    pub(crate) fn new(of_table: Arc<TableRepr>) -> Self {
        Self { of_table }
    }
}

impl MappedSequence for Rows {
    fn values(&self) -> Tuple {
        let rows = RowsAsTuple {
            of_table: Arc::clone(&self.of_table),
        };
        let repr = Box::new(rows);
        Tuple(repr)
    }

    fn keys(&self) -> Option<Tuple> {
        self.of_table.row_names()
    }
}

impl Object for Rows {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        MappedSequence::repr(self)
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        MappedSequence::get_value(self, key)
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        MappedSequence::enumerate(self)
    }

    fn call_method(
        self: &Arc<Self>,
        state: &State,
        name: &str,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, minijinja::Error> {
        MappedSequence::call_method(self, state, name, args, listeners)
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        MappedSequence::render(self, f)
    }
}
