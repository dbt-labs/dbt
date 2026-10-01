use arrow::datatypes::DataType;

/// Returns a readable, concise representation of the given data type from Fusion's implicit
/// universal type system. Does not conflate type options that could have different semantics
/// in the code (either intentionally, or due to a bug or missing feature). The returned string
/// is appropriate for diagnostics (e.g. display in error messages), but inappropriate for use
/// in SQL.
///
/// This function is similar to `DataType::to_string`, but recognizes Fusion's "distinct types"
/// and provides more concise representation for non-primitive types.
///
/// As dialects and binders operate on types potentially coming from other dialects, this
/// function is quite different in purpose than `Dialect::format_type`. That other function
/// needs to produce valid SQL type names in target dialect (and therefore is fallible),
/// and is allowed to conflate different types (for example Snowflake's `format_type`
/// may render various Int types as `NUMBER(_, 0)`).
pub fn fusion_type_repr(dt: &DataType) -> String {
    match dt {
        dt @ (DataType::Null
        | DataType::Boolean
        | DataType::Int8
        | DataType::Int16
        | DataType::Int32
        | DataType::Int64
        | DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::UInt64
        | DataType::Float16
        | DataType::Float32
        | DataType::Float64
        | DataType::Decimal32(_, _)
        | DataType::Decimal64(_, _)
        | DataType::Decimal128(_, _)
        | DataType::Decimal256(_, _)
        | DataType::Utf8
        | DataType::Utf8View
        | DataType::LargeUtf8
        | DataType::Binary
        | DataType::BinaryView
        | DataType::LargeBinary
        | DataType::FixedSizeBinary(_)
        | DataType::Date32
        | DataType::Date64
        | DataType::Time32(_)
        | DataType::Time64(_)
        | DataType::Timestamp(_, _)
        | DataType::Duration(_)
        | DataType::Interval(_)) => dt.to_string(),

        // "distinct types"
        DataType::FixedSizeList(field, 1)
            if !["item", "element"].contains(&field.name().as_str()) =>
        {
            // The apostrophes in result are so that this is distinguishable from "normal" types
            format!("'{}'", field.name())
        }

        DataType::List(item) => {
            format!("List({})", fusion_type_repr(item.data_type()))
        }

        DataType::ListView(item) => {
            format!("ListView({})", fusion_type_repr(item.data_type()))
        }

        DataType::LargeList(item) => {
            format!("LargeList({})", fusion_type_repr(item.data_type()))
        }

        DataType::LargeListView(item) => {
            format!("LargeListView({})", fusion_type_repr(item.data_type()))
        }

        DataType::Struct(fields) => {
            format!(
                "Struct({})",
                fields
                    .iter()
                    .map(|field| format!(
                        "{}: {}",
                        field.name(),
                        fusion_type_repr(field.data_type())
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }

        DataType::Dictionary(_, value_type) => {
            format!("Dict({})", fusion_type_repr(value_type))
        }

        DataType::RunEndEncoded(_, value_type) => {
            format!("REE({})", fusion_type_repr(value_type.data_type()))
        }

        dt @ DataType::Map(inner, _) => {
            let mut to_string = None;
            if let DataType::Struct(fields) = inner.data_type()
                && let [key, value] = &**fields
            {
                to_string = Some(format!(
                    "Map({}, {})",
                    fusion_type_repr(key.data_type()),
                    fusion_type_repr(value.data_type())
                ));
            }
            to_string.unwrap_or_else(|| dt.to_string())
        }

        // fallback
        DataType::FixedSizeList(_, _) | DataType::Union(_, _) => dt.to_string(),
    }
}
