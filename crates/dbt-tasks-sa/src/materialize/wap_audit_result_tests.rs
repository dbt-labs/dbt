//! Validate warehouse result conversion through the actual Agate boundary.

use std::sync::Arc;

use arrow::array::{
    ArrayRef, BooleanArray, Decimal128Array, Float64Array, Int64Array, RecordBatch, StringArray,
    UInt64Array,
};
use arrow::datatypes::{Field, Schema};
use dbt_agate::AgateTable;

use super::get_test_results;

fn table(columns: Vec<(&str, ArrayRef)>) -> AgateTable {
    let fields: Vec<_> = columns
        .iter()
        .map(|(name, array)| Field::new(*name, array.data_type().clone(), true))
        .collect();
    let arrays = columns.into_iter().map(|(_, array)| array).collect();
    AgateTable::from_record_batch(Arc::new(
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap(),
    ))
}

fn boolean(value: Option<bool>) -> ArrayRef {
    Arc::new(BooleanArray::from(vec![value]))
}

fn integer(value: Option<i64>) -> ArrayRef {
    Arc::new(Int64Array::from(vec![value]))
}

fn decimal(value: Option<i128>, scale: i8) -> ArrayRef {
    Arc::new(
        Decimal128Array::from(vec![value])
            .with_precision_and_scale(38, scale)
            .unwrap(),
    )
}

fn audit_table(failures: ArrayRef, should_warn: ArrayRef, should_error: ArrayRef) -> AgateTable {
    table(vec![
        ("failures", failures),
        ("should_warn", should_warn),
        ("should_error", should_error),
    ])
}

#[test]
fn wap_audit_accepts_snowflake_decimal_counts_and_resolves_reordered_uppercase_columns() {
    for failures in [0, 1, i64::MAX] {
        let table = table(vec![
            ("SHOULD_ERROR", boolean(Some(false))),
            ("FAILURES", decimal(Some(i128::from(failures)), 0)),
            ("SHOULD_WARN", boolean(Some(true))),
        ]);
        let results = get_test_results(&table, true).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].failures, failures);
        assert!(results[0].should_warn);
        assert!(!results[0].should_error);
        assert!(results[0].column_name.is_none());
    }
}

#[test]
fn wap_audit_accepts_boolean_verdicts_without_changing_their_meaning() {
    for should_warn in [false, true] {
        for should_error in [false, true] {
            let table = audit_table(
                integer(Some(0)),
                boolean(Some(should_warn)),
                boolean(Some(should_error)),
            );
            let results = get_test_results(&table, true).unwrap();
            assert_eq!(results[0].should_warn, should_warn);
            assert_eq!(results[0].should_error, should_error);
        }
    }
}

#[test]
fn wap_audit_rejects_null_counts_and_null_verdicts() {
    for table in [
        audit_table(integer(None), boolean(Some(false)), boolean(Some(false))),
        audit_table(decimal(None, 0), boolean(Some(false)), boolean(Some(false))),
        audit_table(integer(Some(0)), boolean(None), boolean(Some(false))),
        audit_table(integer(Some(0)), boolean(Some(false)), boolean(None)),
    ] {
        assert!(get_test_results(&table, true).is_err());
    }
}

#[test]
fn wap_audit_rejects_invalid_failure_counts_instead_of_coercing_them() {
    let invalid: Vec<(&str, ArrayRef)> = vec![
        ("false", boolean(Some(false))),
        ("true", boolean(Some(true))),
        ("integral float", Arc::new(Float64Array::from(vec![0.0]))),
        ("fractional float", Arc::new(Float64Array::from(vec![0.5]))),
        ("NaN", Arc::new(Float64Array::from(vec![f64::NAN]))),
        (
            "infinity",
            Arc::new(Float64Array::from(vec![f64::INFINITY])),
        ),
        ("negative", integer(Some(-1))),
        ("negative decimal", decimal(Some(-1), 0)),
        ("scaled decimal", decimal(Some(100), 2)),
        (
            "overflow decimal",
            decimal(Some(i128::from(i64::MAX) + 1), 0),
        ),
        (
            "overflow unsigned",
            Arc::new(UInt64Array::from(vec![u64::MAX])),
        ),
        ("string", Arc::new(StringArray::from(vec!["0"]))),
    ];
    for (description, failures) in invalid {
        let table = audit_table(failures, boolean(Some(false)), boolean(Some(false)));
        assert!(
            get_test_results(&table, true).is_err(),
            "accepted {description} failures"
        );
    }
}

#[test]
fn wap_audit_rejects_string_and_numeric_verdicts() {
    let invalid: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(vec!["false"])),
        Arc::new(StringArray::from(vec!["TRUE"])),
        Arc::new(StringArray::from(vec![""])),
        integer(Some(0)),
        integer(Some(1)),
        Arc::new(Float64Array::from(vec![0.0])),
    ];
    for value in invalid {
        for table in [
            audit_table(integer(Some(0)), Arc::clone(&value), boolean(Some(false))),
            audit_table(integer(Some(0)), boolean(Some(false)), Arc::clone(&value)),
        ] {
            assert!(get_test_results(&table, true).is_err());
        }
    }
}

#[test]
fn wap_audit_rejects_missing_extra_and_duplicate_result_columns() {
    for columns in [
        vec![
            ("failures", integer(Some(0))),
            ("should_warn", boolean(Some(false))),
        ],
        vec![
            ("failures", integer(Some(0))),
            ("should_warn", boolean(Some(false))),
            ("should_error", boolean(Some(false))),
            ("extra", integer(Some(0))),
        ],
        vec![
            ("wrong_name", integer(Some(0))),
            ("should_warn", boolean(Some(false))),
            ("should_error", boolean(Some(false))),
        ],
        vec![
            ("failures", integer(Some(0))),
            ("FAILURES", integer(Some(0))),
            ("should_error", boolean(Some(false))),
        ],
        vec![
            ("failures", integer(Some(0))),
            ("should_warn", boolean(Some(false))),
            ("SHOULD_WARN", boolean(Some(false))),
        ],
        vec![
            (
                "column_name",
                Arc::new(StringArray::from(vec!["id"])) as ArrayRef,
            ),
            ("failures", integer(Some(0))),
            ("should_warn", boolean(Some(false))),
            ("should_error", boolean(Some(false))),
        ],
    ] {
        assert!(get_test_results(&table(columns), true).is_err());
    }
}

#[test]
fn wap_audit_requires_exactly_one_result_row() {
    for rows in [0, 2] {
        let table = audit_table(
            Arc::new(Int64Array::from(vec![0; rows])),
            Arc::new(BooleanArray::from(vec![false; rows])),
            Arc::new(BooleanArray::from(vec![false; rows])),
        );
        assert!(get_test_results(&table, true).is_err());
    }
}

#[test]
fn ordinary_test_result_coercion_remains_unchanged() {
    let table = audit_table(
        integer(None),
        Arc::new(StringArray::from(vec!["FALSE"])),
        boolean(None),
    );
    let results = get_test_results(&table, false).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].failures, -1);
    assert!(!results[0].should_warn);
    assert!(!results[0].should_error);
}
