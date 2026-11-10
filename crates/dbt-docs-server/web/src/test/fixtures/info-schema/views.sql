-- dbt information schema. Generated; do not edit.
--
-- Query with:
--   duckdb -cmd ".read views.sql"
--
-- Objects in dbt_internal are not part of the public contract and may
-- change without notice.

CREATE SCHEMA IF NOT EXISTS dbt;
CREATE SCHEMA IF NOT EXISTS dbt_rt;
CREATE SCHEMA IF NOT EXISTS dbt_internal;

CREATE OR REPLACE VIEW dbt.project AS SELECT * FROM read_parquet('dbt.project.parquet');
CREATE OR REPLACE VIEW dbt.packages AS SELECT * FROM read_parquet('dbt.packages.parquet');
CREATE OR REPLACE VIEW dbt.project_vars AS SELECT * FROM read_parquet('dbt.project_vars.parquet');
CREATE OR REPLACE VIEW dbt.project_env_vars AS SELECT * FROM read_parquet('dbt.project_env_vars.parquet');
CREATE OR REPLACE VIEW dbt.models AS SELECT * FROM read_parquet('dbt.models.parquet');
CREATE OR REPLACE VIEW dbt.seeds AS SELECT * FROM read_parquet('dbt.seeds.parquet');
CREATE OR REPLACE VIEW dbt.snapshots AS SELECT * FROM read_parquet('dbt.snapshots.parquet');
CREATE OR REPLACE VIEW dbt.functions AS SELECT * FROM read_parquet('dbt.functions.parquet');
CREATE OR REPLACE VIEW dbt.analyses AS SELECT * FROM read_parquet('dbt.analyses.parquet');
CREATE OR REPLACE VIEW dbt.hooks AS SELECT * FROM read_parquet('dbt.hooks.parquet');
CREATE OR REPLACE VIEW dbt.checks AS SELECT * FROM read_parquet('dbt.checks.parquet');
CREATE OR REPLACE VIEW dbt.sources AS SELECT * FROM read_parquet('dbt.sources.parquet');
CREATE OR REPLACE VIEW dbt.data_tests AS SELECT * FROM read_parquet('dbt.data_tests.parquet');
CREATE OR REPLACE VIEW dbt.unit_tests AS SELECT * FROM read_parquet('dbt.unit_tests.parquet');
CREATE OR REPLACE VIEW dbt.macros AS SELECT * FROM read_parquet('dbt.macros.parquet');
CREATE OR REPLACE VIEW dbt.groups AS SELECT * FROM read_parquet('dbt.groups.parquet');
CREATE OR REPLACE VIEW dbt.exposures AS SELECT * FROM read_parquet('dbt.exposures.parquet');
CREATE OR REPLACE VIEW dbt.metrics AS SELECT * FROM read_parquet('dbt.metrics.parquet');
CREATE OR REPLACE VIEW dbt.docs_blocks AS SELECT * FROM read_parquet('dbt.docs_blocks.parquet');
CREATE OR REPLACE VIEW dbt.saved_queries AS SELECT * FROM read_parquet('dbt.saved_queries.parquet');
CREATE OR REPLACE VIEW dbt.semantic_models AS SELECT * FROM read_parquet('dbt.semantic_models.parquet');
CREATE OR REPLACE VIEW dbt.semantic_entities AS SELECT * FROM read_parquet('dbt.semantic_entities.parquet');
CREATE OR REPLACE VIEW dbt.semantic_measures AS SELECT * FROM read_parquet('dbt.semantic_measures.parquet');
CREATE OR REPLACE VIEW dbt.semantic_dimensions AS SELECT * FROM read_parquet('dbt.semantic_dimensions.parquet');
CREATE OR REPLACE VIEW dbt.semantic_relationships AS SELECT * FROM read_parquet('dbt.semantic_relationships.parquet');
CREATE OR REPLACE VIEW dbt.time_spines AS SELECT * FROM read_parquet('dbt.time_spines.parquet');
CREATE OR REPLACE VIEW dbt.dag_nodes AS SELECT * FROM read_parquet('dbt.dag_nodes.parquet');
CREATE OR REPLACE VIEW dbt.edges AS SELECT * FROM read_parquet('dbt.edges.parquet');
CREATE OR REPLACE VIEW dbt.node_columns AS SELECT * FROM read_parquet('dbt.node_columns.parquet');
CREATE OR REPLACE VIEW dbt.column_lineage AS SELECT * FROM read_parquet('dbt.column_lineage.parquet');
CREATE OR REPLACE VIEW dbt.classifiers AS SELECT * FROM read_parquet('dbt.classifiers.parquet');

CREATE OR REPLACE VIEW dbt_rt.invocations AS SELECT * FROM read_parquet('dbt_rt.invocations.parquet');
CREATE OR REPLACE VIEW dbt_rt.run_results AS SELECT * FROM read_parquet('dbt_rt.run_results.parquet');
CREATE OR REPLACE VIEW dbt_rt.freshness AS SELECT * FROM read_parquet('dbt_rt.freshness.parquet');
CREATE OR REPLACE VIEW dbt_rt.relations AS SELECT * FROM read_parquet('dbt_rt.relations.parquet');
CREATE OR REPLACE VIEW dbt_rt.diagnostics AS SELECT * FROM read_parquet('dbt_rt.diagnostics.parquet');
CREATE OR REPLACE VIEW dbt_rt.adapter_queries AS SELECT * FROM read_parquet('dbt_rt.adapter_queries.parquet');

CREATE OR REPLACE VIEW dbt_internal.node_input_files AS SELECT * FROM read_parquet('dbt_internal.node_input_files.parquet');

CREATE OR REPLACE VIEW dbt_rt.run_results_latest AS
SELECT * FROM dbt_rt.run_results
WHERE NOT (status = 'error' AND execution_time = 0)
QUALIFY ROW_NUMBER() OVER (PARTITION BY unique_id ORDER BY created_at DESC) = 1;

CREATE OR REPLACE VIEW dbt_internal.resources AS
SELECT * FROM dbt.models
UNION ALL BY NAME SELECT * FROM dbt.seeds
UNION ALL BY NAME SELECT * FROM dbt.snapshots
UNION ALL BY NAME SELECT * FROM dbt.functions
UNION ALL BY NAME SELECT * FROM dbt.analyses
UNION ALL BY NAME SELECT * FROM dbt.hooks
UNION ALL BY NAME SELECT * FROM dbt.checks
UNION ALL BY NAME SELECT * FROM dbt.sources
UNION ALL BY NAME SELECT *, 'test' AS resource_type FROM dbt.data_tests;

