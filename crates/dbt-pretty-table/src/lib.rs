use std::sync::Arc;

use arrow::array::Array;
use arrow::datatypes::DataType;
use arrow::{
    array::{ArrayRef, StringArray},
    record_batch::RecordBatch,
    util::display::array_value_to_string,
};
use arrow_schema::{Field, Schema};
use comfy_table::*;
use comfy_table::{Table, presets::UTF8_FULL_CONDENSED};

pub fn make_table_name<T: AsRef<str>>(catalog: T, schema: T, table: T) -> String {
    [catalog.as_ref(), schema.as_ref(), table.as_ref()].join(".")
}

pub fn make_column_names(schema: &Schema) -> Vec<String> {
    schema
        .fields()
        .iter()
        .map(|f| f.name().to_owned())
        .collect()
}

const BORDER_PADDING_SIZE: usize = 3;
const BORDER_SIZE: usize = 1;
pub const ELLIPSIS: &str = "..";

// originally defined in print_data_format.rs
macro_rules! batches_to_json {
    ($TYPE:path, $batches:expr) => {{
        let mut bytes = vec![];
        {
            let mut writer = arrow::json::WriterBuilder::new()
                .with_explicit_nulls(true)
                .build::<_, $TYPE>(&mut bytes);
            for b in $batches.as_slice() {
                writer.write(b).map_err(Error::Arrow)?;
            }
            writer.finish().map_err(Error::Arrow)?;
        }
        String::from_utf8(bytes).map_err(|_| Error::FromUtf8)?
    }};
}

pub fn batches_to_json_rows(batches: &[RecordBatch]) -> Vec<serde_json::Value> {
    (|| -> Result<Vec<serde_json::Value>, Error> {
        let json_str = batches_to_json!(arrow::json::writer::JsonArray, batches.to_vec());
        serde_json::from_str(&json_str).map_err(|_| Error::FromUtf8)
    })()
    .unwrap_or_default()
}

#[derive(Debug, Clone, Copy)]
pub enum DisplayFormat {
    Table,
    Csv,
    Tsv,
    Json,
    NdJson,
    Yml,
    /// Output nodes as selector strings (e.g. "source:pkg.source_name.table_name")
    Selector,
    /// Output nodes as search names (node.search_name)
    Name,
    /// Output nodes as file paths (node.original_file_path)
    Path,
}

pub enum Error {
    FromUtf8,
    Arrow(arrow::error::ArrowError),
    UnsupportedFormat(DisplayFormat),
}

pub fn print_csv_batches_with_sep(batches: &[RecordBatch], delimiter: u8) -> Result<String, Error> {
    let mut bytes = vec![];
    if !batches.is_empty() {
        {
            let builder = arrow::csv::WriterBuilder::new()
                .with_header(true)
                .with_delimiter(delimiter);
            let mut writer = builder.build(&mut bytes);
            for batch in batches {
                writer.write(batch).map_err(Error::Arrow)?;
            }
        }
        let formatted = String::from_utf8(bytes).map_err(|_| Error::FromUtf8)?;
        Ok(formatted)
    } else {
        Ok("".to_owned())
    }
}

#[allow(clippy::too_many_arguments)]
pub fn pretty_data_table(
    title: &str,
    subtitle: &str,
    column_names: &[String],
    record_batches: &[RecordBatch],
    display_format: DisplayFormat,
    limit: Option<usize>,
    show_footer: bool,
    actual_rows: Option<usize>,
) -> Result<String, Error> {
    let mut out = String::new();

    // If the actual row count is provided, use it. Otherwise, compute the row
    // count by summing the number of rows in each record batch
    let row_count: usize =
        actual_rows.unwrap_or_else(|| record_batches.iter().map(|batch| batch.num_rows()).sum());

    // Handle different display formats here...
    match display_format {
        DisplayFormat::Csv => {
            let converted_batches = cast_decimal_128_to_string(record_batches);
            let converted_batches = stringify_types_for_show(converted_batches.as_slice());
            out.push_str(&print_csv_batches_with_sep(
                converted_batches.as_slice(),
                b',',
            )?);
        }
        DisplayFormat::Tsv => {
            let converted_batches = cast_decimal_128_to_string(record_batches);
            let converted_batches = stringify_types_for_show(converted_batches.as_slice());
            out.push_str(&print_csv_batches_with_sep(
                converted_batches.as_slice(),
                b'\t',
            )?);
        }
        DisplayFormat::Json => {
            out.push_str(&batches_to_json!(
                arrow::json::writer::JsonArray,
                record_batches.to_vec()
            ));
        }
        DisplayFormat::Yml => {
            let converted_batches = cast_decimal_128_to_string(record_batches);
            let converted_batches = stringify_types_for_show(converted_batches.as_slice()); // TODO should we convert variant to json here?
            let ndjson_str =
                batches_to_json!(arrow::json::writer::LineDelimited, converted_batches);
            // todo: inline the whole effect of the serde_yml
            // let line_count = ndjson_str.lines().count();
            for line in ndjson_str.lines() {
                // Parse JSON object from each line of NDJSON
                let json_obj: serde_json::Value =
                    serde_json::from_str(line).expect("Failed to parse JSON");
                // Convert JSON object to YAML
                let yaml_obj: dbt_yaml::Value =
                    dbt_yaml::to_value(&json_obj).expect("Failed to convert to YAML");
                // Print YAML formatted object
                out.push_str("---\n");
                out.push_str(&dbt_yaml::to_string(&yaml_obj).expect("Failed to serialize to YAML"));
                // Print YAML formatted object
            }
        }

        DisplayFormat::NdJson => {
            let converted_batches = cast_decimal_128_to_string(record_batches);
            let converted_batches = stringify_types_for_show(converted_batches.as_slice()); // TODO should we convert variant to json here?
            out.push_str(&batches_to_json!(
                arrow::json::writer::LineDelimited,
                converted_batches
            ));
        }
        DisplayFormat::Table => {
            let converted_batches = stringify_types_for_show(record_batches);

            // print title and subtitle
            if !title.is_empty() {
                out.push_str(&format!("{title}\n"));
            }
            if !subtitle.is_empty() {
                out.push_str(&format!("{subtitle}\n"));
            }

            // define table and table width
            let (mut table, total_width, indices_to_include, include_ellipsis) =
                create_table(column_names);

            // format cells
            let mut num_rows = 0;
            for batch in converted_batches {
                for row in 0..batch.num_rows() {
                    let mut cells = Vec::new();
                    for index in indices_to_include.iter() {
                        let column = batch.column(*index);
                        let value = array_value_to_string(column, row).map_err(Error::Arrow)?;
                        // if the column's data_type is decimal, remove the trailing zeros after the decimal point
                        let content = if value.contains('.')
                            && matches!(
                                column.data_type(),
                                DataType::Decimal128(_, _) | DataType::Decimal256(_, _)
                            ) {
                            value.trim_end_matches('0').to_string()
                        } else {
                            value
                        };
                        cells.push(Cell::new(&content));
                    }
                    if include_ellipsis {
                        cells.push(Cell::new(ELLIPSIS));
                    }
                    if limit.is_some() && num_rows >= limit.expect("is some") {
                        break;
                    }
                    table.add_row(cells);
                    num_rows += 1;
                }
            }

            // print the table
            out.push_str(table.to_string().as_str());
            out.push('\n');

            // print footer
            if limit.is_some_and(|l| row_count >= l) {
                if include_ellipsis {
                    out.push_str(&format!(
                        "{} rows, {} columns, showing only {} rows and only {} columns. Run with --limit -1 to show all rows. Run with a terminal width of at least {} to show all columns.",
                        row_count,
                        column_names.len(),
                        limit.expect("is some"),
                        indices_to_include.len(),
                        total_width
                    ));
                } else {
                    out.push_str(&format!(
                        "{} rows, showing only {} rows. Run with --limit -1 to show all rows.",
                        row_count,
                        limit.expect("is some")
                    ));
                }
            } else if include_ellipsis {
                out.push_str(&format!(
                    "{} rows, {} columns, showing only {} columns. Run with a terminal width of at least {} to show all columns.",
                    row_count,
                    column_names.len(),
                    indices_to_include.len(),
                    total_width
                ));
            } else if show_footer {
                out.push_str(&format!("{row_count} rows."));
            }
        }
        // The new DisplayFormat variants are handled differently - they output node information
        // rather than tabular data These should be handled at a higher level in the list command
        // logic, not in this pretty_table function
        DisplayFormat::Selector | DisplayFormat::Name | DisplayFormat::Path => {
            // These formats are not applicable for tabular data display
            // They should be handled by the list command's show_dbt_nodes method
            // For now, we'll treat them as unsupported in this context
            return Err(Error::UnsupportedFormat(display_format));
        }
    };
    Ok(out)
}

fn cast_decimal_128_to_string(new_batches_slice: &[RecordBatch]) -> Vec<RecordBatch> {
    let mut new_batches: Vec<RecordBatch> = Vec::new();

    for batch in new_batches_slice {
        let mut new_columns: Vec<ArrayRef> = Vec::new();
        let mut new_schema_fields = Vec::new();

        for (i, column) in batch.columns().iter().enumerate() {
            match batch.schema().field(i).data_type() {
                DataType::Decimal128(_, _) => {
                    let decimal_column = column
                        .as_any()
                        .downcast_ref::<arrow::array::Decimal128Array>()
                        .unwrap();

                    let string_vec: Vec<Option<String>> = decimal_column
                        .iter()
                        .map(|maybe_decimal| {
                            maybe_decimal
                                .map(|decimal| Some(decimal.to_string()))
                                .unwrap_or(None)
                        })
                        .collect();

                    let a =
                        StringArray::from_iter(string_vec.iter().map(|option| option.as_deref()));
                    let string_array: ArrayRef = Arc::new(a);
                    new_columns.push(string_array);
                    new_schema_fields.push(Field::new(
                        batch.schema().field(i).name(),
                        DataType::Utf8,
                        true,
                    ));
                }
                _ => {
                    new_columns.push(column.clone());
                    new_schema_fields.push(batch.schema().field(i).clone());
                }
            }
        }

        let new_batch =
            RecordBatch::try_new(Arc::new(Schema::new(new_schema_fields)), new_columns).unwrap();
        new_batches.push(new_batch);
    }
    new_batches
}

fn stringify_types_for_show(new_batches_slice: &[RecordBatch]) -> Vec<RecordBatch> {
    let mut new_batches: Vec<RecordBatch> = Vec::new();

    for batch in new_batches_slice {
        let mut new_columns: Vec<ArrayRef> = Vec::new();
        let mut new_schema_fields = Vec::new();

        for (field, array) in batch.schema().fields().iter().zip(batch.columns()) {
            // todo: add this back, but then this has deps on snowflake, which seems strange..
            // let unvarianted = convert_array_recursive(array, |array| {
            //     let mut array = Arc::clone(array);
            //     if array.data_type() == &SnowflakeTyping::object() {
            //         // Convert to variant. This requires changing the field name inside FSL
            //         array = arrow::compute::cast_with_options(&array, &SnowflakeTyping::variant())?;
            //     }
            //     if array.data_type() == &SnowflakeTyping::variant() {
            //         return Ok(Some(
            //             CastFromVariant::new(DataType::Utf8)
            //                 .cast(as_fixed_size_list_array(&array)?)?,
            //         ));
            //     }
            //     Ok(None)
            // });
            let unvarianted = None;
            // we're not supposed to throw here 🤷‍♂️
            #[allow(clippy::unnecessary_literal_unwrap)]
            #[expect(clippy::or_fun_call)]
            // it's easy to address by inlining the `unvarianted`, but would make above TODO less clear
            let maybe_converted = unvarianted.unwrap_or(Arc::clone(array));
            if Arc::ptr_eq(array, &maybe_converted) {
                new_schema_fields.push(field.clone());
                new_columns.push(Arc::clone(array));
            } else {
                new_schema_fields.push(Arc::new(Field::new(
                    field.name(),
                    maybe_converted.data_type().clone(),
                    maybe_converted.is_nullable(),
                )));
                new_columns.push(maybe_converted);
            }
        }

        let new_batch =
            RecordBatch::try_new(Arc::new(Schema::new(new_schema_fields)), new_columns).unwrap();
        new_batches.push(new_batch);
    }
    new_batches
}

fn create_table(column_names: &[String]) -> (Table, usize, Vec<usize>, bool) {
    let mut table = Table::new();
    table
        .set_content_arrangement(ContentArrangement::Dynamic)
        .load_preset(UTF8_FULL_CONDENSED);

    let column_widths: Vec<usize> = column_names.iter().map(|h| h.len()).collect();

    let total_width: usize = (column_widths.iter().sum::<usize>())
        + (column_names.len() * BORDER_PADDING_SIZE) // borders and padding
        + BORDER_SIZE;
    // for the last border
    let default_width = 1000;
    // Use a large value to fit all content if not running in a terminal
    // todo:  should term_size  be a property of the console?
    let max_table_width = term_size::dimensions().map_or(default_width, |(w, _)| w);
    table.set_width(max_table_width.try_into().unwrap());

    // determine which columns to include
    let mut indices_to_include = vec![];
    let mut include_ellipsis = false;
    let mut current_width = BORDER_SIZE;
    // Account for the first border
    if total_width > max_table_width {
        include_ellipsis = true;
        current_width += ELLIPSIS.len() + BORDER_PADDING_SIZE; // Account for the '..' column width
        for (i, column_width) in column_widths.iter().enumerate() {
            if current_width + column_width + BORDER_PADDING_SIZE > max_table_width {
                break;
            }
            indices_to_include.push(i);
            current_width += column_width + BORDER_PADDING_SIZE;
        }
    } else {
        indices_to_include.extend(0..column_names.len()); // Include all columns if space permits
    }

    // format headers
    let mut headers = Vec::new();
    for (i, column_name) in column_names.iter().enumerate() {
        if indices_to_include.contains(&i) {
            headers.push(Cell::new(column_name).add_attribute(Attribute::Bold));
        }
    }
    if include_ellipsis {
        headers.push(Cell::new(ELLIPSIS).add_attribute(Attribute::Bold));
    }
    table.set_header(headers);
    (table, total_width, indices_to_include, include_ellipsis)
}

#[allow(clippy::too_many_arguments)]
pub fn pretty_schema_table(
    title: &str,
    subtitle: &str,
    display_format: DisplayFormat,
    table_schema: &Schema,
    limit: Option<usize>,
    show_footer: bool,
) -> Result<String, Error> {
    // Define column names for the schema table
    let column_names = vec!["column_name".to_owned(), "data_type".to_owned()];

    // Create fields for the schema
    let fields = vec![
        Field::new("column_name", DataType::Utf8, false),
        Field::new("data_type", DataType::Utf8, false),
    ];

    // Extract column names and data types from the table schema
    let column_names_array: Vec<&str> = table_schema
        .fields()
        .iter()
        .map(|field| field.name().as_str())
        .collect();
    // todo: @wizardxz: let's make this dialect dependent...
    let data_types_array: Vec<String> = table_schema
        .fields()
        .iter()
        .map(|field| field.data_type().to_string())
        .collect();

    // Create Arrow arrays for column names and data types
    let column_name_array: ArrayRef = Arc::new(StringArray::from(column_names_array));
    let data_type_array: ArrayRef = Arc::new(StringArray::from(data_types_array));

    // Create a RecordBatch with the schema and arrays
    let schema = Arc::new(Schema::new(fields));
    let record_batch =
        RecordBatch::try_new(schema, vec![column_name_array, data_type_array]).unwrap();
    let record_batches = &[record_batch];

    // Call pretty_data_table to display the schema table
    pretty_data_table(
        title,
        subtitle,
        &column_names,
        record_batches,
        display_format,
        limit,
        show_footer,
        None,
    )
}

/// Creates a pretty table from rows of strings and column names, with an optional index column
///
/// # Arguments
/// * `title` - The title of the table
/// * `subtitle` - The subtitle of the table (can be empty)
/// * `column_names` - Vector of column names
/// * `rows` - Vector of vectors where each inner vector represents a row of data
/// * `display_format` - The format to display the data in (table, csv, etc.)
/// * `limit` - Maximum number of rows to display (0 for no limit)
/// * `show_footer` - Whether to show the row count footer
/// * `show_index` - Whether to show an index column (1-based)
///
/// # Returns
/// * `Result<String, _>` - The formatted table as a string
#[allow(clippy::too_many_arguments)]
pub fn pretty_vec_table(
    title: &str,
    subtitle: &str,
    column_names: &[String],
    rows: &[Vec<String>],
    display_format: DisplayFormat,
    limit: Option<usize>,
    show_footer: bool,
    show_index: bool,
) -> Result<String, Error> {
    // Create the full set of column names (including index if requested)
    let display_column_names = if show_index {
        let mut cols = vec!["#".to_string()];
        cols.extend(column_names.iter().cloned());
        cols
    } else {
        column_names.to_vec()
    };

    // Create Arrow arrays from the input data
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(display_column_names.len());
    let mut fields = Vec::with_capacity(display_column_names.len());

    // Add index column if requested
    if show_index {
        let index_data: Vec<String> = (1..=rows.len()).map(|i| i.to_string()).collect();
        columns.push(Arc::new(StringArray::from(index_data)));
        fields.push(Field::new("#", DataType::Utf8, false));
    }

    // For each data column, collect all values from that column across all rows
    for (col_idx, column_name) in column_names.iter().enumerate() {
        let column_data: Vec<&str> = rows
            .iter()
            .map(|row| row.get(col_idx).map_or_else(|| "", |s| s.as_str()))
            .collect();

        columns.push(Arc::new(StringArray::from(column_data)));
        fields.push(Field::new(column_name, DataType::Utf8, false));
    }

    // Create a RecordBatch with the schema and arrays
    let schema = Arc::new(Schema::new(fields));
    let record_batch = RecordBatch::try_new(schema, columns).map_err(Error::Arrow)?;
    let record_batches = &[record_batch];

    // Use the existing pretty_data_table function to format the output
    pretty_data_table(
        title,
        subtitle,
        &display_column_names,
        record_batches,
        display_format,
        limit,
        show_footer,
        Some(rows.len()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int32Array, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;

    fn make_batch(names: &[&str], ages: &[i32]) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("name", DataType::Utf8, false),
            Field::new("age", DataType::Int32, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(names.to_vec())),
                Arc::new(Int32Array::from(ages.to_vec())),
            ],
        )
        .unwrap()
    }

    #[test]
    fn test_batches_to_json_rows_empty() {
        let rows = batches_to_json_rows(&[]);
        assert!(rows.is_empty());
    }

    #[test]
    fn test_batches_to_json_rows_single_batch() {
        let batch = make_batch(&["alice", "bob"], &[30, 25]);
        let rows = batches_to_json_rows(&[batch]);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "alice");
        assert_eq!(rows[0]["age"], 30);
        assert_eq!(rows[1]["name"], "bob");
        assert_eq!(rows[1]["age"], 25);
    }

    #[test]
    fn test_batches_to_json_rows_multiple_batches() {
        let batch1 = make_batch(&["alice"], &[30]);
        let batch2 = make_batch(&["bob"], &[25]);
        let rows = batches_to_json_rows(&[batch1, batch2]);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "alice");
        assert_eq!(rows[1]["name"], "bob");
    }

    #[test]
    fn test_batches_to_json_rows_null_values() {
        let schema = Arc::new(Schema::new(vec![Field::new("name", DataType::Utf8, true)]));
        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(StringArray::from(vec![Some("alice"), None]))],
        )
        .unwrap();
        let rows = batches_to_json_rows(&[batch]);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "alice");
        assert!(rows[1]["name"].is_null());
    }
}
