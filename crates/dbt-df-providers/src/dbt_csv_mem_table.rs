//! Custom CSV reading for dbt seeds using agate-compatible inference.
//!
//! This module provides utilities for reading CSV files with Python/dbt/agate-compatible
//! type inference and parsing logic, designed for dbt seed files.

use datafusion::datasource::MemTable;
use datafusion::datasource::TableProvider;
use datafusion_common::DataFusionError;
use dbt_csv::{CustomCsvOptions, read_to_arrow_records};
use std::path::Path;
use std::sync::Arc;

/// Creates a table provider using custom CSV inference and reading.
///
/// This reads the entire CSV into memory using our custom reader.
/// Designed for dbt seed files which are typically small (no more than 10MB).
///
/// Uses Python/agate-compatible type inference that matches dbt behavior.
///
/// # Returns
/// A tuple of:
/// - The table provider
/// - List of column names from `text_columns` that didn't match any header
///   (for warning purposes)
pub fn make_custom_csv_mem_table(
    path: &Path,
    options: &CustomCsvOptions,
) -> Result<(Arc<dyn TableProvider>, Vec<String>), DataFusionError> {
    let result =
        read_to_arrow_records(path, options).map_err(|e| DataFusionError::External(Box::new(e)))?;

    // Create MemTable
    let table = Arc::new(MemTable::try_new(result.schema, vec![result.batches])?);
    Ok((table, result.unmatched_text_columns))
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::prelude::SessionContext;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[dbt_runtime::test]
    async fn test_custom_csv_mem_table() {
        // Create a temp CSV file
        let mut file = NamedTempFile::with_suffix(".csv").unwrap();
        writeln!(file, "id,name").unwrap();
        writeln!(file, "1,alice").unwrap();
        writeln!(file, "2,bob").unwrap();
        writeln!(file, "3,charlie").unwrap();
        file.flush().unwrap();

        let options = CustomCsvOptions::default();
        let (table, _missing_columns) = make_custom_csv_mem_table(file.path(), &options).unwrap();

        // Check schema
        let schema = table.schema();
        assert_eq!(schema.field(0).name(), "id");
        assert_eq!(schema.field(1).name(), "name");

        // Register and query
        let ctx = SessionContext::new();
        ctx.register_table("test", table).unwrap();

        let df = ctx
            .sql("SELECT id, name FROM test ORDER BY id")
            .await
            .unwrap();
        let batches = df.collect().await.unwrap();

        assert_eq!(batches[0].num_rows(), 3);
    }
}
