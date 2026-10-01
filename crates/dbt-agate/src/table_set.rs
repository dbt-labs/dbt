use std::rc::Rc;
use std::sync::Arc;
use std::{fmt, io};

use minijinja::arg_utils::ArgsIter;
use minijinja::listener::RenderingEventListener;
use minijinja::value::{Enumerator, Object, ObjectRepr};
use minijinja::{Error, ErrorKind, State, Value};

use crate::columns::{ColumnNamesAsTuple, ColumnTypesAsTuple};
use crate::table::AgateTable;
use crate::{MappedSequence, Tuple, TupleRepr, adjusted_index};

/// A group of named tables with identical column definitions.
///
/// The [TableSet] class collects a set of related tables in a single data
/// structure. The most common way of creating a [TableSet] is using the
/// [AgateTable::group_by] method, which is similar to SQL's ``GROUP BY`` keyword.
/// The resulting set of tables will all have identical columns structure.
///
/// [TableSet] functions as a dictionary. Individual tables in the set can
/// be accessed by using their name as a key. If the table set was created using
/// [AgaetTable.group_by] then the names of the tables will be the grouping
/// factors found in the original data.
///
/// [TableSet] replicates the majority of the features of [AgateTable].
/// When methods such as [TableSet::select], [TableSet::where] or
/// [TableSet::order_by] are used, the operation is applied to *each* table
/// in the set and the result is a new [ableSet] instance made up of
/// entirely new [Table] instances.
///
/// [TableSet] instances can also contain other TableSet's. This means you
/// can chain calls to [Table::group_by] and [TableSet::group_by]
/// and end up with data grouped across multiple dimensions.
/// [TableSet::aggregate] on nested TableSets will then group across multiple
/// dimensions.
///
/// https://github.com/wireservice/agate/blob/5ebea8dd0b9c7cd0f795e53695aa4d782b95e40c/agate/tableset/__init__.py
#[derive(Debug)]
pub struct TableSet {
    repr: Arc<TableSetRepr>,
}

#[derive(Debug)]
pub(crate) struct TableSetRepr {
    key_name: String,
    key_type: crate::DataType,
    #[allow(dead_code)]
    sample_table: Option<Arc<AgateTable>>,
    column_types: Option<ColumnTypesAsTuple>,
    column_names: Option<ColumnNamesAsTuple>,
    tables: Vec<Arc<AgateTable>>,
    keys: Vec<Value>,
}

impl TableSet {
    /// ```python
    /// class TableSet(MappedSequence):
    ///     """
    ///     An group of named tables with identical column definitions. Supports
    ///     (almost) all the same operations as :class:`.Table`. When executed on a
    ///     :class:`TableSet`, any operation that would have returned a new
    ///     :class:`.Table` instead returns a new :class:`TableSet`. Any operation
    ///     that would have returned a single value instead returns a dictionary of
    ///     values.
    ///
    ///     TableSet is implemented as a subclass of :class:`.MappedSequence`
    ///
    ///     :param tables:
    ///         A sequence :class:`Table` instances.
    ///     :param keys:
    ///         A sequence of keys corresponding to the tables. These may be any type
    ///         except :class:`int`.
    ///     :param key_name:
    ///         A name that describes the grouping properties. Used as the column
    ///         header when the groups are aggregated. Defaults to the column name that
    ///         was grouped on.
    ///     :param key_type:
    ///         An instance some subclass of :class:`.DataType`. If not provided it
    ///         will default to a :class`.Text`.
    ///     :param _is_fork:
    ///         Used internally to skip certain validation steps when data
    ///         is propagated from an existing tablset.
    ///     """
    /// ```
    pub fn try_new(
        tables: Vec<Arc<AgateTable>>,
        keys: Vec<Value>,
        key_name: Option<String>,
        key_type: Option<crate::DataType>,
    ) -> Result<Self, Error> {
        let repr = TableSetRepr::try_new(tables, keys, key_name, key_type, false)?;
        Ok(Self::from_repr(repr))
    }

    pub(crate) fn from_repr(repr: Arc<TableSetRepr>) -> Self {
        Self { repr }
    }

    pub fn print_structure(
        self: &Arc<Self>,
        max_rows: Option<usize>,
        output: Option<&mut dyn io::Write>,
    ) -> io::Error {
        let _max_rows = max_rows.unwrap_or(20);
        #[allow(clippy::or_fun_call)]
        let _output = output.unwrap_or(&mut io::stdout());
        // TODO: actual print_structure() implementation
        todo!("TableSet.print_structure")
    }

    pub fn is_empty(&self) -> bool {
        self.repr.tables.len() == 0
    }

    pub fn len(&self) -> usize {
        self.repr.tables.len()
    }
}

impl TableSetRepr {
    pub(crate) fn try_new(
        tables: Vec<Arc<AgateTable>>,
        keys: Vec<Value>,
        key_name: Option<String>,
        key_type: Option<crate::DataType>,
        is_fork: bool,
    ) -> Result<Arc<Self>, Error> {
        let key_name = key_name.unwrap_or_else(|| "group".to_string());
        let key_type = key_type.unwrap_or_else(|| crate::DataType::new("Text".to_string()));
        let sample_table = tables.first().map(Arc::clone);

        let column_types = sample_table.as_ref().map(|t| t.column_types_as_tuple());
        let column_names = sample_table.as_ref().map(|t| t.column_names_as_tuple());

        if !is_fork {
            for table in &tables {
                let self_column_types = column_types
                    .as_ref()
                    .expect("column types from sample table");
                if table.column_types_as_tuple() != *self_column_types {
                    return Err(Error::new(
                        ErrorKind::InvalidArgument,
                        "Not all tables have the same column types!",
                    ));
                }

                let self_column_names = column_names
                    .as_ref()
                    .expect("column names from sample table");
                if table.column_names_as_tuple() != *self_column_names {
                    return Err(Error::new(
                        ErrorKind::InvalidArgument,
                        "Not all tables have the same column names!",
                    ));
                }
            }
        }

        let repr = TableSetRepr {
            key_name,
            key_type,
            sample_table,
            column_types,
            column_names,
            tables,
            keys,
        };
        Ok(Arc::new(repr))
    }

    /// Create a new [TableSet] using the metadata from this one.
    ///
    /// ```python
    /// def _fork(self, tables, keys, key_name=None, key_type=None):
    ///     """
    ///     Create a new :class:`.TableSet` using the metadata from this one.
    ///
    ///     This method is used internally by functions like
    ///     :meth:`.TableSet.having`.
    ///     """
    /// ```
    fn _fork(
        &self,
        tables: Vec<Arc<AgateTable>>,
        keys: Vec<Value>,
        key_name: Option<String>,
        key_type: Option<crate::DataType>,
    ) -> Result<Arc<Self>, Error> {
        let key_name = key_name.unwrap_or_else(|| self.key_name.clone());
        let key_type = key_type.unwrap_or_else(|| self.key_type.clone());
        Self::try_new(tables, keys, Some(key_name), Some(key_type), true)
    }

    /// Calls a method on each table in this [TableSet].
    ///
    /// ```python
    /// def _proxy(self, method_name, *args, **kwargs):
    ///     """
    ///     Calls a method on each table in this :class:`.TableSet`.
    ///     """
    /// ```
    fn _proxy(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        method: &str,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Arc<Self>, Error> {
        let mut tables: Vec<Arc<AgateTable>> = Vec::with_capacity(self.tables.len());
        for table in self.tables.iter() {
            let new_table = table
                .call_method(state, method, args, listeners)?
                .downcast_object::<AgateTable>()
                .ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidOperation,
                        format!(
                            "Method '{}' did not return a Table when called on a TableSet",
                            method
                        ),
                    )
                })?;
            tables.push(new_table);
        }
        self._fork(tables, self.keys.clone(), None, None)
    }

    fn tables_as_tuple(self: &Arc<Self>) -> TableSetTablesAsTuple {
        TableSetTablesAsTuple::of_tableset(self)
    }

    fn keys_as_tuple(self: &Arc<Self>) -> TableSetKeysAsTuple {
        TableSetKeysAsTuple::of_tableset(self)
    }

    fn len(&self) -> usize {
        self.tables.len()
    }
}

impl MappedSequence for TableSet {
    fn type_name(&self) -> &str {
        "TableSet"
    }

    fn values(&self) -> Tuple {
        self.repr.tables_as_tuple().into_tuple()
    }

    fn keys(&self) -> Option<Tuple> {
        Some(self.repr.keys_as_tuple().into_tuple())
    }
}

impl Object for TableSet {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        MappedSequence::repr(self)
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        if let Some(name) = key.as_str() {
            match name {
                // ```python
                // @property
                // def key_name(self):
                //     """
                //     Get the name of the key this TableSet is grouped by. (If created using
                //     :meth:`.Table.group_by` then this is the original column name.)
                //     """
                // ```
                "key_name" => Some(Value::from(&self.repr.key_name)),
                // ```python
                // @property
                // def key_type(self):
                //     """
                //     Get the :class:`.DataType` this TableSet is grouped by. (If created
                //     using :meth:`.Table.group_by` then this is the original column type.)
                //     """
                // ```
                "key_type" => Some(Value::from_object(self.repr.key_type.clone())),
                // ```python
                // @property
                // def column_types(self):
                //     """
                //     Get an ordered list of this :class:`.TableSet`'s column types.
                //
                //     :returns:
                //         A :class:`tuple` of :class:`.DataType` instances.
                //     """
                // ```
                "column_types" => self.repr.column_types.as_ref().map(|ct| ct.to_value()),
                // ```python
                // @property
                // def column_names(self):
                //     """
                //     Get an ordered list of this :class:`TableSet`'s column names.
                //
                //     :returns:
                //         A :class:`tuple` of strings.
                //     """
                // ```
                "column_names" => self.repr.column_names.as_ref().map(|cn| cn.to_value()),
                // delegate to the super-class -- `MappedSequence`
                _ => MappedSequence::get_value(self, key),
            }
        } else {
            MappedSequence::get_value(self, key)
        }
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        MappedSequence::enumerate(self)
    }

    fn call_method(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        method: &str,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        match method {
            // TableSet methods
            // TODO: TableSet.aggregate
            // TODO: TableSet.bar_chart
            // TODO: TableSet.bins
            // TODO: TableSet.column_chart
            // TODO: TableSet.compute
            // TODO: TableSet.denormalize
            // TODO: TableSet.distinct
            // TODO: TableSet.exclude
            // TODO: TableSet.find
            // TODO: TableSet.from_csv
            // TODO: TableSet.from_json
            // TODO: TableSet.group_by
            // TODO: TableSet.having
            // TODO: TableSet.homogenize
            // TODO: TableSet.join
            // TODO: TableSet.limit
            // TODO: TableSet.line_chart
            // TODO: TableSet.merge
            // TODO: TableSet.normalize
            // TODO: TableSet.order_by
            // TODO: TableSet.pivot
            // ```python
            // def print_structure(self, max_rows=20, output=sys.stdout):
            //     """
            //     Print the keys and row counts of each table in the tableset.
            //
            //     :param max_rows:
            //         The maximum number of rows to display before truncating the data.
            //         Defaults to 20.
            //     :param output:
            //         The output used to print the structure of the :class:`Table`.
            //     :returns:
            //         None
            //     """
            // ```
            "print_structure" => {
                let args_iter = ArgsIter::new("TableSet.print_structure", &[], args);
                let max_rows = args_iter.next_kwarg::<Option<usize>>("max_rows")?;
                let output = args_iter.next_kwarg::<Option<&Value>>("output")?;
                args_iter.finish()?;

                if output.is_some() {
                    unimplemented!("TableSet.print_structure with output argument");
                }
                self.print_structure(max_rows, None);
                Ok(Value::from(()))
            }
            // TODO: TableSet.scatterplot
            // TODO: TableSet.select
            // TODO: TableSet.to_csv
            // TODO: TableSet.to_json
            // TODO: TableSet.where
            // Delegate to the super-class -- MappedSequence
            _ => MappedSequence::call_method(self, state, method, args, listeners),
        }
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: delegate to print_structure()
        MappedSequence::render(self, f)
    }
}

#[derive(Debug)]
struct TableSetTablesAsTuple {
    of_tableset: Arc<TableSetRepr>,
}

impl TableSetTablesAsTuple {
    pub fn of_tableset(of_tableset: &Arc<TableSetRepr>) -> Self {
        Self {
            of_tableset: Arc::clone(of_tableset),
        }
    }

    pub fn into_tuple(self) -> Tuple {
        Tuple(Box::new(self))
    }
}

impl TupleRepr for TableSetTablesAsTuple {
    fn get_item_by_index(&self, idx: isize) -> Option<Value> {
        let idx = adjusted_index(idx, self.of_tableset.len())?;
        let table = self.of_tableset.tables.get(idx)?;
        Some(table.to_value())
    }

    fn len(&self) -> usize {
        self.of_tableset.len()
    }

    fn count_occurrences_of(&self, _value: &Value) -> usize {
        unimplemented!("TableSetTablesAsTuple::count_occurrences_of")
    }

    fn index_of(&self, _value: &Value) -> Option<usize> {
        unimplemented!("TableSetTablesAsTuple::index_of")
    }

    fn clone_repr(&self) -> Box<dyn TupleRepr> {
        Box::new(TableSetTablesAsTuple {
            of_tableset: Arc::clone(&self.of_tableset),
        })
    }
}

#[derive(Debug)]
struct TableSetKeysAsTuple {
    of_tableset: Arc<TableSetRepr>,
}

impl TableSetKeysAsTuple {
    pub fn of_tableset(of_tableset: &Arc<TableSetRepr>) -> Self {
        Self {
            of_tableset: Arc::clone(of_tableset),
        }
    }

    pub fn into_tuple(self) -> Tuple {
        Tuple(Box::new(self))
    }
}

impl TupleRepr for TableSetKeysAsTuple {
    fn get_item_by_index(&self, idx: isize) -> Option<Value> {
        let idx = adjusted_index(idx, self.of_tableset.as_ref().len())?;
        let key = self.of_tableset.keys.get(idx)?;
        Some(key.clone())
    }

    fn len(&self) -> usize {
        self.of_tableset.len()
    }

    fn count_occurrences_of(&self, value: &Value) -> usize {
        self.of_tableset.keys.iter().filter(|k| *k == value).count()
    }

    fn index_of(&self, value: &Value) -> Option<usize> {
        self.of_tableset.keys.iter().position(|k| k == value)
    }

    fn clone_repr(&self) -> Box<dyn TupleRepr> {
        Box::new(TableSetKeysAsTuple {
            of_tableset: Arc::clone(&self.of_tableset),
        })
    }
}
