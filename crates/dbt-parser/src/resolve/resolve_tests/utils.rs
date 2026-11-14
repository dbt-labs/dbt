use std::collections::{BTreeMap, btree_map::Entry};

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
    let Some(columns) = columns else {
        return Ok(None);
    };
    if columns
        .iter()
        .any(|col| col.tests.is_some() && col.data_tests.is_some())
    {
        return err!(
            ErrorCode::InvalidSchema,
            "Cannot have both 'tests' and 'data_tests' defined"
        );
    }
    let mut column_tests: BTreeMap<String, ColumnTestEntry> = BTreeMap::new();
    for (name, entry) in columns.iter().filter_map(column_test_entry) {
        // A column may be declared twice; dbt-core keeps both sets of tests.
        match column_tests.entry(name) {
            Entry::Occupied(mut existing) => merge_redeclared_column(existing.get_mut(), entry),
            Entry::Vacant(slot) => {
                slot.insert(entry);
            }
        }
    }
    Ok(Some(column_tests))
}

fn column_test_entry(col: &ColumnProperties) -> Option<(String, ColumnTestEntry)> {
    let tests = col.tests.as_ref().or(col.data_tests.as_ref())?;
    let tags = col
        .config
        .as_ref()
        .and_then(|c| c.tags.clone())
        .map(|t| t.into())
        .unwrap_or_default();
    Some((
        col.name.clone(),
        ColumnTestEntry {
            quote: col.quote.unwrap_or(false),
            tests: tests.clone(),
            tags,
            legacy_syntax_handling: LegacyTestSyntaxHandling::Strict,
        },
    ))
}

fn merge_redeclared_column(existing: &mut ColumnTestEntry, later: ColumnTestEntry) {
    existing.tests.extend(later.tests);
    existing.quote = later.quote;
    existing.tags = later.tags;
    existing.legacy_syntax_handling = later.legacy_syntax_handling;
}

#[cfg(test)]
mod tests {
    use dbt_yaml::Spanned;

    use super::*;

    fn column(name: &str, tests: &[&str]) -> ColumnProperties {
        ColumnProperties {
            name: name.to_string(),
            data_tests: Some(
                tests
                    .iter()
                    .map(|t| DataTests::String(Spanned::from(t.to_string())))
                    .collect(),
            ),
            ..Default::default()
        }
    }

    fn test_names(entry: &ColumnTestEntry) -> Vec<String> {
        entry
            .tests
            .iter()
            .map(|t| match t {
                DataTests::String(name) => name.to_string(),
                DataTests::CustomTest(_) => unreachable!(),
            })
            .collect()
    }

    #[test]
    fn duplicated_column_keeps_tests_from_every_declaration() {
        let columns = Some(vec![column("id", &["unique"]), column("id", &["not_null"])]);
        let column_tests = column_tests_inner(&columns).unwrap().unwrap();
        assert_eq!(column_tests.len(), 1);
        assert_eq!(test_names(&column_tests["id"]), ["unique", "not_null"]);
    }
}
