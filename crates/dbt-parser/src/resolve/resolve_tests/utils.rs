use std::collections::BTreeMap;

use dbt_common::{ErrorCode, FsResult, err};
use dbt_schemas::schemas::{data_tests::DataTests, dbt_column::ColumnProperties};

use crate::resolve::resolve_tests::persist_generic_data_tests::{
    ColumnTestEntry, LegacyTestSyntaxHandling,
};

type ModelDataTestVec = Vec<DataTests>;

pub fn base_tests_inner(
    tests: Option<&[DataTests]>,
    data_tests: Option<&[DataTests]>,
) -> FsResult<Option<ModelDataTestVec>> {
    // dbt-core renames `tests` -> `data_tests` behind `if data.get("tests"):`
    // (`core/dbt/parser/schemas.py`), so an *empty* `tests:` list is falsy and behaves as if the
    // key were absent: it is not renamed, it does not conflict with a real `data_tests:`, and --
    // where inheritance applies, as on a model version -- it does not override the inherited
    // value. An empty `data_tests:` list is a real value and does override.
    let tests = tests.filter(|tests| !tests.is_empty());
    if tests.is_some() && data_tests.is_some() {
        return err!(
            ErrorCode::InvalidSchema,
            "Cannot have both 'tests' and 'data_tests' defined"
        );
    }
    if let Some(data_tests) = data_tests {
        Ok(Some(data_tests.to_vec()))
    } else if let Some(tests) = tests {
        Ok(Some(tests.to_vec()))
    } else {
        Ok(None) // Return an empty map if there are no tests
    }
}

pub fn column_tests_inner(
    columns: &Option<Vec<ColumnProperties>>,
) -> FsResult<Option<BTreeMap<String, ColumnTestEntry>>> {
    if columns.is_some()
        && columns
            .as_ref()
            .unwrap()
            .iter()
            .any(|col| col.tests.is_some() && col.data_tests.is_some())
    {
        return err!(
            ErrorCode::InvalidSchema,
            "Cannot have both 'tests' and 'data_tests' defined"
        );
    }
    if let Some(columns) = columns {
        let column_tests = columns
            .iter()
            .filter_map(|col| {
                // Check for both tests and data_tests, and handle them appropriately
                if col.tests.is_some() && col.data_tests.is_some() {
                    return None; // Error is handled above
                }
                (col.tests)
                    .as_ref()
                    .or((col.data_tests).as_ref())
                    .map(|tests| {
                        let tags = col
                            .config
                            .as_ref()
                            .and_then(|c| c.tags.clone())
                            .map(|t| t.into())
                            .unwrap_or_default();
                        (
                            col.name.clone(),
                            ColumnTestEntry {
                                quote: col.quote.unwrap_or(false),
                                tests: tests.clone(),
                                tags,
                                legacy_syntax_handling: LegacyTestSyntaxHandling::Strict,
                            },
                        )
                    })
            })
            .collect();
        Ok(Some(column_tests))
    } else {
        Ok(None) // Return an empty map if there are no columns
    }
}
