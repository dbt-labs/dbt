use core::fmt;
use std::rc::Rc;
use std::sync::Arc;

use minijinja::listener::RenderingEventListener;
use minijinja::value::{Enumerator, Object, ObjectRepr};
use minijinja::{State, Value};

use crate::table::TableRepr;
use crate::{MappedSequence, Tuple, TupleRepr, ZippedTupleRepr};

#[derive(Debug)]
pub(crate) struct ColumnTypesAsTuple {
    of_table: Arc<TableRepr>,
}

impl ColumnTypesAsTuple {
    pub fn of_table(table: &Arc<TableRepr>) -> Self {
        Self {
            of_table: Arc::clone(table),
        }
    }

    pub fn into_tuple(self) -> Tuple {
        Tuple(Box::new(self))
    }

    pub fn to_value(&self) -> Value {
        let tuple = ColumnTypesAsTuple::of_table(&self.of_table).into_tuple();
        Value::from_object(tuple)
    }
}

impl Eq for ColumnTypesAsTuple {}

impl PartialEq for ColumnTypesAsTuple {
    fn eq(&self, other: &Self) -> bool {
        let self_types = self.of_table.column_types();
        let other_types = other.of_table.column_types();
        self_types.eq(other_types)
    }
}

impl TupleRepr for ColumnTypesAsTuple {
    fn get_item_by_index(&self, idx: isize) -> Option<Value> {
        self.of_table
            .column_type(idx)
            .map(|dt| Value::from_object(dt.clone()))
    }

    fn len(&self) -> usize {
        self.of_table.num_columns()
    }

    fn count_occurrences_of(&self, needle: &Value) -> usize {
        let column_types = self.of_table.column_types().iter();
        if let Some(name) = needle.as_str() {
            column_types.filter(|dt| (*dt).type_name() == name).count()
        } else if let Some(needle_dt) = needle.downcast_object_ref::<crate::DataType>() {
            column_types.filter(|dt| (*dt).eq(needle_dt)).count()
        } else {
            0
        }
    }

    fn index_of(&self, needle: &Value) -> Option<usize> {
        let mut column_types = self.of_table.column_types().iter();
        if let Some(name) = needle.as_str() {
            column_types.position(|dt| dt.type_name() == name)
        } else if let Some(needle_dt) = needle.downcast_object_ref::<crate::DataType>() {
            column_types.position(|dt| dt.eq(needle_dt))
        } else {
            None
        }
    }

    fn clone_repr(&self) -> Box<dyn TupleRepr> {
        Box::new(ColumnTypesAsTuple {
            of_table: Arc::clone(&self.of_table),
        })
    }

    fn eq_repr(&self, other: &dyn TupleRepr) -> bool {
        if self.len() != other.len() {
            return false;
        }
        let self_types = self.of_table.column_types().iter();
        for (i, self_type) in self_types.enumerate() {
            let other_value = match other.get_item_by_index(i as isize) {
                Some(other_value) => other_value,
                None => return false,
            };
            if let Some(other_dt) = other_value.downcast_object_ref::<crate::DataType>() {
                if !self_type.eq(other_dt) {
                    return false;
                }
                continue;
            } else {
                return false;
            }
        }
        true
    }
}

#[derive(Debug)]
pub(crate) struct ColumnNamesAsTuple {
    of_table: Arc<TableRepr>,
}

impl ColumnNamesAsTuple {
    pub fn of_table(table: &Arc<TableRepr>) -> Self {
        Self {
            of_table: Arc::clone(table),
        }
    }

    pub fn into_tuple(self) -> Tuple {
        Tuple(Box::new(self))
    }

    pub fn to_value(&self) -> Value {
        let tuple = ColumnNamesAsTuple::of_table(&self.of_table).into_tuple();
        Value::from_object(tuple)
    }
}

impl Eq for ColumnNamesAsTuple {}

impl PartialEq for ColumnNamesAsTuple {
    fn eq(&self, other: &Self) -> bool {
        let self_names = self.of_table.column_names();
        let other_names = other.of_table.column_names();
        self_names.eq(other_names)
    }
}

impl TupleRepr for ColumnNamesAsTuple {
    fn get_item_by_index(&self, idx: isize) -> Option<Value> {
        self.of_table.column_name(idx).map(Value::from)
    }

    fn len(&self) -> usize {
        self.of_table.num_columns()
    }

    fn count_occurrences_of(&self, needle: &Value) -> usize {
        if let Some(name) = needle.as_str() {
            self.of_table.column_names().filter(|n| n == &name).count()
        } else {
            0
        }
    }

    fn index_of(&self, needle: &Value) -> Option<usize> {
        if let Some(name) = needle.as_str() {
            self.of_table.column_names().position(|n| n == name)
        } else {
            None
        }
    }

    fn eq_repr(&self, other: &dyn TupleRepr) -> bool {
        if self.len() != other.len() {
            return false;
        }
        let self_names = self.of_table.column_names();
        for (i, self_name) in self_names.enumerate() {
            let other_name = other.get_item_by_index(i as isize);
            if Some(self_name.as_str()) != other_name.as_ref().and_then(|v| v.as_str()) {
                return false;
            }
        }
        true
    }

    fn clone_repr(&self) -> Box<dyn TupleRepr> {
        Box::new(ColumnNamesAsTuple {
            of_table: Arc::clone(&self.of_table),
        })
    }
}

#[derive(Debug)]
struct ColumnsAsTuple {
    of_table: Arc<TableRepr>,
}

impl ColumnsAsTuple {
    pub fn of_table(of_table: &Arc<TableRepr>) -> Self {
        Self {
            of_table: Arc::clone(of_table),
        }
    }

    pub fn into_tuple(self) -> Tuple {
        Tuple(Box::new(self))
    }
}

impl TupleRepr for ColumnsAsTuple {
    fn get_item_by_index(&self, idx: isize) -> Option<Value> {
        let column = self.of_table.get_column(idx)?;
        Some(Value::from_object(column))
    }

    fn len(&self) -> usize {
        self.of_table.num_columns()
    }

    fn count_occurrences_of(&self, _needle: &Value) -> usize {
        todo!("ColumnsAsTuple::count_occurrences_of")
    }

    fn index_of(&self, _needle: &Value) -> Option<usize> {
        todo!("ColumnsAsTuple::index_of")
    }

    fn clone_repr(&self) -> Box<dyn TupleRepr> {
        Box::new(ColumnsAsTuple {
            of_table: Arc::clone(&self.of_table),
        })
    }
}

/// Represents an instance of a `MappedSequence` populated by a list of columns.
///
/// https://github.com/wireservice/agate/blob/7023e35b51e8abfe9784fe292a23dd4d7d983c63/agate/table/__init__.py#L181
#[derive(Debug)]
pub struct Columns {
    /// Internal representation of the columns sequence is the table representation itself.
    of_table: Arc<TableRepr>,
}

impl Columns {
    pub(crate) fn new(of_table: Arc<TableRepr>) -> Self {
        Self { of_table }
    }
}

impl MappedSequence for Columns {
    fn values(&self) -> Tuple {
        let columns = ColumnsAsTuple {
            of_table: Arc::clone(&self.of_table),
        };
        let repr = Box::new(columns);
        Tuple(repr)
    }

    fn keys(&self) -> Option<Tuple> {
        let column_names = ColumnNamesAsTuple::of_table(&self.of_table).into_tuple();
        Some(column_names)
    }

    fn items(&self) -> Option<Tuple> {
        let column_names = ColumnNamesAsTuple::of_table(&self.of_table).into_tuple();
        let columns = ColumnsAsTuple::of_table(&self.of_table).into_tuple();
        let items = ZippedTupleRepr::from_tuples(&column_names, &columns).into_tuple();
        Some(items)
    }
}

impl Object for Columns {
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
