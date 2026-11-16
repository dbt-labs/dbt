use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, Int64Array, StringArray};
use arrow::compute::{CastOptions, cast_with_options};
use arrow::datatypes::{DataType, Field, FieldRef, Schema};
use arrow::record_batch::RecordBatch;

use dbt_adapter_core::AdapterType;
use dbt_adbc as adbc;

use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};

/// Information about a column that was renamed during disambiguation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenamedColumn<'a> {
    /// The original column name (duplicate).
    pub original: &'a str,
    /// The new unique column name (e.g., "col_2", "col_3").
    pub renamed: &'a str,
}

pub trait RecordBatchExt {
    fn meta_string(&self, key: &str) -> Option<String>;
    fn meta_i64(&self, key: &str) -> Option<i64>;
    fn first_value_as_i64(&self) -> Option<i64>;
    fn named_value_as_i64(&self, column_name: &str) -> Option<i64>;
    fn column_typed<'a>(&'a self, name: &str) -> AdapterResult<&'a Arc<dyn Array>>;
    fn column_values<T>(&self, column_name: &str) -> AdapterResult<T>
    where
        T: std::any::Any + Clone;
    fn disambiguate_column_names(
        self,
        on_disambiguate: Option<impl FnOnce(&[RenamedColumn<'_>])>,
    ) -> RecordBatch;
    /// Replace each nested column with a UTF-8 column of its JSON text.
    ///
    /// `jsonify_column` turns one nested column into its JSON strings.
    fn jsonify_nested_columns(
        self,
        jsonify_column: impl Fn(&FieldRef, &ArrayRef) -> StringArray,
    ) -> RecordBatch;
    /// The warehouse's query id, read from the schema metadata the driver attaches.
    fn query_id(&self, adapter_type: AdapterType) -> Option<String>;
    /// Lowercase all column names in the RecordBatch schema.
    ///
    /// This is used for database commands whose result column names are defined
    /// as lowercase even when a driver reports them in uppercase.
    fn lowercase_column_names(self) -> RecordBatch;
}

impl RecordBatchExt for RecordBatch {
    fn meta_string(&self, key: &str) -> Option<String> {
        self.schema().metadata().get(key).cloned()
    }

    fn meta_i64(&self, key: &str) -> Option<i64> {
        self.schema()
            .metadata()
            .get(key)
            .and_then(|v| v.parse::<i64>().ok())
    }

    fn first_value_as_i64(&self) -> Option<i64> {
        cast_column_to_i64(self.columns().first()?.as_ref())
    }

    fn named_value_as_i64(&self, column_name: &str) -> Option<i64> {
        let idx = self.schema().index_of(column_name).ok()?;
        cast_column_to_i64(self.column(idx).as_ref())
    }

    fn column_typed<'a>(&'a self, name: &str) -> AdapterResult<&'a Arc<dyn Array>> {
        self.column_by_name(name).ok_or_else(|| {
            let schema = self.schema();
            let columns = schema.fields().iter().map(|f| f.name()).collect::<Vec<_>>();
            AdapterError::new(
                AdapterErrorKind::Internal,
                format!("expected column {name} not found, available are: {columns:?}"),
            )
        })
    }

    fn column_values<T>(&self, column_name: &str) -> AdapterResult<T>
    where
        T: std::any::Any + Clone,
    {
        let column = self.column_typed(column_name)?;

        // Metadata/catalog queries are parsed by downcasting straight to a
        // concrete `StringArray`, but drivers/engines may now return
        // `Utf8View`/`LargeUtf8` for what used to always be plain `Utf8`
        // (e.g. the query-execution path unconditionally widens to view
        // types). Normalize down to `Utf8` first so callers expecting
        // `StringArray` keep working regardless of which string
        // representation produced the result.
        let normalized: ArrayRef = if std::any::TypeId::of::<T>()
            == std::any::TypeId::of::<StringArray>()
            && matches!(column.data_type(), DataType::Utf8View | DataType::LargeUtf8)
        {
            cast_with_options(column.as_ref(), &DataType::Utf8, &CastOptions::default())
                .map_err(|e| AdapterError::new(AdapterErrorKind::Internal, e.to_string()))?
        } else {
            column.clone()
        };

        Ok(normalized
            .as_any()
            .downcast_ref::<T>()
            .ok_or_else(|| {
                let schema = self.schema();
                let field = schema.fields().iter().find(|f| f.name() == column_name);
                AdapterError::new(
                    AdapterErrorKind::Internal,
                    format!(
                        "expected column of type: {} not found, available are: {field:?}",
                        std::any::type_name::<T>()
                    ),
                )
            })?
            .to_owned())
    }

    fn disambiguate_column_names(
        self,
        on_disambiguate: Option<impl FnOnce(&[RenamedColumn<'_>])>,
    ) -> RecordBatch {
        let schema = self.schema();
        let fields = schema.fields();

        let mut name_counts: HashMap<&str, usize> = HashMap::new();
        let mut new_names: Vec<String> = Vec::with_capacity(fields.len());

        for field in fields.iter() {
            let name = field.name().as_str();
            let count = name_counts.entry(name).or_insert(0);
            *count += 1;
            if *count > 1 {
                new_names.push(format!("{}_{}", name, count));
            } else {
                new_names.push(name.to_string());
            }
        }

        let renamed_columns: Vec<_> = fields
            .iter()
            .zip(new_names.iter())
            .filter(|(field, new_name)| field.name() != *new_name)
            .map(|(field, new_name)| RenamedColumn {
                original: field.name().as_str(),
                renamed: new_name.as_str(),
            })
            .collect();

        if renamed_columns.is_empty() {
            return self;
        }

        if let Some(callback) = on_disambiguate {
            callback(&renamed_columns);
        }

        let new_fields: Vec<_> = fields
            .iter()
            .zip(new_names.iter())
            .map(|(field, new_name)| Arc::new(field.as_ref().clone().with_name(new_name.clone())))
            .collect();

        let new_schema = Arc::new(Schema::new_with_metadata(
            new_fields,
            schema.metadata().clone(),
        ));

        RecordBatch::try_new(new_schema, self.columns().to_vec())
            .expect("disambiguate_column_names: schema and columns should be compatible")
    }

    /// Stringify nested columns, ignoring struct fields
    ///
    /// dbt Core flattens struct fields as JSON strings for certain adapters, only keeping the
    /// root-level columns.
    ///
    /// Reference: https://github.com/dbt-labs/dbt-common/blob/f21aa0d3093e98c7c35ed93163832c84f46eb3d8/dbt_common/clients/agate_helper.py#L110
    fn jsonify_nested_columns(
        self,
        jsonify_column: impl Fn(&FieldRef, &ArrayRef) -> StringArray,
    ) -> RecordBatch {
        let schema = self.schema();
        let fields = schema.fields();

        if !fields.iter().any(|f| f.data_type().is_nested()) {
            return self;
        }

        let mut new_fields: Vec<Arc<Field>> = Vec::with_capacity(fields.len());
        let mut new_columns: Vec<Arc<dyn Array>> = Vec::with_capacity(self.num_columns());

        for (field, column) in fields.iter().zip(self.columns().iter()) {
            if field.data_type().is_nested() {
                new_columns.push(Arc::new(jsonify_column(field, column)));
                new_fields.push(Arc::new(
                    Field::new(field.name(), DataType::Utf8, field.is_nullable())
                        .with_metadata(field.metadata().clone()),
                ));
            } else {
                new_columns.push(column.clone());
                new_fields.push(field.clone());
            }
        }

        let new_schema = Arc::new(Schema::new_with_metadata(
            new_fields,
            schema.metadata().clone(),
        ));
        RecordBatch::try_new(new_schema, new_columns)
            .expect("jsonify_nested_columns: rewritten schema and columns are consistent")
    }

    fn query_id(&self, adapter_type: AdapterType) -> Option<String> {
        match adapter_type {
            AdapterType::Snowflake => self.meta_string(adbc::snowflake::schema_metadata::QUERY_ID),
            AdapterType::Bigquery => self.meta_string(adbc::bigquery::schema_metadata::QUERY_ID),
            AdapterType::Databricks => {
                self.meta_string(adbc::databricks::schema_metadata::QUERY_ID)
            }
            AdapterType::LakeCompute => {
                self.meta_string(adbc::lake_compute::schema_metadata::QUERY_ID)
            }
            _ => None,
        }
    }

    fn lowercase_column_names(self) -> RecordBatch {
        let schema = self.schema();
        let fields = schema.fields();

        if fields.iter().all(|f| {
            f.name()
                .chars()
                .all(|c| c.is_lowercase() || !c.is_alphabetic())
        }) {
            return self;
        }

        let new_fields: Vec<_> = fields
            .iter()
            .map(|f| Arc::new(f.as_ref().clone().with_name(f.name().to_lowercase())))
            .collect();

        let new_schema = Arc::new(Schema::new_with_metadata(
            new_fields,
            schema.metadata().clone(),
        ));

        RecordBatch::try_new(new_schema, self.columns().to_vec())
            .expect("lowercase_column_names: schema and columns should be compatible")
    }
}

fn cast_column_to_i64(column: &dyn Array) -> Option<i64> {
    if column.is_empty() {
        return None;
    }
    let casted = cast_with_options(column, &DataType::Int64, &CastOptions::default())
        .inspect_err(|_| {
            debug_assert!(
                false,
                "cast_column_to_i64: unsupported data type {:?}",
                column.data_type()
            );
        })
        .ok()?;
    casted
        .as_any()
        .downcast_ref::<Int64Array>()?
        .iter()
        .next()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{ArrayRef, Float64Array, Int32Array, Int64Array, StringArray};
    use arrow::datatypes::{DataType, Field};

    macro_rules! assert_contains {
        ($actual:expr, $searched_for:expr $(,)?) => {
            match (&$actual, $searched_for) {
                (actual, searched_for) => {
                    assert!(
                        actual.contains(searched_for),
                        "Expected {actual:?} to contain {searched_for:?}"
                    );
                }
            }
        };
    }

    #[test]
    fn named_value_as_i64_missing_column_returns_none() {
        let schema = Schema::new(vec![Field::new("id", DataType::Int64, false)]);
        let batch =
            RecordBatch::try_new(Arc::new(schema), vec![Arc::new(Int64Array::from(vec![99]))])
                .unwrap();
        assert!(batch.named_value_as_i64("nonexistent").is_none());
    }

    use std::sync::LazyLock;

    static TEST_DATA: LazyLock<RecordBatch> = LazyLock::new(|| {
        let schema = Schema::new(vec![
            Field::new("name", DataType::Utf8, false),
            Field::new("score", DataType::Float64, false),
        ]);
        RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(StringArray::from(vec!["FOO"])),
                Arc::new(Float64Array::from(vec![42.0])),
            ],
        )
        .unwrap()
    });

    #[test]
    fn column_values_success() {
        let result: AdapterResult<StringArray> = TEST_DATA.column_values("name");
        assert!(result.is_ok());
    }

    #[test]
    fn column_values_column_not_found() {
        let result: AdapterResult<Int32Array> = TEST_DATA.column_values("nonexistent");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.kind(), AdapterErrorKind::Internal);
        assert_contains!(error.message(), "expected column nonexistent not found");
        assert_contains!(error.message(), "available are");
        assert_contains!(error.message(), "name");
        assert_contains!(error.message(), "score");
    }

    #[test]
    fn column_values_normalizes_utf8_view_to_string_array() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "name",
                DataType::Utf8View,
                false,
            )])),
            vec![Arc::new(arrow_array::StringViewArray::from(vec!["FOO"]))],
        )
        .unwrap();
        let result: AdapterResult<StringArray> = batch.column_values("name");
        assert_eq!(result.unwrap(), StringArray::from(vec!["FOO"]));
    }

    #[test]
    fn column_values_normalizes_large_utf8_to_string_array() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "name",
                DataType::LargeUtf8,
                false,
            )])),
            vec![Arc::new(arrow_array::LargeStringArray::from(vec!["FOO"]))],
        )
        .unwrap();
        let result: AdapterResult<StringArray> = batch.column_values("name");
        assert_eq!(result.unwrap(), StringArray::from(vec!["FOO"]));
    }

    #[test]
    fn column_values_wrong_type() {
        let result: AdapterResult<Int32Array> = TEST_DATA.column_values("name");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.kind(), AdapterErrorKind::Internal);
        assert_contains!(error.message(), "expected column of type");
        assert!(error.message().contains(
            "arrow_array::array::primitive_array::PrimitiveArray<arrow_array::types::Int32Type>"
        ));
    }

    #[test]
    fn disambiguate_no_duplicates() {
        let schema = Schema::new(vec![
            Field::new("a", DataType::Int32, false),
            Field::new("b", DataType::Int32, false),
            Field::new("c", DataType::Int32, false),
        ]);
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(Int32Array::from(vec![4, 5, 6])),
                Arc::new(Int32Array::from(vec![7, 8, 9])),
            ],
        )
        .unwrap();

        let callback_invoked = std::cell::Cell::new(false);
        let result = batch.disambiguate_column_names(Some(|_: &[RenamedColumn]| {
            callback_invoked.set(true);
        }));
        assert!(!callback_invoked.get());
        let schema = result.schema();
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn disambiguate_with_duplicates() {
        let schema = Schema::new(vec![
            Field::new("A", DataType::Int32, false),
            Field::new("B", DataType::Int32, false),
            Field::new("A", DataType::Int32, false),
            Field::new("A", DataType::Int32, false),
        ]);
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(Int32Array::from(vec![4, 5, 6])),
                Arc::new(Int32Array::from(vec![7, 8, 9])),
                Arc::new(Int32Array::from(vec![10, 11, 12])),
            ],
        )
        .unwrap();

        let captured = std::cell::RefCell::new(Vec::new());
        let result = batch.disambiguate_column_names(Some(|renamed: &[RenamedColumn]| {
            captured.borrow_mut().extend(
                renamed
                    .iter()
                    .map(|r| (r.original.to_string(), r.renamed.to_string())),
            );
        }));

        let renamed = captured.into_inner();
        assert_eq!(renamed.len(), 2);
        assert_eq!(renamed[0], ("A".to_string(), "A_2".to_string()));
        assert_eq!(renamed[1], ("A".to_string(), "A_3".to_string()));
        let schema = result.schema();
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["A", "B", "A_2", "A_3"]);
    }

    #[test]
    fn disambiguate_multiple_duplicates() {
        let schema = Schema::new(vec![
            Field::new("x", DataType::Int32, false),
            Field::new("y", DataType::Int32, false),
            Field::new("x", DataType::Int32, false),
            Field::new("y", DataType::Int32, false),
            Field::new("x", DataType::Int32, false),
        ]);
        let cols: Vec<_> = (0..5)
            .map(|_| Arc::new(Int32Array::from(vec![1])) as _)
            .collect();
        let batch = RecordBatch::try_new(Arc::new(schema), cols).unwrap();
        let result = batch.disambiguate_column_names(None::<fn(&[RenamedColumn])>);
        let schema = result.schema();
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["x", "y", "x_2", "y_2", "x_3"]);
    }

    #[test]
    fn lowercase_column_names_already_lowercase_is_noop() {
        let schema = Schema::new(vec![
            Field::new("column_name", DataType::Utf8, false),
            Field::new("data_type", DataType::Utf8, false),
        ]);
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(StringArray::from(vec!["id"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["integer"])) as ArrayRef,
            ],
        )
        .unwrap();

        let original_ptr = Arc::as_ptr(batch.schema_ref());
        let result = batch.lowercase_column_names();
        // Schema should be the same object (no rebuild needed)
        assert_eq!(Arc::as_ptr(result.schema_ref()), original_ptr);
        let schema = result.schema();
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["column_name", "data_type"]);
    }

    #[test]
    fn lowercase_column_names_snowflake_uppercase() {
        // Simulates what the Snowflake ADBC driver returns for SHOW / system queries.
        // dbt_constraints accesses columns["column_name"].values() (lowercase).
        // Without lowercasing, columns["column_name"] returns undefined.
        let schema = Schema::new(vec![
            Field::new("COLUMN_NAME", DataType::Utf8, false),
            Field::new("DATA_TYPE", DataType::Utf8, false),
        ]);
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(StringArray::from(vec!["id", "name"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["integer", "varchar"])) as ArrayRef,
            ],
        )
        .unwrap();

        let result = batch.lowercase_column_names();
        let schema = result.schema();
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["column_name", "data_type"]);

        // Data should be preserved unchanged.
        let col = result
            .column_by_name("column_name")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(col.value(0), "id");
        assert_eq!(col.value(1), "name");
    }

    #[test]
    fn lowercase_column_names_mixed_case() {
        let schema = Schema::new(vec![
            Field::new("Column_Name", DataType::Utf8, false),
            Field::new("id_42", DataType::Int32, false),
        ]);
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(StringArray::from(vec!["x"])) as ArrayRef,
                Arc::new(Int32Array::from(vec![1])) as ArrayRef,
            ],
        )
        .unwrap();

        let result = batch.lowercase_column_names();
        let schema = result.schema();
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["column_name", "id_42"]);
    }
}
