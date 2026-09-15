use std::sync::Arc;

use arrow_array::RecordBatch;
use dbt_index_core::{Backend, BackendError};

use super::*;

/// Backend that answers only the probe the export makes.
///
/// The export issues no SQL now — it copies files at most — so the only thing a
/// backend is asked is whether there are any resources.
struct MockBackend {
    resource_count: u64,
    column_lineage: bool,
}

impl MockBackend {
    fn new() -> Self {
        Self {
            resource_count: 6,
            column_lineage: true,
        }
    }

    /// Artifacts that exist but hold no resources.
    fn empty_index() -> Self {
        Self {
            resource_count: 0,
            column_lineage: true,
        }
    }

    /// A compile without `--static-analysis strict`: the table is there, empty.
    fn without_column_lineage() -> Self {
        Self {
            resource_count: 6,
            column_lineage: false,
        }
    }
}

impl Backend for MockBackend {
    fn is_available(&self) -> bool {
        true
    }

    fn table_has_rows(&self, table: &str) -> bool {
        table == "dbt.column_lineage" && self.column_lineage
    }

    fn query_scalar(&self, sql: &str) -> Option<String> {
        if sql.contains("COUNT(*) FROM dbt_internal.resources") {
            return Some(self.resource_count.to_string());
        }
        None
    }

    fn query_arrow(&self, _sql: &str) -> Result<Vec<RecordBatch>, BackendError> {
        Ok(vec![])
    }
}

struct Harness {
    dir: tempfile::TempDir,
    info_schema_dir: PathBuf,
    output_dir: PathBuf,
}

impl Harness {
    /// An information-schema directory holding one parquet per named table, plus
    /// the `views.sql` every real one carries.
    ///
    /// `output_dir` is the target directory, mirroring the real layout: the site is
    /// written there and reads `info_schema/v<n>/` beside itself.
    fn in_place(tables: &[&str]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let output_dir = dir.path().join("target");
        let info_schema_dir = output_dir.join(data_dir());
        std::fs::create_dir_all(&info_schema_dir).unwrap();
        std::fs::write(info_schema_dir.join("views.sql"), b"CREATE SCHEMA dbt;").unwrap();
        for table in tables {
            std::fs::write(
                info_schema_dir.join(format!("{table}.parquet")),
                b"parquet-stub",
            )
            .unwrap();
        }
        Self {
            dir,
            info_schema_dir,
            output_dir,
        }
    }

    /// The same artifacts, but writing the site somewhere else entirely.
    fn out_of_place(tables: &[&str]) -> Self {
        let mut harness = Self::in_place(tables);
        harness.output_dir = harness.dir.path().join("elsewhere");
        harness
    }

    fn options(&self) -> ExportOptions {
        ExportOptions {
            info_schema_dir: self.info_schema_dir.clone(),
            output_dir: self.output_dir.clone(),
            duckdb_cdn_base: None,
            analytics_enabled: false,
        }
    }
}

fn providers(backend: Arc<MockBackend>) -> Providers {
    Providers {
        backend,
        ..Providers::default()
    }
}

/// Run an export and hand back the result.
///
/// Under `cargo test` the SPA step fails — `web/dist/` is a build artifact, not a
/// fixture — so tests assert on what happens before it, or on guards that reject
/// before anything is written at all.
fn export(harness: &Harness, backend: Arc<MockBackend>) -> Result<ExportSummary, ExportError> {
    export_site(&providers(backend), &harness.options())
}

#[test]
fn missing_artifacts_are_reported_before_anything_is_written() {
    let harness = Harness::in_place(&[]);
    std::fs::remove_dir_all(&harness.info_schema_dir).unwrap();

    let err = export(&harness, Arc::new(MockBackend::new())).unwrap_err();
    assert!(matches!(err, ExportError::NoIndex { .. }), "{err:?}");
    assert!(
        !harness.output_dir.join("index.html").exists(),
        "nothing should be written when there are no artifacts"
    );
}

#[test]
fn an_empty_data_dir_is_not_an_information_schema() {
    // The directory exists but holds no parquet — the state after a run that never
    // wrote one.
    let harness = Harness::in_place(&[]);

    let err = export(&harness, Arc::new(MockBackend::new())).unwrap_err();
    assert!(matches!(err, ExportError::NoIndex { .. }), "{err:?}");
}

#[test]
fn artifacts_with_no_resources_are_refused() {
    // Artifacts present but holding no rows: a partially written set. Shipping a
    // site that renders nothing is worse than failing.
    let harness = Harness::in_place(&["dbt.models"]);

    let err = export(&harness, Arc::new(MockBackend::empty_index())).unwrap_err();
    assert!(matches!(err, ExportError::EmptyIndex { .. }), "{err:?}");
}

#[test]
fn an_in_place_information_schema_is_read_where_it_lies() {
    let harness = Harness::in_place(&["dbt.models", "dbt.edges"]);
    let before = std::fs::read_dir(&harness.info_schema_dir).unwrap().count();

    let _ = export(&harness, Arc::new(MockBackend::new()));

    assert_eq!(
        std::fs::read_dir(&harness.info_schema_dir).unwrap().count(),
        before,
        "the information schema must not gain files"
    );
    assert!(
        !harness.output_dir.join("docs_data").exists(),
        "no derived data directory should be created"
    );
}

#[test]
fn writing_elsewhere_copies_the_information_schema_verbatim() {
    // A self-contained site still needs the data, and it travels as an exact copy —
    // same names, same bytes — so both layouts read one contract.
    let tables = ["dbt.models", "dbt.edges", "dbt_rt.run_results"];
    let harness = Harness::out_of_place(&tables);

    let _ = export(&harness, Arc::new(MockBackend::new()));

    let dest = harness.output_dir.join(data_dir());
    for table in tables {
        let copied = dest.join(format!("{table}.parquet"));
        assert!(copied.exists(), "{table} was not copied");
        assert_eq!(
            std::fs::read(&copied).unwrap(),
            std::fs::read(harness.info_schema_dir.join(format!("{table}.parquet"))).unwrap(),
            "{table} was not copied byte for byte"
        );
    }
}

#[test]
fn views_sql_travels_with_the_parquet_but_bookkeeping_does_not() {
    // `views.sql` *is* the view surface — the browser executes it rather than
    // authoring `CREATE VIEW` of its own — so a copied site without it has
    // artifacts and no way to name them. Everything else in the directory is
    // writer bookkeeping the browser has no use for.
    let harness = Harness::out_of_place(&["dbt.models"]);
    std::fs::write(harness.info_schema_dir.join(".fusion_state.json"), b"{}").unwrap();

    let _ = export(&harness, Arc::new(MockBackend::new()));

    let dest = harness.output_dir.join(data_dir());
    assert!(
        dest.join("views.sql").exists(),
        "views.sql must travel: without it the site cannot name its relations"
    );
    assert!(
        !dest.join(".fusion_state.json").exists(),
        "writer bookkeeping should stay behind"
    );
}

#[test]
fn column_lineage_is_reported_from_rows_not_from_the_file() {
    // Rows, because the file is always there: the information schema writes every
    // table even at zero rows. It is not even a size signal — a populated
    // `dbt.column_lineage.parquet` and an empty one measured the same 1552 bytes
    // on a real project, so the size heuristic this replaced reported "no column
    // lineage" for a project that had it.
    //
    // Asking the backend is also the same question the browser asks, so the
    // progress message and the site cannot disagree.
    assert!(has_column_lineage(&MockBackend::new()));
    assert!(
        !has_column_lineage(&MockBackend::without_column_lineage()),
        "a compile without `--static-analysis strict` leaves the table empty, \
         and an empty table is not a feature"
    );
}
