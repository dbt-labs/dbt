use std::cmp::Ordering;

use minijinja::arg_utils::ArgsIter;
use minijinja::value::{DynObject, Value};
use minijinja::Error;

use crate::AgateTable;

impl AgateTable {
    /// Cell-wise comparison: names, agate types, and converter-backed values.
    ///
    /// Physical Arrow identity is intentionally ignored (Utf8 vs Utf8View).
    /// After a successful table downcast, [`minijinja::value::Object::custom_cmp`]
    /// must return `Some` so MiniJinja does not fall through to default Map equality.
    pub(crate) fn semantic_cmp(&self, other: &Self) -> Ordering {
        if std::ptr::eq(self, other) {
            return Ordering::Equal;
        }
        self.cmp_shape(other)
            .then_with(|| self.cmp_cells(other))
    }

    fn cmp_shape(&self, other: &Self) -> Ordering {
        self.num_rows()
            .cmp(&other.num_rows())
            .then_with(|| self.num_columns().cmp(&other.num_columns()))
            .then_with(|| self.column_names_iter().cmp(other.column_names_iter()))
            .then_with(|| self.cmp_column_types(other))
    }

    fn cmp_column_types(&self, other: &Self) -> Ordering {
        self.column_types()
            .iter()
            .map(|t| t.type_name())
            .cmp(other.column_types().iter().map(|t| t.type_name()))
    }

    fn cmp_cells(&self, other: &Self) -> Ordering {
        for row_idx in 0..self.num_rows() {
            let row_ord = self.cmp_row(other, row_idx);
            if row_ord != Ordering::Equal {
                return row_ord;
            }
        }
        Ordering::Equal
    }

    fn cmp_row(&self, other: &Self, row_idx: usize) -> Ordering {
        for col_idx in 0..self.num_columns() {
            let cell_ord = self
                .cell(row_idx as isize, col_idx as isize)
                .cmp(&other.cell(row_idx as isize, col_idx as isize));
            if cell_ord != Ordering::Equal {
                return cell_ord;
            }
        }
        Ordering::Equal
    }

    pub(crate) fn custom_cmp_object(&self, other: &DynObject) -> Option<Ordering> {
        other
            .downcast_ref::<AgateTable>()
            .map(|other| self.semantic_cmp(other))
    }

    fn eq_value(&self, other: &Value) -> bool {
        other
            .downcast_object_ref::<AgateTable>()
            .is_some_and(|other| self.semantic_cmp(other) == Ordering::Equal)
    }

    pub(crate) fn call_unknown_method(&self, name: &str, args: &[Value]) -> Result<Value, Error> {
        match name {
            "__eq__" | "__ne__" => self.call_eq_ne(name, args),
            other => unimplemented!("AgateTable::{other}"),
        }
    }

    fn call_eq_ne(&self, name: &str, args: &[Value]) -> Result<Value, Error> {
        let eq = name == "__eq__";
        let fn_name = if eq { "Table.__eq__" } else { "Table.__ne__" };
        let iter = ArgsIter::for_unnamed_pos_args(fn_name, 1, args);
        let other = iter.next_arg::<&Value>()?;
        iter.finish()?;
        let equal = self.eq_value(other);
        Ok(Value::from(if eq { equal } else { !equal }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{ArrayRef, Int32Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use arrow_array::StringViewArray;
    use minijinja::Environment;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn render_globals(globals: Vec<(&str, Value)>, template: &str) -> String {
        let mut env = Environment::new();
        for (name, value) in globals {
            env.add_global(name, value);
        }
        env.render_str(template, HashMap::<String, String>::new(), &[])
            .expect("render should succeed without panic")
    }

    fn table_from_ids_and_countries(
        ids: Vec<Option<i32>>,
        countries: Vec<Option<&str>>,
    ) -> AgateTable {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, true),
            Field::new("country", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(ids)) as ArrayRef,
                Arc::new(StringArray::from(countries)) as ArrayRef,
            ],
        )
        .unwrap();
        AgateTable::from_record_batch(Arc::new(batch))
    }

    #[test]
    fn table_eq_same_object_is_true() {
        let table = table_from_ids_and_countries(
            vec![Some(42), Some(43), Some(44)],
            vec![Some("Brazil"), Some("USA"), Some("Canada")],
        );
        let out = render_globals(vec![("a", table.into_value())], "{{ a == a }}");
        assert_eq!(out, "True");
    }

    #[test]
    fn table_eq_is_cell_wise_not_arrow_identity() {
        let utf8 =
            table_from_ids_and_countries(vec![Some(1), None], vec![Some("Brazil"), Some("USA")]);
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, true),
            Field::new("country", DataType::Utf8View, true),
        ]));
        let utf8_view = AgateTable::from_record_batch(Arc::new(
            RecordBatch::try_new(
                schema,
                vec![
                    Arc::new(Int32Array::from(vec![Some(1), None])) as ArrayRef,
                    Arc::new(StringViewArray::from(vec![Some("Brazil"), Some("USA")])) as ArrayRef,
                ],
            )
            .unwrap(),
        ));

        let out = render_globals(
            vec![("a", utf8.into_value()), ("b", utf8_view.into_value())],
            "{{ a == b }} {{ a != b }}",
        );
        assert_eq!(out, "True False");
    }

    #[test]
    fn table_eq_false_when_values_names_or_types_differ() {
        let left = table_from_ids_and_countries(vec![Some(1)], vec![Some("Brazil")]);
        let different_values = table_from_ids_and_countries(vec![Some(2)], vec![Some("Brazil")]);
        let different_names_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, true),
            Field::new("nation", DataType::Utf8, true),
        ]));
        let different_names = AgateTable::from_record_batch(Arc::new(
            RecordBatch::try_new(
                different_names_schema,
                vec![
                    Arc::new(Int32Array::from(vec![Some(1)])) as ArrayRef,
                    Arc::new(StringArray::from(vec![Some("Brazil")])) as ArrayRef,
                ],
            )
            .unwrap(),
        ));
        let different_types_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, true),
            Field::new("country", DataType::Int32, true),
        ]));
        let different_types = AgateTable::from_record_batch(Arc::new(
            RecordBatch::try_new(
                different_types_schema,
                vec![
                    Arc::new(Int32Array::from(vec![Some(1)])) as ArrayRef,
                    Arc::new(Int32Array::from(vec![Some(0)])) as ArrayRef,
                ],
            )
            .unwrap(),
        ));

        let out = render_globals(
            vec![
                ("a", left.into_value()),
                ("values", different_values.into_value()),
                ("names", different_names.into_value()),
                ("types", different_types.into_value()),
            ],
            "{{ a == values }} {{ a == names }} {{ a == types }}",
        );
        assert_eq!(out, "False False False");
    }

    #[test]
    fn table_eq_non_table_rhs_is_false() {
        let table = table_from_ids_and_countries(vec![Some(1)], vec![Some("Brazil")]);
        let out = render_globals(vec![("a", table.into_value())], "{{ a == 1 }} {{ a != 1 }}");
        assert_eq!(out, "False True");
    }

    #[test]
    fn table_eq_empty_tables_with_same_schema_are_equal() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, true),
            Field::new("country", DataType::Utf8, true),
        ]));
        let empty = || {
            AgateTable::from_record_batch(Arc::new(
                RecordBatch::try_new(
                    Arc::clone(&schema),
                    vec![
                        Arc::new(Int32Array::from(Vec::<Option<i32>>::new())) as ArrayRef,
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())) as ArrayRef,
                    ],
                )
                .unwrap(),
            ))
        };
        let nonempty = table_from_ids_and_countries(vec![Some(1)], vec![Some("Brazil")]);
        let out = render_globals(
            vec![
                ("a", empty().into_value()),
                ("b", empty().into_value()),
                ("c", nonempty.into_value()),
            ],
            "{{ a == b }} {{ a == c }}",
        );
        assert_eq!(out, "True False");
    }

    fn empty_id_text_table(text_column: &str) -> AgateTable {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, true),
            Field::new(text_column, DataType::Utf8, true),
        ]));
        AgateTable::from_record_batch(Arc::new(
            RecordBatch::try_new(
                schema,
                vec![
                    Arc::new(Int32Array::from(Vec::<Option<i32>>::new())) as ArrayRef,
                    Arc::new(StringArray::from(Vec::<Option<&str>>::new())) as ArrayRef,
                ],
            )
            .unwrap(),
        ))
    }

    #[test]
    fn table_eq_nested_empty_tables_with_different_names_are_unequal() {
        let a = empty_id_text_table("country");
        let b = empty_id_text_table("nation");
        let out = render_globals(
            vec![("a", a.into_value()), ("b", b.into_value())],
            "{{ [a] == [b] }} {{ a in [b] }}",
        );
        assert_eq!(out, "False False");
    }
}
