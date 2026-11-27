use crate::flat_record_batch::FlatRecordBatch;
use crate::test_fixtures::*;
use crate::*;
use arrow::array::{
    ArrayRef, BooleanBuilder, DictionaryArray, Float64Builder, Int32Array, Int32Builder,
    ListBuilder, StringBuilder, StringViewBuilder, StructBuilder,
};
use arrow::array::{GenericListArray, StringArray};
use arrow::csv::reader::ReaderBuilder;
use arrow::datatypes::{DataType, Field, Int32Type, Schema};
use arrow::record_batch::RecordBatch;
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, ListArray, RecordBatchOptions,
    builder::{FixedSizeListBuilder, Float32Builder},
};
use arrow_schema::Fields;
use minijinja::value::Kwargs;
use minijinja::value::ValueMap;
use minijinja::value::mutable_map::MutableMap;
use minijinja::{Environment, context};
use minijinja_contrib::testing::jinja_assert;
use std::io;
use std::sync::Arc;

#[test]
fn retain_original_values_with_separate_display_batch() {
    let original = simple_record_batch();
    let display = Arc::new(
        RecordBatch::try_from_iter(vec![
            (
                "id",
                Arc::new(StringArray::from(vec!["42", "43", "44"])) as ArrayRef,
            ),
            ("country", original.column(1).clone()),
        ])
        .unwrap(),
    );
    let table = AgateTable::from_record_batch_impl(display.clone(), Some(original.clone()), None);
    assert!(Arc::ptr_eq(&table.original_record_batch(), &original));
    assert_eq!(table.to_record_batch().column(0), display.column(0));
    assert_eq!(
        table
            .to_record_batch()
            .schema()
            .field(0)
            .metadata()
            .get("AGATE:dtype")
            .map(String::as_str),
        Some("Text")
    );
    let native_display = AgateTable::from_record_batch_impl(original.clone(), Some(original), None);
    assert_eq!(
        native_display
            .to_record_batch()
            .schema()
            .field(0)
            .metadata()
            .get("AGATE:dtype")
            .map(String::as_str),
        Some("Number")
    );
}

/// End-to-end regression test for dbt-labs/fs#15412.
///
/// This is the path from the bug report: a Snowflake `VECTOR(FLOAT, n)` column arrives
/// as Arrow `FixedSizeList(Float32, n)` and is read into an [AgateTable]. The flattener
/// forwards `FixedSizeList` as a flat column, so it reaches converter construction
/// inside [AgateTable::new] -- which unwraps, and therefore used to panic with
/// `CastError("Casting from FixedSizeList(..) to Utf8 not supported")`.
#[test]
fn agate_table_reads_snowflake_vector_column() {
    // select 'a' as id, [1.0, 2.0, 3.0]::vector(float, 3) as embedding
    let mut builder = FixedSizeListBuilder::new(Float32Builder::new(), 3);
    builder.values().append_slice(&[1.0, 2.0, 3.0]);
    builder.append(true);
    builder.values().append_slice(&[4.0, 5.0, 6.0]);
    builder.append(true);
    let embedding: ArrayRef = Arc::new(builder.finish());

    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, true),
        Field::new("embedding", embedding.data_type().clone(), true),
    ]));
    let id: ArrayRef = Arc::new(StringArray::from(vec![Some("a"), Some("b")]));
    let batch = Arc::new(RecordBatch::try_new(schema, vec![id, embedding]).unwrap());

    // Must not panic.
    let table = AgateTable::from_record_batch(batch);

    assert_eq!(table.num_rows(), 2);
    assert_eq!(table.num_columns(), 2);

    // The vector renders as a Jinja sequence, not a stringified blob.
    assert_eq!(
        table.cell(0, 1).unwrap(),
        Value::from(vec![
            Value::from(1.0f32),
            Value::from(2.0f32),
            Value::from(3.0f32)
        ])
    );
    assert_eq!(
        table.cell(1, 1).unwrap(),
        Value::from(vec![
            Value::from(4.0f32),
            Value::from(5.0f32),
            Value::from(6.0f32)
        ])
    );
}

/// A `FixedSizeList` column must not be expanded into one column per element by the
/// flattener -- it stays a single column whose cells are sequences.
#[test]
fn agate_table_keeps_fixed_size_list_as_single_column() {
    let values = Float32Array::from(vec![1.0, 2.0, 3.0, 4.0]);
    let field = Arc::new(Field::new("element", DataType::Float32, false));
    let embedding: ArrayRef = Arc::new(FixedSizeListArray::new(field, 2, Arc::new(values), None));

    let schema = Arc::new(Schema::new(vec![Field::new(
        "embedding",
        embedding.data_type().clone(),
        true,
    )]));
    let batch = Arc::new(RecordBatch::try_new(schema, vec![embedding]).unwrap());

    let table = AgateTable::from_record_batch(batch);

    assert_eq!(table.num_columns(), 1);
    assert_eq!(table.num_rows(), 2);
    assert_eq!(table.column_name(0).unwrap(), "embedding");
}

#[test]
fn jinja_sequence_value_matrix() {
    let table = Arc::new(main_table(&[1, 2], &["a", "b"], &[None, Some(1)]));
    let columns = Value::from_object(table.columns());
    let rows = Value::from_object(table.rows());
    let values = [
        ("columns", columns.clone(), 3),
        ("column", columns.get_item(&Value::from(0)).unwrap(), 2),
        ("rows", rows.clone(), 2),
        ("row", rows.get_item(&Value::from(0)).unwrap(), 3),
        (
            "tuple",
            Value::from_object(table.column_names_as_tuple().into_tuple()),
            3,
        ),
    ];
    let env = Environment::new();

    for (name, value, expected_len) in values {
        assert_eq!(value.kind(), ValueKind::Seq, "{name}");
        assert_eq!(
            env.render_str(
                "{{ value | list | length }}",
                context! { value => value },
                &[],
            )
            .unwrap(),
            expected_len.to_string(),
            "{name}",
        );
    }
}

#[test]
fn table_identity_same_object_is_equal() {
    let a = Value::from_object(main_table(&[1, 2], &["a", "b"], &[None, Some(1)]));
    let out = Environment::new()
        .render_str("{{ a == a }} {{ a != a }}", context! { a => a }, &[])
        .unwrap();
    assert_eq!(out, "True False");
}

#[test]
fn table_identity_equal_data_is_not_equal() {
    let a = Value::from_object(main_table(&[1, 2], &["a", "b"], &[None, Some(1)]));
    let b = Value::from_object(main_table(&[1, 2], &["a", "b"], &[None, Some(1)]));
    let out = Environment::new()
        .render_str(
            "{{ a == b }} {{ a != b }}",
            context! { a => a, b => b },
            &[],
        )
        .unwrap();
    assert_eq!(out, "False True");
}

#[test]
fn table_identity_empty_tables_are_not_equal() {
    let a = Value::from_object(main_table(&[], &[], &[]));
    let b = Value::from_object(main_table(&[], &[], &[]));
    let out = Environment::new()
        .render_str(
            "{{ a == b }} {{ a != b }}",
            context! { a => a, b => b },
            &[],
        )
        .unwrap();
    assert_eq!(out, "False True");
}

#[test]
fn table_identity_holds_inside_containers() {
    let a = Value::from_object(main_table(&[1, 2], &["a", "b"], &[None, Some(1)]));
    let b = Value::from_object(main_table(&[1, 2], &["a", "b"], &[None, Some(1)]));
    let out = Environment::new()
        .render_str(
            "{{ [a] == [b] }} {{ a in [b] }} {{ a in [a] }}",
            context! { a => a, b => b },
            &[],
        )
        .unwrap();
    assert_eq!(out, "False False True");
}

#[test]
fn table_identity_non_table_operand_is_not_equal() {
    let a = Value::from_object(main_table(&[1, 2], &["a", "b"], &[None, Some(1)]));
    let out = Environment::new()
        .render_str(
            "{{ a == 1 }} {{ a != 1 }} {{ 1 == a }} {{ 1 != a }}",
            context! { a => a },
            &[],
        )
        .unwrap();
    assert_eq!(out, "False True False True");
}

#[test]
fn test_columns() {
    let batch = simple_record_batch();
    let table = Arc::new(AgateTable::from_record_batch(batch));

    // there are 2 columns
    let columns = table.columns();
    let values = columns.values();
    assert_eq!(values.len(), 2);

    let id = values.get(0).unwrap();
    let country = values.get(1).unwrap();

    let id = id.as_object().unwrap();
    let country = country.as_object().unwrap();

    // each column contains 3 values
    assert_eq!(id.enumerator_len().unwrap(), 3);
    assert_eq!(country.enumerator_len().unwrap(), 3);
}

#[test]
fn test_select() {
    let batch = simple_record_batch();
    let table = AgateTable::from_record_batch(batch).into_value();

    let env = Environment::new();
    let state = env.empty_state();
    let select = |table: &Value, args: &[Value]| -> Result<Value, minijinja::Error> {
        table.call_method(&state, "select", args, &[])
    };

    let selected = select(
        &table,
        &[Value::from_iter([
            Value::from("country"),
            Value::from("id"),
            Value::from("country"),
        ])],
    )
    .unwrap()
    .downcast_object::<AgateTable>()
    .unwrap();

    assert_eq!(selected.num_columns(), 3);
    assert_eq!(selected.num_rows(), 3);

    assert_eq!(selected.column_name(0).unwrap(), "country");
    assert_eq!(selected.column_name(1).unwrap(), "id");
    assert_eq!(selected.column_name(2).unwrap(), "country");

    let cols = selected.columns().values();
    let country = cols.get(2).unwrap();
    assert_eq!(country.len(), Some(3));
    assert_eq!(
        country.get_item_by_index(0).unwrap().as_str().unwrap(),
        "Brazil"
    );
    assert_eq!(
        country.get_item_by_index(1).unwrap().as_str().unwrap(),
        "USA"
    );
    assert_eq!(
        country.get_item_by_index(2).unwrap().as_str().unwrap(),
        "Canada"
    );

    // select 0 columns
    let selected = select(&table, &[Value::from_iter([] as [Value; 0])])
        .unwrap()
        .downcast_object::<AgateTable>()
        .unwrap();
    // result has 0 columns and 3 rows
    assert_eq!(selected.num_columns(), 0);
    assert_eq!(selected.num_rows(), 3);
}

#[test]
fn test_rows() {
    let table = AgateTable::from_record_batch(simple_record_batch());
    let rows = table.rows();
    let values = rows.values();
    assert_eq!(values.len(), 3);
}

fn country_at(table: &AgateTable, idx: usize) -> String {
    let table_arc = Arc::new(table.clone());
    let cols = table_arc.columns().values();
    let country = cols.get(1).unwrap();
    country
        .get_item_by_index(idx)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn test_limit_zero_preserves_schema() {
    let table = AgateTable::from_record_batch(simple_record_batch());
    assert_eq!(table.num_rows(), 3);
    let empty = table.limit(0).unwrap();
    assert_eq!(empty.num_rows(), 0);
    assert_eq!(empty.num_columns(), 2);
    assert_eq!(
        empty.column_names(),
        vec!["id".to_string(), "country".to_string()]
    );
    assert_eq!(empty.column_types().len(), 2);
}

#[test]
fn test_limit_single_arg_takes_first_n() {
    let table = AgateTable::from_record_batch(simple_record_batch());
    let limited = table.limit(2).unwrap();
    assert_eq!(limited.num_rows(), 2);
    assert_eq!(country_at(&limited, 0), "Brazil");
    assert_eq!(country_at(&limited, 1), "USA");
}

#[test]
fn test_limit_exceeds_returns_all_rows() {
    let table = AgateTable::from_record_batch(simple_record_batch());
    let limited = table.limit(100).unwrap();
    assert_eq!(limited.num_rows(), 3);
}

#[test]
fn test_limit_negative_drops_rows_from_the_end() {
    let table = AgateTable::from_record_batch(simple_record_batch());
    let limited = table.limit(-1).unwrap();
    assert_eq!(limited.num_rows(), 2);
    assert_eq!(country_at(&limited, 0), "Brazil");
    assert_eq!(country_at(&limited, 1), "USA");
    assert_eq!(table.limit(-10).unwrap().num_rows(), 0);
}

/// A table with a single `id` column holding `0..5` and row names `r0..r4`.
fn five_row_table() -> Value {
    let ids: ArrayRef = Arc::new(Int32Array::from_iter_values(0..5));
    let batch = RecordBatch::try_from_iter([("id", ids)]).unwrap();
    let row_names = {
        let mut builder = StringViewBuilder::with_capacity(5);
        for i in 0..5 {
            builder.append_value(format!("r{i}"));
        }
        Arc::new(builder.finish())
    };
    AgateTable::new(Arc::new(batch), Some(row_names)).into_value()
}

fn render_limit(table: &Value, args: &str) -> Result<String, minijinja::Error> {
    Environment::new().render_str(
        &format!(
            "{{% set l = t.limit({args}) %}}\
                 {{% for r in l.rows %}}{{{{ r[0] }}}} {{% endfor %}}\
                 | {{% for n in l.row_names %}}{{{{ n }}}} {{% endfor %}}"
        ),
        context! { t => table.clone() },
        &[],
    )
}

/// The expected ids are what Python agate's `limit()` returns for a table
/// with rows `0..5`, including its `if stop or step:` quirk.
#[test]
fn test_limit_follows_python_slice_semantics() {
    let table = five_row_table();
    let cases: &[(&str, &[i32])] = &[
        ("", &[0, 1, 2, 3, 4]),
        ("2", &[0, 1]),
        ("-1", &[0, 1, 2, 3]),
        ("-10", &[]),
        ("10", &[0, 1, 2, 3, 4]),
        ("0", &[]),
        ("none", &[0, 1, 2, 3, 4]),
        ("1, 3", &[1, 2]),
        ("1, none, 2", &[1, 3]),
        ("none, none, -1", &[4, 3, 2, 1, 0]),
        ("-2, none, -2", &[3, 1]),
        ("3, 1, -1", &[3, 2]),
        // `stop` and `step` of 0 are falsy, so agate falls back to `slice(start_or_stop)`
        ("2, 0", &[0, 1]),
        ("1, none, 0", &[0]),
        ("1, 0, 2", &[]),
        ("start_or_stop=1, stop=4", &[1, 2, 3]),
        ("1, step=2", &[1, 3]),
        ("stop=3", &[0, 1, 2]),
    ];
    for (args, expected) in cases {
        let ids: String = expected.iter().map(|i| format!("{i} ")).collect();
        let names: String = expected.iter().map(|i| format!("r{i} ")).collect();
        let actual = render_limit(&table, args).unwrap();
        assert_eq!(actual, format!("{ids}| {names}"), "limit({args})");
    }
}

#[test]
fn test_limit_rejects_invalid_arguments() {
    let table = five_row_table();

    let err = render_limit(&table, "1, 3, 0").unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidOperation);
    assert!(
        err.to_string().contains("slice step cannot be zero"),
        "{err}"
    );

    let err = render_limit(&table, "1, 2, 3, 4").unwrap_err();
    assert_eq!(err.kind(), ErrorKind::TooManyArguments, "{err}");

    let err = render_limit(&table, "n=2").unwrap_err();
    assert_eq!(err.kind(), ErrorKind::TooManyArguments, "{err}");

    assert!(render_limit(&table, "'2'").is_err());
}

#[test]
fn test_table_columns_and_object_properties() {
    let table = AgateTable::new_with_single_row_name(simple_record_batch(), "row-name");
    let table_value = table.into_value();
    let env = Environment::new();
    let state = env.empty_state();

    let column_names = table_value.get_attr("column_names").unwrap();
    assert_eq!(column_names.len(), Some(2));
    assert_eq!(
        column_names.get_item_by_index(0).unwrap(),
        Value::from("id")
    );
    assert_eq!(
        column_names.get_item_by_index(1).unwrap(),
        Value::from("country")
    );

    let column_types = table_value.get_attr("column_types").unwrap();
    assert_eq!(column_types.len(), Some(2));
    assert_eq!(
        column_types
            .get_item_by_index(0)
            .unwrap()
            .downcast_object_ref::<crate::DataType>()
            .unwrap()
            .type_name(),
        "Number"
    );
    assert_eq!(
        column_types
            .get_item_by_index(1)
            .unwrap()
            .downcast_object_ref::<crate::DataType>()
            .unwrap()
            .type_name(),
        "Text"
    );

    let columns = table_value.get_attr("columns").unwrap();
    assert_eq!(columns.len(), Some(2));
    let keys = columns.call_method(&state, "keys", &[], &[]).unwrap();
    assert_eq!(keys.get_item_by_index(1).unwrap(), Value::from("country"));
    let id_column = columns.get_item_by_index(0).unwrap();
    assert_eq!(id_column.get_attr("index").unwrap(), Value::from(0usize));
    assert_eq!(id_column.get_attr("name").unwrap(), Value::from("id"));
    assert_eq!(id_column.len(), Some(3));
    assert_eq!(id_column.get_item_by_index(0).unwrap(), Value::from(42));
    assert_eq!(id_column.get_item_by_index(2).unwrap(), Value::from(44));
    assert_eq!(
        id_column
            .get_attr("data_type")
            .unwrap()
            .downcast_object_ref::<crate::DataType>()
            .unwrap()
            .type_name(),
        "Number"
    );
    let columns_items = columns.call_method(&state, "items", &[], &[]).unwrap();
    assert_eq!(columns_items.len(), Some(2));
    assert_eq!(
        columns_items
            .get_item_by_index(0)
            .unwrap()
            .get_item_by_index(0)
            .unwrap(),
        Value::from("id")
    );
    let country_column = columns
        .call_method(&state, "get", &[Value::from("country")], &[])
        .unwrap();
    assert_eq!(
        country_column.get_attr("name").unwrap(),
        Value::from("country")
    );
}

#[test]
fn test_table_rows_and_row_name_properties() {
    let table = AgateTable::new_with_single_row_name(simple_record_batch(), "row-name");
    let table_value = table.into_value();
    let env = Environment::new();
    let state = env.empty_state();

    let rows = table_value.get_attr("rows").unwrap();
    assert_eq!(rows.len(), Some(3));
    let row_keys = rows.call_method(&state, "keys", &[], &[]).unwrap();
    assert_eq!(
        row_keys.get_item_by_index(0).unwrap(),
        Value::from("row-name")
    );
    let first_row = rows.get_item_by_index(0).unwrap();
    assert_eq!(first_row.get_attr("id").unwrap(), Value::from(42));
    assert_eq!(
        first_row.get_attr("country").unwrap(),
        Value::from("Brazil")
    );
    let rows_items = rows.call_method(&state, "items", &[], &[]).unwrap();
    assert_eq!(rows_items.len(), Some(3));
    assert_eq!(
        rows_items
            .get_item_by_index(0)
            .unwrap()
            .get_item_by_index(0)
            .unwrap(),
        Value::from("row-name")
    );

    let row_names = table_value.get_attr("row_names").unwrap();
    assert_eq!(row_names.len(), Some(3));
    assert_eq!(
        row_names.get_item_by_index(2).unwrap(),
        Value::from("row-name")
    );
}

#[test]
fn test_table_with_single_row_name() {
    let table = AgateTable::new_with_single_row_name(simple_record_batch(), "The Row Name");
    let row_names: Tuple = table.row_names().unwrap();
    for i in 0..table.num_rows() {
        let name = row_names.get(i as isize).unwrap();
        assert_eq!(name.as_str().unwrap(), "The Row Name");
    }
    let the_row_name = Value::from("The Row Name");
    let not_the_row_name = Value::from("Not The Row Name");
    assert_eq!(row_names.count(&the_row_name), table.num_rows());
    assert_eq!(row_names.count(&not_the_row_name), 0);
}

#[test]
fn test_table_with_multiple_row_names() {
    let row_names = {
        let mut builder = StringViewBuilder::with_capacity(3);
        builder.append_value("Row 1");
        builder.append_value("Row 2");
        builder.append_value("Row 3");
        Arc::new(builder.finish())
    };
    let table = AgateTable::new(simple_record_batch(), Some(row_names));
    let row_names: Tuple = table.row_names().unwrap();
    for i in 0..table.num_rows() {
        let name = row_names.get(i as isize).unwrap();
        assert_eq!(name.as_str().unwrap(), format!("Row {}", i + 1));
    }
    for i in 0..table.num_rows() {
        let name = Value::from(format!("Row {}", i + 1));
        assert_eq!(row_names.count(&name), 1);
    }
    let row_2_name = Value::from("Row 2");
    assert_eq!(row_names.count(&row_2_name), 1);

    // Now get the rows via the Jinja API
    let table = table.into_value();
    let row_names = table.get_attr("row_names").unwrap();
    row_names
        .try_iter()
        .unwrap()
        .enumerate()
        .for_each(|(i, name)| {
            assert_eq!(name.as_str().unwrap(), format!("Row {}", i + 1));
        });

    // We can also get it as a property from the table object
    let row_names_prop = table.get_attr("row_names").unwrap();
    assert_eq!(row_names_prop, row_names);
}

#[test]
fn test_agate_table_from_value() {
    let file = io::Cursor::new(
        "grantee,privilege_type\n\
 dbt_test_user_1,SELECT\n\
 dbt_test_user_2,SELECT\n\
 dbt_test_user_3,SELECT\n",
    );
    let csv_schema = Schema::new(vec![
        Field::new("grantee", DataType::Utf8, true),
        Field::new("privilege_type", DataType::Utf8, true),
    ]);
    let mut reader = ReaderBuilder::new(Arc::new(csv_schema))
        .with_header(true)
        .build(file)
        .unwrap();
    let batch = reader.next().unwrap().unwrap();
    let table = AgateTable::from_record_batch(Arc::new(batch)).into_value();

    let downcasted = table.downcast_object::<AgateTable>().unwrap();
    assert_eq!(downcasted.num_columns(), 2);
    assert_eq!(downcasted.num_rows(), 3);
    let record_batch = downcasted.original_record_batch();
    assert_eq!(record_batch.num_columns(), 2);
    assert_eq!(record_batch.num_rows(), 3);
}

/// Create a nested record batch with different data types.
///
/// NOTE: other tests may use a JSON->Arrow parser to create record batches more
/// easily, but let's keep this one as an example on how to use builders to create
/// record batches imperatively.
///
/// The data in the record batch is what the following SQL would generate:
///
/// ```sql
/// INSERT INTO user_events (id, user_name, event_tags, event_meta, groups) VALUES
///   (1, 'alice',   ARRAY['login', 'mobile'],   '{"device": "iPhone", "success": true}',
///     ARRAY[
///       ARRAY[1, 2, 3],
///       ARRAY[4, 5],
///       ARRAY[6]
///     ]),
///   (2, 'bob',     ARRAY['purchase'],          '{"item_id": 1234, "amount": 49.99}',
///     ARRAY[
///       ARRAY[10, 20],
///       ARRAY[30, 40, 50],
///       ARRAY[60, 70],
///       ARRAY[80]
///     ]),
///   (3, 'charlie', ARRAY['logout', 'timeout'], '{"duration_sec": 300}',
///     ARRAY[
///       ARRAY[7],
///       NULL,
///       ARRAY[8, 9]
///     ]),
///   (4, 'dana',    ARRAY[]::TEXT[],            '{"device": "desktop"}',
///     ARRAY[]::INTEGER[][]),  -- Empty outer list
///   (5, 'eve',     NULL,                       '{"success": false}',
///     NULL)
///   );
/// ```
fn nested_record_batch() -> RecordBatch {
    const CAPACITY: usize = 5;
    // all the missing fields become NULL in the record batch
    let event_type_fields = Fields::from(vec![
        Field::new("device", DataType::Utf8, true),
        Field::new("item_id", DataType::Int32, true),
        Field::new("amount", DataType::Float64, true),
        Field::new("duration_sec", DataType::Int32, true),
        Field::new("success", DataType::Boolean, true),
    ]);
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int32, false),
        Field::new("name", DataType::Utf8, false),
        Field::new(
            "event_tags",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, false))),
            true,
        ),
        Field::new(
            "event_meta",
            DataType::Struct(event_type_fields.clone()),
            false,
        ),
        Field::new(
            "groups",
            DataType::List(Arc::new(Field::new(
                "item",
                DataType::List(Arc::new(Field::new("item", DataType::Int32, true))),
                true,
            ))),
            true,
        ),
    ]));
    let id_array: ArrayRef = Arc::new(Int32Array::from(vec![
        Some(1),
        Some(2),
        Some(3),
        Some(4),
        Some(5),
    ]));
    let user_name_array: ArrayRef = Arc::new(StringArray::from(vec![
        Some("alice"),
        Some("bob"),
        Some("charlie"),
        Some("dana"),
        Some("eve"),
    ]));
    let event_tags_array = {
        let mut event_tags_builder = {
            let values_builder = StringBuilder::with_capacity(CAPACITY, CAPACITY * 10);
            ListBuilder::<StringBuilder>::with_capacity(values_builder, CAPACITY)
        };
        event_tags_builder.append_value(vec![Some("login"), Some("mobile")]);
        event_tags_builder.append_value(vec![Some("purchase")]);
        event_tags_builder.append_value(vec![Some("logout"), Some("timeout")]);
        event_tags_builder.append_value(Vec::<Option<String>>::new());
        event_tags_builder.append_null();

        let list_array = event_tags_builder.finish();
        // re-create the list array with a non-nullable field because finish()
        // doesn't let us specify the nullability of the list field
        let new_list_field = Field::new_list_field(
            list_array.values().data_type().clone(),
            false, // the values are non-nullable!
        );
        let event_tags_array = GenericListArray::new(
            Arc::new(new_list_field),
            list_array.offsets().clone(),
            list_array.values().clone(),
            None,
        );
        Arc::new(event_tags_array)
    };

    let events_array = {
        let mut event_builder = StructBuilder::from_fields(event_type_fields, CAPACITY);
        let mut append = |device: Option<&str>,
                          item_id: Option<i32>,
                          amount: Option<f64>,
                          duration_sec: Option<i32>,
                          success: Option<bool>| {
            event_builder
                .field_builder::<StringBuilder>(0)
                .unwrap()
                .append_option(device.to_owned());
            event_builder
                .field_builder::<Int32Builder>(1)
                .unwrap()
                .append_option(item_id);
            event_builder
                .field_builder::<Float64Builder>(2)
                .unwrap()
                .append_option(amount);
            event_builder
                .field_builder::<Int32Builder>(3)
                .unwrap()
                .append_option(duration_sec);
            event_builder
                .field_builder::<BooleanBuilder>(4)
                .unwrap()
                .append_option(success);
            event_builder.append(true);
        };
        append(Some("iPhone"), None, None, None, Some(true));
        append(None, Some(1234), Some(49.99), None, None);
        append(None, None, None, Some(300), None);
        append(Some("Desktop"), None, None, None, None);
        append(None, None, None, None, Some(false));
        Arc::new(event_builder.finish())
    };

    let groups_array = {
        let mut groups_builder = {
            let inner_values_builder = Int32Builder::new();
            let inner_list_builder = ListBuilder::<Int32Builder>::new(inner_values_builder);
            ListBuilder::<ListBuilder<Int32Builder>>::with_capacity(inner_list_builder, CAPACITY)
        };
        let inner_list = groups_builder.values();
        inner_list.append_value(vec![Some(1), Some(2), Some(3)]);
        inner_list.append_value(vec![Some(4), Some(5)]);
        inner_list.append_value(vec![Some(6)]);
        groups_builder.append(true); // groups 0

        let inner_list = groups_builder.values();
        inner_list.append_value(vec![Some(10), Some(20)]);
        inner_list.append_value(vec![Some(30), Some(40), Some(50)]);
        inner_list.append_value(vec![Some(60), Some(70)]);
        inner_list.append_value(vec![Some(80)]);
        groups_builder.append(true); // groups 1

        let inner_list = groups_builder.values();
        inner_list.append_value(vec![Some(7)]);
        inner_list.append_null();
        inner_list.append_value(vec![Some(8), Some(9)]);
        groups_builder.append(true); // groups 2

        // []   -- Empty list of groups (non-NULL)
        groups_builder.append(true); // groups 3

        // NULL -- Null list of groups
        groups_builder.append(false); // groups 4

        Arc::new(groups_builder.finish())
    };

    let columns = vec![
        id_array,
        user_name_array,
        event_tags_array,
        events_array,
        groups_array,
    ];
    RecordBatch::try_new(schema, columns).unwrap()
}

#[test]
fn test_record_batch_flattening() {
    let batch = nested_record_batch();
    let _batch = FlatRecordBatch::try_new(Arc::new(batch)).unwrap();
    // TODO(felipcrv); implement CSV serialization to assert here
}

/// Take a 5-element column and make a dictionary-encoded version of it
/// using the first two elements as dictionary values.
fn dict_encoded_example(col: &ArrayRef) -> ArrayRef {
    let dictionary_values = col.slice(0, 2);
    let indices_array = Int32Array::from(vec![Some(0), Some(1), Some(0), Some(1), Some(0)]);
    let dict_array =
        DictionaryArray::<Int32Type>::try_new(indices_array, dictionary_values).unwrap();
    Arc::new(dict_array) as ArrayRef
}

fn batch_with_replaced_column(
    batch: &RecordBatch,
    col_idx: usize,
    new_col: ArrayRef,
) -> RecordBatch {
    let new_columns = batch
        .columns()
        .iter()
        .enumerate()
        .map(|(i, col)| {
            if i == col_idx {
                new_col.clone()
            } else {
                col.clone()
            }
        })
        .collect::<Vec<_>>();
    let new_schema = {
        let old_schema = batch.schema();
        let fields = new_columns
            .iter()
            .enumerate()
            .map(|(i, col)| {
                old_schema
                    .field(i)
                    .clone()
                    .with_data_type(col.data_type().clone())
            })
            .collect::<Vec<_>>();
        Arc::new(Schema::new(fields))
    };
    RecordBatch::try_new(new_schema, new_columns).unwrap()
}

#[test]
fn test_record_batch_flattening_with_dict_encoded_struct() {
    let batch = nested_record_batch();

    let event_meta = batch.column(3);
    assert!(matches!(event_meta.data_type(), DataType::Struct(_)));

    // Build a dictionary-encoded version of the "events" column
    let dict_event_meta = dict_encoded_example(event_meta);

    let new_batch = batch_with_replaced_column(&batch, 3, dict_event_meta);
    let _flat_batch = FlatRecordBatch::try_new(Arc::new(new_batch)).unwrap();
    // TODO(felipcrv); implement CSV serialization to assert here
}

#[test]
fn test_record_batch_flattening_with_dict_encoded_list() {
    let batch = nested_record_batch();

    let event_tags = batch.column(2);
    assert!(matches!(event_tags.data_type(), DataType::List(_)));

    // Build a dictionary-encoded version of the "event_tags" column
    let dict_event_tags = dict_encoded_example(event_tags);

    let new_batch = batch_with_replaced_column(&batch, 2, dict_event_tags);
    let _flat_batch = FlatRecordBatch::try_new(Arc::new(new_batch)).unwrap();
    // TODO(felipcrv); implement CSV serialization to assert here
}

#[test]
fn test_record_batch_flattening_with_nested_dict_encoded() {
    let batch = nested_record_batch();

    let event_meta = batch.column(3);
    assert!(matches!(event_meta.data_type(), DataType::Struct(_)));

    // Build a dictionary-encoded version of the "events" column...
    let dict_event_meta = dict_encoded_example(event_meta);
    // ...and dictionary-encode it again.
    let dict_dict_event_meta = dict_encoded_example(&dict_event_meta);

    let new_batch = batch_with_replaced_column(&batch, 3, dict_dict_event_meta);
    let _flat_batch = FlatRecordBatch::try_new(Arc::new(new_batch)).unwrap();
    // TODO(felipcrv); implement CSV serialization to assert here
}

#[test]
#[allow(clippy::cognitive_complexity)]
fn test_empty_batch_column_types_and_names() {
    let contact_fields = Fields::from(vec![
        Field::new("first", DataType::Utf8, true),
        Field::new("last", DataType::Utf8, true),
    ]);
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int32, false),
        Field::new("name", DataType::Utf8, false),
        Field::new(
            "event_tags",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, false))),
            true,
        ),
        Field::new("contact", DataType::Struct(contact_fields.clone()), true),
    ]));
    let mut contact_builder = StructBuilder::from_fields(contact_fields, 0);
    let opts = RecordBatchOptions::default().with_row_count(Some(0));
    let batch = RecordBatch::try_new_with_options(
        schema,
        vec![
            Arc::new(Int32Array::new_null(0)) as ArrayRef,
            Arc::new(StringArray::new_null(0)) as ArrayRef,
            Arc::new(ListArray::new_null(
                Arc::new(Field::new_list_field(
                    DataType::Utf8,
                    false, // the values are non-nullable!
                )),
                0,
            )) as ArrayRef,
            Arc::new(contact_builder.finish()) as ArrayRef,
        ],
        &opts,
    )
    .unwrap();
    let table = AgateTable::from_record_batch(Arc::new(batch));
    let column_types = table.column_types();
    assert_eq!(
        column_types,
        vec![
            crate::DataType::new("Number".to_string()),
            crate::DataType::new("Text".to_string()),
            crate::DataType::new("Text".to_string()),
            crate::DataType::new("Text".to_string()),
            crate::DataType::new("Text".to_string()),
        ]
    );
    let column_names = table.column_names();
    assert_eq!(
        column_names,
        vec![
            "id",
            "name",
            "event_tags.0",
            "contact/first",
            "contact/last",
        ]
    );
    assert_eq!(
        table
            .repr
            .flat
            .top_level_column_names()
            .collect::<Vec<&String>>(),
        vec!["id", "name", "event_tags", "contact"]
    );
    jinja_assert(
        table.clone(),
        "{{ obj.top_level_column_names | join(',') }}",
        "id,name,event_tags,contact",
    );

    let column_types = table.column_types_as_tuple();
    assert_eq!(column_types.len(), 5);

    // column_types now returns DataType objects, not strings
    let dt0 = column_types.get_item_by_index(0).unwrap();
    let dt0_obj = dt0.downcast_object_ref::<crate::DataType>().unwrap();
    assert_eq!(dt0_obj.type_name(), "Number");

    let dt1 = column_types.get_item_by_index(1).unwrap();
    let dt1_obj = dt1.downcast_object_ref::<crate::DataType>().unwrap();
    assert_eq!(dt1_obj.type_name(), "Text");

    let dt2 = column_types.get_item_by_index(2).unwrap();
    let dt2_obj = dt2.downcast_object_ref::<crate::DataType>().unwrap();
    assert_eq!(dt2_obj.type_name(), "Text");

    let dt3 = column_types.get_item_by_index(3).unwrap();
    let dt3_obj = dt3.downcast_object_ref::<crate::DataType>().unwrap();
    assert_eq!(dt3_obj.type_name(), "Text");

    let dt4 = column_types.get_item_by_index(4).unwrap();
    let dt4_obj = dt4.downcast_object_ref::<crate::DataType>().unwrap();
    assert_eq!(dt4_obj.type_name(), "Text");

    assert_eq!(column_types.count_occurrences_of(&Value::from("Number")), 1);
    assert_eq!(column_types.count_occurrences_of(&Value::from("Text")), 4);
    assert_eq!(
        column_types.count_occurrences_of(&Value::from("DateTime")),
        0
    );
    assert_eq!(column_types.index_of(&Value::from("Number")), Some(0));
    assert_eq!(column_types.index_of(&Value::from("Text")), Some(1));
    assert_eq!(column_types.index_of(&Value::from("DateTime")), None);
    let column_types2 = table.column_types_as_tuple();
    assert!(column_types.eq_repr(&column_types2 as &dyn TupleRepr));

    let column_names = table.column_names_as_tuple();
    assert_eq!(column_names.len(), 5);
    assert_eq!(
        column_names.get_item_by_index(0).unwrap().as_str().unwrap(),
        "id"
    );
    assert_eq!(
        column_names.get_item_by_index(1).unwrap().as_str().unwrap(),
        "name"
    );
    assert_eq!(
        column_names.get_item_by_index(2).unwrap().as_str().unwrap(),
        "event_tags.0"
    );
    assert_eq!(
        column_names.get_item_by_index(3).unwrap().as_str().unwrap(),
        "contact/first"
    );
    assert_eq!(
        column_names.get_item_by_index(4).unwrap().as_str().unwrap(),
        "contact/last"
    );
    assert_eq!(column_names.count_occurrences_of(&Value::from("id")), 1);
    assert_eq!(column_names.count_occurrences_of(&Value::from("name")), 1);
    assert_eq!(
        column_names.count_occurrences_of(&Value::from("event_tags.0")),
        1
    );
    assert_eq!(
        column_names.count_occurrences_of(&Value::from("contact/first")),
        1
    );
    assert_eq!(
        column_names.count_occurrences_of(&Value::from("contact/last")),
        1
    );
    assert_eq!(
        column_names.count_occurrences_of(&Value::from("nonexistent")),
        0
    );
    assert_eq!(column_names.index_of(&Value::from("id")), Some(0));
    assert_eq!(column_names.index_of(&Value::from("name")), Some(1));
    assert_eq!(column_names.index_of(&Value::from("event_tags.0")), Some(2));
    assert_eq!(
        column_names.index_of(&Value::from("contact/first")),
        Some(3)
    );
    assert_eq!(column_names.index_of(&Value::from("contact/last")), Some(4));
    assert_eq!(column_names.index_of(&Value::from("nonexistent")), None);
    let column_names2 = table.column_names_as_tuple();
    assert!(column_names.eq_repr(&column_names2 as &dyn TupleRepr));
}

#[test]
fn test_column_renaming() {
    let batch = Arc::new(nested_record_batch());
    let agate_table = AgateTable::from_record_batch(batch);
    let col_names = agate_table.column_names();
    let table = agate_table.into_value();

    let env = Environment::new();
    let state = env.empty_state();
    let rename = |table: &Value, args: &[Value]| -> Result<Value, minijinja::Error> {
        table.call_method(&state, "rename", args, &[])
    };

    // Original column names:
    //   "id", "name", "event_tags.0", "event_tags.1", "event_meta/device",
    //   "event_meta/item_id", "event_meta/amount", "event_meta/duration_sec",
    //   "event_meta/success", "groups.0", "groups.1", "groups.2", "groups.3"

    // Renaming with a map
    let map = ValueMap::from_iter([
        ("groups.0.0".into(), "first_group_cell".into()),
        ("groups.1.0".into(), "second_group_cell".into()),
        ("groups.2.0".into(), "third_group_cell".into()),
        ("groups.3.0".into(), "fourth_group_cell".into()),
        ("nonexistent".into(), "should_not_exist".into()),
    ]);
    let new_names = rename(&table, &[Value::from_object(map.clone())])
        .unwrap()
        .downcast_object::<AgateTable>()
        .unwrap()
        .column_names();
    assert_eq!(
        new_names[0..new_names.len() - 12],
        col_names[0..col_names.len() - 12]
    );
    static EXPECTED_12_LAST_COL_NAMES: [&str; 12] = [
        "first_group_cell",
        "groups.0.1",
        "groups.0.2",
        "second_group_cell",
        "groups.1.1",
        "groups.1.2",
        "third_group_cell",
        "groups.2.1",
        "groups.2.2",
        "fourth_group_cell",
        "groups.3.1",
        "groups.3.2",
    ];
    assert_eq!(
        new_names[new_names.len() - 12..],
        EXPECTED_12_LAST_COL_NAMES
    );

    // Renaming with a mutable map
    let map: MutableMap = map.into();
    let new_names = rename(&table, &[Value::from_object(map)])
        .unwrap()
        .downcast_object::<AgateTable>()
        .unwrap()
        .column_names();
    assert_eq!(
        new_names[0..new_names.len() - 12],
        col_names[0..col_names.len() - 12]
    );
    assert_eq!(
        new_names[new_names.len() - 12..],
        EXPECTED_12_LAST_COL_NAMES
    );

    // Renaming with an array
    let array = {
        let mut array = col_names[0..col_names.len() - 12].to_vec();
        array.extend_from_slice(EXPECTED_12_LAST_COL_NAMES.map(|s| s.to_string()).as_slice());
        Value::from_object(array)
    };
    let new_names = rename(&table, &[array])
        .unwrap()
        .downcast_object::<AgateTable>()
        .unwrap()
        .column_names();
    assert_eq!(
        new_names[0..new_names.len() - 12],
        col_names[0..col_names.len() - 12]
    );
    assert_eq!(
        new_names[new_names.len() - 12..],
        EXPECTED_12_LAST_COL_NAMES
    );
}

#[test]
fn test_row_renaming() {
    let batch = simple_record_batch();
    let table = AgateTable::from_record_batch(batch).into_value();

    let env = Environment::new();
    let state = env.empty_state();
    let rename = |table: &Value, args: &[Value]| -> Result<Value, minijinja::Error> {
        table.call_method(&state, "rename", args, &[])
    };

    // Original row names are undefined
    let original_row_names = table.get_attr("row_names").unwrap();
    assert!(original_row_names.get_item_by_index(0).is_err());

    // Renaming with an array
    let array = Value::from_object(vec![
        "Row 1".to_string(),
        "Row 2".to_string(),
        "Row 3".to_string(),
    ]);
    let table = rename(&table, &[Value::from(()), array]).unwrap();
    let new_names = table
        .downcast_object::<AgateTable>()
        .unwrap()
        .row_names()
        .unwrap();
    assert_eq!(new_names.len(), 3);
    assert_eq!(new_names.get(0).unwrap().as_str().unwrap(), "Row 1");
    assert_eq!(new_names.get(1).unwrap().as_str().unwrap(), "Row 2");
    assert_eq!(new_names.get(2).unwrap().as_str().unwrap(), "Row 3");

    // Renaming with a map
    let map = ValueMap::from_iter([
        ("Row 1".into(), "First Row".into()),
        ("Nonexistent".into(), "Should Not Exist".into()),
    ]);
    let table = rename(&table, &[Value::from(()), Value::from_object(map)]).unwrap();
    let new_names = table
        .downcast_object::<AgateTable>()
        .unwrap()
        .row_names()
        .unwrap();
    assert_eq!(new_names.len(), 3);
    assert_eq!(new_names.get(0).unwrap().as_str().unwrap(), "First Row");
    assert_eq!(new_names.get(1).unwrap().as_str().unwrap(), "Row 2");
    assert_eq!(new_names.get(2).unwrap().as_str().unwrap(), "Row 3");
}

#[test]
fn test_column_renaming_with_map_filter() {
    // Test case for the dbt_profiler issue: using map filter to rename columns
    let batch = simple_record_batch();
    let table_val = AgateTable::from_record_batch(batch).into_value();

    let env = Environment::new();
    let state = env.empty_state();

    // Simulate what happens in dbt_profiler:
    // {% set information_schema_columns = information_schema_columns.rename(information_schema_columns.column_names | map('lower') | list) %}
    // Get the table and its column_names
    let column_names = table_val.get_attr("column_names").unwrap();

    // The map filter returns an iterable that should work with rename
    // We'll use call_method directly instead of template rendering
    let rename_args = {
        // Create the mapped column names list by iterating the tuple
        let mut mapped_names = Vec::new();
        let iter = column_names.try_iter().unwrap();
        for name in iter {
            if let Some(s) = name.as_str() {
                mapped_names.push(Value::from(s.to_lowercase()));
            }
        }
        Value::from_iter(mapped_names)
    };

    // Call rename with the mapped column names (this should not error)
    let result = table_val.call_method(&state, "rename", &[rename_args], &[]);

    // Verify it doesn't throw an error about "column_names must be a map or an array"
    assert!(
        result.is_ok(),
        "rename should accept iterable from map filter"
    );

    // Verify the columns were renamed
    let renamed_table = result.unwrap().downcast_object::<AgateTable>().unwrap();
    assert_eq!(renamed_table.column_name(0).unwrap(), "id");
    assert_eq!(renamed_table.column_name(1).unwrap(), "country");
}

#[test]
fn test_rename_rejects_each_slug_flag_independently() {
    let table = AgateTable::from_record_batch(simple_record_batch());
    let err = table.rename(None, None, true, false).unwrap_err();
    assert!(err.to_string().contains("slugging columns or rows"));

    let err = table.rename(None, None, false, true).unwrap_err();
    assert!(err.to_string().contains("slugging columns or rows"));
}

fn color_table() -> AgateTable {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int32, true),
        Field::new("color", DataType::Utf8, true),
        Field::new("value", DataType::Int32, true),
    ]));
    let id_array: ArrayRef = Arc::new(Int32Array::new(vec![1, 2, 3, 4, 5, 6].into(), None));
    let color_array: ArrayRef = Arc::new(StringArray::from(vec![
        Some("red"),
        Some("blue"),
        Some("red"),
        Some("green"),
        Some("blue"),
        Some("red"),
    ]));
    let value_array: ArrayRef =
        Arc::new(Int32Array::new(vec![10, 20, 30, 40, 50, 60].into(), None));
    let batch = RecordBatch::try_new(schema, vec![id_array, color_array, value_array]).unwrap();
    AgateTable::from_record_batch(Arc::new(batch))
}

fn nullable_and_wide_table() -> AgateTable {
    let schema = Arc::new(Schema::new(vec![
        Field::new("n", DataType::Utf8, true),
        Field::new("word", DataType::Utf8, true),
    ]));
    let nullable_array: ArrayRef = Arc::new(StringArray::from(vec![None, Some("plain")]));
    let word_array: ArrayRef = Arc::new(StringArray::from(vec![Some("violet"), Some("green")]));
    let batch = RecordBatch::try_new(schema, vec![nullable_array, word_array]).unwrap();
    AgateTable::from_record_batch(Arc::new(batch))
}

#[test]
fn test_grouper() {
    let table = color_table();
    let grouper = table.grouper(&["color".to_string()]).unwrap();
    let mut groups = grouper.iter();
    assert_eq!(groups.next().unwrap(), 0); // red
    assert_eq!(groups.next().unwrap(), 1); // blue
    assert_eq!(groups.next().unwrap(), 0); // red
    assert_eq!(groups.next().unwrap(), 2); // green
    assert_eq!(groups.next().unwrap(), 1); // blue
    assert_eq!(groups.next().unwrap(), 0); // red
    assert_eq!(groups.next(), None);
}

#[test]
fn test_print_table_truncates_rows_columns_and_width() {
    let table = color_table();
    let printed = table.print_table_to_string(2, 2, 5).unwrap();
    assert_eq!(
        printed,
        "\
| id | color | ... |
| -- | ----- | --- |
|  1 | red   | ... |
|  2 | blue  | ... |
| ... | ...   | ... |
"
    );

    assert_eq!(
        table
            .display()
            .with_max_rows(2)
            .with_max_columns(2)
            .with_max_column_width(5)
            .to_string(),
        printed
    );

    let mut output = Vec::new();
    table.print_table(1, 1, 4, Some(&mut output)).unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("| id | ... |"));
    assert!(output.contains("|  1 | ... |"));
}

#[test]
fn test_print_table_keeps_boundary_limits_and_formats_nulls() {
    let table = color_table();
    let printed = table.print_table_to_string(6, 3, 20).unwrap();
    assert_eq!(
        printed,
        "\
| id | color | value |
| -- | ----- | ----- |
|  1 | red   |    10 |
|  2 | blue  |    20 |
|  3 | red   |    30 |
|  4 | green |    40 |
|  5 | blue  |    50 |
|  6 | red   |    60 |
"
    );
    assert!(!printed.contains("..."));

    let printed = nullable_and_wide_table()
        .print_table_to_string(2, 2, 5)
        .unwrap();
    assert_eq!(
        printed,
        "\
|     n |  word |
| ----- | ----- |
|       | vi... |
| plain | green |
"
    );
    assert!(!printed.contains("none"));
}

#[test]
fn test_group_by() {
    let env = Environment::new();
    let state = env.empty_state();
    let table = Value::from_object(color_table());

    let table_set = table
        .call_method(&state, "group_by", &[Value::from("color")], &[])
        .unwrap();
    let keys = table_set.call_method(&state, "keys", &[], &[]).unwrap();
    assert_eq!(keys.get_item_by_index(0).unwrap(), Value::from("red"));
    assert_eq!(keys.get_item_by_index(1).unwrap(), Value::from("blue"));
    assert_eq!(keys.get_item_by_index(2).unwrap(), Value::from("green"));

    let mut groups = table_set.try_iter().unwrap();
    // each group Value is an AgateTable
    let (red, blue, green) = (
        groups.next().unwrap(),
        groups.next().unwrap(),
        groups.next().unwrap(),
    );
    let mut iter = red.try_iter().unwrap();
    assert_eq!(
        "<agate.Row: (1, red, 10)>",
        iter.next().unwrap().to_string()
    );
    assert_eq!(
        "<agate.Row: (3, red, 30)>",
        iter.next().unwrap().to_string()
    );
    assert_eq!(
        "<agate.Row: (6, red, 60)>",
        iter.next().unwrap().to_string()
    );
    assert!(iter.next().is_none());
    let mut iter = blue.try_iter().unwrap();
    assert_eq!(
        "<agate.Row: (2, blue, 20)>",
        iter.next().unwrap().to_string()
    );
    assert_eq!(
        "<agate.Row: (5, blue, 50)>",
        iter.next().unwrap().to_string()
    );
    assert!(iter.next().is_none());
    let mut iter = green.try_iter().unwrap();
    assert_eq!(
        "<agate.Row: (4, green, 40)>",
        iter.next().unwrap().to_string()
    );
    assert!(iter.next().is_none());
}

#[test]
fn test_distinct() {
    let env = Environment::new();
    let state = env.empty_state();
    let table = Value::from_object(color_table());

    let result = table
        .call_method(&state, "distinct", &[Value::from("color")], &[])
        .unwrap();

    let distinct_table = result.downcast_object::<AgateTable>().unwrap();

    // Verify all columns are returned
    assert_eq!(distinct_table.column_name(0).unwrap(), "id");
    assert_eq!(distinct_table.column_name(1).unwrap(), "color");
    assert_eq!(distinct_table.column_name(2).unwrap(), "value");

    // Verify that there are only three rows: red, blue, and green
    let cols = distinct_table.columns().values();
    let color = cols.get(1).unwrap();
    assert_eq!(color.len(), Some(3));
    assert_eq!(color.get_item_by_index(0).unwrap().as_str().unwrap(), "red");
    assert_eq!(
        color.get_item_by_index(1).unwrap().as_str().unwrap(),
        "blue"
    );
    assert_eq!(
        color.get_item_by_index(2).unwrap().as_str().unwrap(),
        "green"
    );

    let result = table
        .call_method(
            &state,
            "distinct",
            &[Value::from_iter(vec!["id", "color", "value"])],
            &[],
        )
        .unwrap();

    let distinct_table = result.downcast_object::<AgateTable>().unwrap();

    assert_eq!(distinct_table.num_rows(), 6);

    let result = table
        .call_method(&state, "distinct", &[Value::NONE], &[])
        .unwrap();

    let distinct_table = result.downcast_object::<AgateTable>().unwrap();

    assert_eq!(distinct_table.num_rows(), 6);
}

#[test]
fn column_indices_of_accepts_names_indices_and_sequences() {
    // columns: 0 = "id", 1 = "country"
    let table = AgateTable::from_record_batch(simple_record_batch());
    let indices = |keys: Value| table.column_indices_of(&keys);

    // single column name and single column index
    assert_eq!(indices(Value::from("country")).unwrap(), vec![1]);
    assert_eq!(indices(Value::from(1)).unwrap(), vec![1]);
    // negative indices count from the end
    assert_eq!(indices(Value::from(-1)).unwrap(), vec![1]);

    // sequences keep the order of the keys, not the order of the columns
    assert_eq!(
        indices(Value::from_iter(["country", "id"])).unwrap(),
        vec![1, 0]
    );
    assert_eq!(indices(Value::from_iter([1, 0])).unwrap(), vec![1, 0]);
    // names and indices can be mixed
    assert_eq!(
        indices(Value::from_iter([Value::from("country"), Value::from(0)])).unwrap(),
        vec![1, 0]
    );

    // unknown names and out-of-range indices are skipped
    assert_eq!(
        indices(Value::from_iter(["country", "nope"])).unwrap(),
        vec![1]
    );
    assert_eq!(indices(Value::from_iter([0, 7])).unwrap(), vec![0]);
    assert_eq!(indices(Value::from("nope")).unwrap(), Vec::<usize>::new());

    // anything that is not a column name or index is an error
    let err = indices(Value::from(1.5)).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidArgument);
}

/// The `Table.join` method as a macro would call it: through `call_method`, which
/// parses the arguments and turns `inner`/`full_outer` into a [JoinType].
fn call_join(
    table: &Value,
    args: &[Value],
    kwargs: &[(&str, Value)],
) -> Result<Value, minijinja::Error> {
    let env = Environment::new();
    let state = env.empty_state();
    let args = if kwargs.is_empty() {
        args.to_vec()
    } else {
        let mut args = args.to_vec();
        args.push(Value::from(Kwargs::from_iter(kwargs.to_vec())));
        args
    };
    table.call_method(&state, "join", args.as_slice(), &[])
}

fn joined_rows(result: Result<Value, minijinja::Error>) -> Vec<String> {
    let table = result.unwrap().downcast_object::<AgateTable>().unwrap();
    stringly_rows_of(&table, "|")
}

#[test]
fn call_join_accepts_keyword_arguments() {
    let left = main_table(&[1, 2], &["a", "b"], &[Some(10), Some(20)]).into_value();
    let right = related_table(&[Some(10)], &["The Sign of the Four"]).into_value();

    let joined = call_join(
        &left,
        &[right],
        &[
            ("left_key", Value::from("fk")),
            ("right_key", Value::from("id")),
        ],
    );
    assert_eq!(
        joined_rows(joined),
        vec!["1|a|10|The Sign of the Four", "2|b|20|None"]
    );
}

#[test]
fn call_join_accepts_positional_arguments() {
    let left = main_table(&[1, 2], &["a", "b"], &[Some(10), Some(20)]).into_value();
    let right = related_table(&[Some(10)], &["The Sign of the Four"]).into_value();

    // right_table, left_key, right_key, inner
    let joined = call_join(
        &left,
        &[
            right,
            Value::from("fk"),
            Value::from("id"),
            Value::from(true),
        ],
        &[],
    );
    assert_eq!(joined_rows(joined), vec!["1|a|10|The Sign of the Four"]);
}

#[test]
fn call_join_rejects_a_join_that_is_both_inner_and_full_outer() {
    let left = main_table(&[1], &["a"], &[Some(10)]).into_value();
    let right = related_table(&[Some(10)], &["The Sign of the Four"]).into_value();

    let err = call_join(
        &left,
        &[right],
        &[
            ("left_key", Value::from("fk")),
            ("right_key", Value::from("id")),
            ("inner", Value::from(true)),
            ("full_outer", Value::from(true)),
        ],
    )
    .unwrap_err();

    assert_eq!(err.kind(), ErrorKind::InvalidArgument);
    assert!(
        err.to_string()
            .contains(r#"A join can not be both "inner" and "full_outer"."#),
        "{err}"
    );
}

#[test]
fn call_join_rejects_a_right_table_that_is_not_a_table() {
    let left = main_table(&[1], &["a"], &[Some(10)]).into_value();

    let err = call_join(&left, &[Value::from("not a table")], &[]).unwrap_err();

    assert_eq!(err.kind(), ErrorKind::InvalidArgument);
    assert!(
        err.to_string()
            .contains("Table.join: right_table must be a Table"),
        "{err}"
    );
}

const COLUMN_VALUES_METHODS: [&str; 4] = [
    "values_distinct",
    "values_without_nulls",
    "values_sorted",
    "values_without_nulls_sorted",
];

fn single_column_table_value(array: ArrayRef) -> Value {
    let batch = RecordBatch::try_from_iter([("c", array)]).unwrap();
    Value::from_object(AgateTable::from_record_batch(Arc::new(batch)))
}

fn render_column_method(table: &Value, method: &str) -> Result<String, minijinja::Error> {
    Environment::new().render_str(
        &format!("{{{{ t.columns[0].{method}() | list }}}}"),
        context! { t => table.clone() },
        &[],
    )
}

fn assert_column_values_methods(name: &str, array: ArrayRef, expected: [&str; 4]) {
    let table = single_column_table_value(array);
    for (method, expected) in COLUMN_VALUES_METHODS.iter().zip(expected) {
        let actual = render_column_method(&table, method).unwrap();
        let expected = Environment::new()
            .render_str(&format!("{{{{ {expected} }}}}"), context! {}, &[])
            .unwrap();
        assert_eq!(actual, expected, "{name}: {method}");
    }
}

fn utf8_with_nulls() -> Vec<Option<&'static str>> {
    vec![Some("b"), None, Some("a"), Some("c"), Some("a"), None]
}

#[test]
fn test_column_values_methods() {
    assert_column_values_methods(
        "int64",
        Arc::new(arrow::array::Int64Array::from(vec![
            Some(3),
            None,
            Some(1),
            Some(2),
            Some(1),
            None,
        ])),
        [
            "[3, none, 1, 2]",
            "[3, 1, 2, 1]",
            "[1, 1, 2, 3, none, none]",
            "[1, 1, 2, 3]",
        ],
    );
    assert_column_values_methods(
        "utf8",
        Arc::new(StringArray::from(utf8_with_nulls())),
        [
            "['b', none, 'a', 'c']",
            "['b', 'a', 'c', 'a']",
            "['a', 'a', 'b', 'c', none, none]",
            "['a', 'a', 'b', 'c']",
        ],
    );
    assert_column_values_methods(
        "int64 without nulls",
        Arc::new(arrow::array::Int64Array::from(vec![2, 1, 2])),
        ["[2, 1]", "[2, 1, 2]", "[1, 2, 2]", "[1, 2, 2]"],
    );
    assert_column_values_methods(
        "empty int64",
        Arc::new(arrow::array::Int64Array::from(Vec::<i64>::new())),
        ["[]", "[]", "[]", "[]"],
    );
}

#[test]
fn test_column_values_methods_logical_nulls() {
    assert_column_values_methods(
        "all-null int64",
        Arc::new(arrow::array::Int64Array::from(Vec::<Option<i64>>::from([
            None, None, None,
        ]))),
        ["[none]", "[]", "[none, none, none]", "[]"],
    );
    assert_column_values_methods(
        "null array",
        Arc::new(arrow::array::NullArray::new(3)),
        ["[none]", "[]", "[none, none, none]", "[]"],
    );
    let keys_to_null_value = DictionaryArray::<Int32Type>::try_new(
        Int32Array::from(vec![0, 1, 2, 0, 1]),
        Arc::new(StringArray::from(vec![Some("b"), None, Some("a")])),
    )
    .unwrap();
    assert_column_values_methods(
        "dictionary with null values",
        Arc::new(keys_to_null_value),
        [
            "['b', none, 'a']",
            "['b', 'a', 'b']",
            "['a', 'b', 'b', none, none]",
            "['a', 'b', 'b']",
        ],
    );
    let dictionary: DictionaryArray<Int32Type> = utf8_with_nulls().into_iter().collect();
    assert_column_values_methods(
        "dictionary-encoded utf8",
        Arc::new(dictionary),
        [
            "['b', none, 'a', 'c']",
            "['b', 'a', 'c', 'a']",
            "['a', 'a', 'b', 'c', none, none]",
            "['a', 'a', 'b', 'c']",
        ],
    );
}

#[test]
fn test_column_values_methods_unsupported_type() {
    let mut builder = arrow::array::MapBuilder::new(
        None,
        StringBuilder::new(),
        arrow::array::Int64Builder::new(),
    );
    for value in [1, 2] {
        builder.keys().append_value("k");
        builder.values().append_value(value);
        builder.append(true).unwrap();
    }
    let table = single_column_table_value(Arc::new(builder.finish()));

    for method in [
        "values_distinct",
        "values_sorted",
        "values_without_nulls_sorted",
    ] {
        let err = render_column_method(&table, method).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidOperation, "{err}");
        assert!(
            err.to_string().contains(&format!("Column.{method}:")),
            "{err}"
        );
    }
    let out = Environment::new()
        .render_str(
            "{{ t.columns[0].values_without_nulls() | list | length }}",
            context! { t => table },
            &[],
        )
        .unwrap();
    assert_eq!(out, "2");
}
