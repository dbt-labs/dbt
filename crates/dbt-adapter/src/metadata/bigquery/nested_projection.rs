use dbt_adapter_core::AdapterType;
use dbt_adapter_sql::types::SqlType;

/// Generates a safe SQL projection expression for BigQuery struct columns.
///
/// This recursively builds an explicitly aliased projection for nested structures
/// so that BigQuery aligns the data by name rather than position. If no structural
/// rewrite is needed, it returns the original column name.
pub fn render_struct_projection(col_name: &str, data_type: &str) -> String {
    let Ok((sql_type, _)) = SqlType::parse(AdapterType::Bigquery, data_type.trim()) else {
        return col_name.to_string();
    };

    match build_projection(col_name, &sql_type, 0) {
        Some(expr) => format!("{expr} AS {col_name}"),
        None => col_name.to_string(),
    }
}

/// Recursively traverses the SQL type tree to build the inner projection string.
///
/// The `depth` parameter tracks array nesting to generate unique aliases (`elem_0`, `elem_1`),
/// preventing BigQuery scope shadowing bugs in correlated subqueries.
fn build_projection(path: &str, sql_type: &SqlType, depth: usize) -> Option<String> {
    match sql_type {
        SqlType::Struct(Some(fields)) => {
            if fields.is_empty() {
                return None;
            }

            let struct_parts: Vec<String> = fields
                .iter()
                .map(|field| {
                    let name = field.name.display(AdapterType::Bigquery);
                    let field_path = format!("{path}.{name}");
                    let sub = build_projection(&field_path, &field.sql_type, depth)
                        .unwrap_or_else(|| field_path.clone());
                    format!("{sub} AS {name}")
                })
                .collect();

            Some(format!(
                "IF({path} IS NULL, NULL, STRUCT({}))",
                struct_parts.join(", ")
            ))
        }
        SqlType::Array(Some(inner)) if matches!(inner.as_ref(), SqlType::Struct(Some(_))) => {
            let elem_alias = format!("elem_{depth}");
            let elem = build_projection(&elem_alias, inner, depth + 1)
                .unwrap_or_else(|| elem_alias.clone());
            Some(format!(
                "IF({path} IS NULL, NULL, ARRAY(SELECT {elem} FROM UNNEST({path}) AS {elem_alias}))"
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_struct_projection_flat_struct() {
        assert_eq!(
            render_struct_projection("col", "struct<y INT64, x INT64>"),
            "IF(col IS NULL, NULL, STRUCT(col.y AS y, col.x AS x)) AS col"
        );
    }

    #[test]
    fn test_render_struct_projection_nested_struct() {
        assert_eq!(
            render_struct_projection("col", "struct<a struct<b INT64, c STRING>, d INT64>"),
            "IF(col IS NULL, NULL, STRUCT(IF(col.a IS NULL, NULL, STRUCT(col.a.b AS b, col.a.c AS c)) AS a, col.d AS d)) AS col"
        );
    }

    #[test]
    fn test_render_struct_projection_preserves_parsed_field_order() {
        assert_eq!(
            render_struct_projection("struct_data", "struct<y integer, x integer, z integer>"),
            "IF(struct_data IS NULL, NULL, STRUCT(struct_data.y AS y, struct_data.x AS x, struct_data.z AS z)) AS struct_data"
        );
    }

    #[test]
    fn test_render_struct_projection_array_of_struct() {
        assert_eq!(
            render_struct_projection("arr", "ARRAY<STRUCT<y INT64, x INT64>>"),
            "IF(arr IS NULL, NULL, ARRAY(SELECT IF(elem_0 IS NULL, NULL, STRUCT(elem_0.y AS y, elem_0.x AS x)) FROM UNNEST(arr) AS elem_0)) AS arr"
        );
    }

    #[test]
    fn test_render_struct_projection_array_of_nested_struct() {
        assert_eq!(
            render_struct_projection("arr", "ARRAY<STRUCT<a STRUCT<b INT64, c STRING>, d INT64>>"),
            "IF(arr IS NULL, NULL, ARRAY(SELECT IF(elem_0 IS NULL, NULL, STRUCT(IF(elem_0.a IS NULL, NULL, STRUCT(elem_0.a.b AS b, elem_0.a.c AS c)) AS a, elem_0.d AS d)) FROM UNNEST(arr) AS elem_0)) AS arr"
        );
    }

    #[test]
    fn test_render_struct_projection_array_of_struct_with_array_field() {
        assert_eq!(
            render_struct_projection("col", "ARRAY<STRUCT<x ARRAY<INT64>>>"),
            "IF(col IS NULL, NULL, ARRAY(SELECT IF(elem_0 IS NULL, NULL, STRUCT(elem_0.x AS x)) FROM UNNEST(col) AS elem_0)) AS col"
        );
    }

    #[test]
    fn test_render_struct_projection_struct_with_array_of_struct_field() {
        assert_eq!(
            render_struct_projection(
                "col",
                "struct<x INT64, my_array ARRAY<STRUCT<y INT64, z INT64>>>",
            ),
            "IF(col IS NULL, NULL, STRUCT(col.x AS x, IF(col.my_array IS NULL, NULL, ARRAY(SELECT IF(elem_0 IS NULL, NULL, STRUCT(elem_0.y AS y, elem_0.z AS z)) FROM UNNEST(col.my_array) AS elem_0)) AS my_array)) AS col"
        );
    }

    #[test]
    fn test_nested_array_of_struct_elem_aliases_increment() {
        assert_eq!(
            render_struct_projection(
                "col",
                "struct<x INT64, outer ARRAY<STRUCT<inner ARRAY<STRUCT<y INT64, z INT64>>>>>",
            ),
            "IF(col IS NULL, NULL, STRUCT(col.x AS x, IF(col.outer IS NULL, NULL, ARRAY(SELECT IF(elem_0 IS NULL, NULL, STRUCT(IF(elem_0.inner IS NULL, NULL, ARRAY(SELECT IF(elem_1 IS NULL, NULL, STRUCT(elem_1.y AS y, elem_1.z AS z)) FROM UNNEST(elem_0.inner) AS elem_1)) AS inner)) FROM UNNEST(col.outer) AS elem_0)) AS outer)) AS col"
        );
    }

    #[test]
    fn test_render_struct_projection_struct_with_array_of_array_of_struct_field_passthrough() {
        assert_eq!(
            render_struct_projection(
                "col",
                "struct<x INT64, nested ARRAY<ARRAY<STRUCT<y INT64>>>>",
            ),
            "IF(col IS NULL, NULL, STRUCT(col.x AS x, col.nested AS nested)) AS col"
        );
    }

    #[test]
    fn test_render_struct_projection_struct_with_array_of_int_field_passthrough() {
        assert_eq!(
            render_struct_projection("col", "struct<x INT64, nums ARRAY<INT64>>"),
            "IF(col IS NULL, NULL, STRUCT(col.x AS x, col.nums AS nums)) AS col"
        );
    }

    #[test]
    fn test_render_struct_projection_no_projection_returns_column_name() {
        for data_type in [
            "INT64",
            "ARRAY<INT64>",
            "struct<x>",
            "ARRAY<ARRAY<STRUCT<x INT64>>>",
            "struct<>",
        ] {
            assert_eq!(
                render_struct_projection("col", data_type),
                "col",
                "data_type={data_type:?}"
            );
        }
    }
}
