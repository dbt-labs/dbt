/// Provides functionality for processing symbolic constraints and generating SQL statements.
/// This module contains functions for solving constraints, generating CREATE TABLE statements,
/// and collecting results by table.
use std::collections::HashMap;

use crate::concolic_profile::ConcolicProfile;
use crate::sym_solver::solve_constraint;
use crate::symbolic_nodes::SQLColumn;
use crate::symbolic_nodes::get_row_and_column_expressions_from_node;
use crate::symbolic_nodes::simplify_holes;
use crate::symbolic_nodes::simplify_holes_relaxed;
use crate::symbolic_nodes::{
    SymbolicExpr, SymbolicScalarValue, arrow_type_to_sql_type, contains_gen_var,
};
use datafusion::physical_plan::ExecutionPlan;
use std::sync::Arc;

fn quote_if_needed(name: &str) -> String {
    // If already quoted, leave as is
    if name.len() >= 2 && name.starts_with('"') && name.ends_with('"') {
        return name.to_string();
    }
    let mut chars = name.chars();
    let needs_quotes = name.is_empty()
        || !matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        || chars.any(|c| !(c.is_ascii_alphanumeric() || c == '_'));
    if needs_quotes {
        let escaped = name.replace('"', "\"\"");
        format!("\"{}\"", escaped)
    } else {
        name.to_string()
    }
}

/// Processes symbolic constraints and generates SQL statements.
///
/// # Arguments
/// * `plan` - The execution plan to process.
/// * `slt_file_path` - Path to the SLT file (for logging).
/// * `timing` - Optional accumulator for per-step timing and hole counts.
///
/// # Returns
/// A tuple containing a map of table names to CREATE TABLE statements and a boolean indicating if convergence was reached.
pub fn process_symbolic_constraints(
    plan: &Arc<dyn ExecutionPlan>,
    slt_file_path: &str,
    mut timing: Option<&mut ConcolicProfile>,
) -> (HashMap<String, String>, bool) {
    let (row_expression, _, table_columns) = get_row_and_column_expressions_from_node(plan);
    {
        let row_constraints = match row_expression.clone() {
            SymbolicExpr::GenericRow {
                table: _,
                constraints,
            } => constraints,
            x => panic!("Expected GenericRow: {:?}", x),
        };
        if row_constraints.is_some()
            && !row_constraints.as_ref().unwrap().is_empty()
            && row_constraints
                .as_ref()
                .unwrap()
                .iter()
                .any(contains_gen_var)
        {
            let row_expression = row_expression;
            // Optimizations might combine predicates of different filters so we need to first try
            // to drop holes within the elements rather than removing the elements entirely.

            let (simplified, holes_dropped) = simplify_holes(&row_expression);

            let mut solution = solve_constraints(&simplified, slt_file_path, &mut timing);

            // simplify_holes can make a satisfiable constraint UNSAT,
            // so retry by just relaxing constraint.
            if solution.is_empty() && holes_dropped {
                let (relaxed, _) = simplify_holes_relaxed(&row_expression);
                solution = solve_constraints(&relaxed, slt_file_path, &mut timing);
            }
            if let Some(ref mut t) = timing {
                t.final_holes = if solution.is_empty() {
                    t.initial_holes
                } else {
                    0
                };
            }

            if let Some(value) = &timing {
                println!(
                    "Initial holes: {}, final holes: {}, holes dropped: {}",
                    value.initial_holes, value.final_holes, holes_dropped
                );
            }

            if solution.is_empty() {
                return (HashMap::new(), true);
            }
            let sym_var_by_tables = collect_z3_results_by_table(&solution);
            let create_table_sql = generate_create_table_sql(
                &solution,
                &table_columns,
                &sym_var_by_tables,
                slt_file_path,
            );
            return (create_table_sql, !holes_dropped);
        }
    }
    (HashMap::new(), true)
}

/// Solves symbolic constraints and returns a map of symbolic variable names to their values.
///
/// # Arguments
/// * `constraints` - The symbolic constraints to solve.
///
/// # Returns
/// A map of symbolic variable names to their values.
fn solve_constraints(
    constraints: &SymbolicExpr,
    slt_file_path: &str,
    timing: &mut Option<&mut ConcolicProfile>,
) -> HashMap<String, SymbolicScalarValue> {
    for i in 1..=10 {
        if let Some(solution) = solve_constraint(constraints, i, timing) {
            return solution;
        }
    }
    // No solution at all for this file
    eprintln!("No solution found at all: {}", slt_file_path);
    HashMap::new()
}

/// Generates CREATE TABLE statements from symbolic variable names and their values.
///
/// # Arguments
/// * `solution` - A map of symbolic variable names to their values.
/// * `table_columns` - A map of table names to their columns.
/// * `sym_var_by_tables` - A map of table names to their symbolic variable names.
///
fn generate_create_table_sql(
    solution: &HashMap<String, SymbolicScalarValue>,
    table_columns: &HashMap<String, Vec<SQLColumn>>,
    sym_var_by_tables: &HashMap<String, Vec<String>>,
    slt_file_path: &str,
) -> HashMap<String, String> {
    let mut create_table_sql: HashMap<String, String> = HashMap::new();
    let mut cardinalities: Vec<usize> = vec![];
    for (table_name, sym_vars) in sym_var_by_tables.iter() {
        if sym_vars.is_empty() {
            continue;
        }
        // We want the sort the collected symbolic variables first by their row id and then by their
        // column id to make it easier to generate the CREATE TABLE statements.
        let mut sorted = sym_vars.clone();
        let mut create_statement: HashMap<usize, Vec<String>> = HashMap::new();
        sorted.sort_by_key(|k| {
            // split off into [<table_name>, <column>, <row>, <column_name>]
            let mut parts = k.split('.').skip(1);
            let col = parts.next().unwrap().parse::<usize>().unwrap();
            let row = parts.next().unwrap().parse::<usize>().unwrap();
            // Sort by (row, col)
            (row, col)
        });
        // Get max row id from sorted
        let max_row_id = sorted
            .last()
            .unwrap()
            .split('.')
            .nth(2)
            .unwrap()
            .parse::<usize>()
            .unwrap();
        cardinalities.push(max_row_id);
        // Generate the CREATE TABLE statements (max_row_id is 1-based!)
        for row_id in 1..=max_row_id {
            for (idx, sql_column) in table_columns.get(table_name).unwrap().iter().enumerate() {
                let column_name = sql_column.column_name.clone();
                let column_sql_type = arrow_type_to_sql_type(sql_column.arrow_type.clone());
                let key = format!("{table_name}.{idx}.{row_id}.{column_name}");
                let alias = quote_if_needed(&column_name);
                let column_statement = if sym_vars.contains(&key) {
                    match solution.get(&key) {
                        Some(SymbolicScalarValue::Utf8(s)) => match s {
                            Some(s) => format!("'{s}'::VARCHAR AS {alias}"),
                            None => format!("NULL::VARCHAR AS {alias}"),
                        },
                        _ => {
                            let value =
                                SymbolicScalarValue::to_string_value(solution.get(&key).unwrap());
                            format!("{value}::{column_sql_type} AS {alias}")
                        }
                    }
                } else {
                    format!("NULL::{column_sql_type} AS {alias}")
                };
                create_statement
                    .entry(row_id)
                    .and_modify(|columns| columns.push(column_statement.clone()))
                    .or_insert_with(|| vec![column_statement]);
            }
        }

        let mut row_statements: Vec<String> = vec![];
        let mut row_ids: Vec<usize> = create_statement.keys().copied().collect();
        row_ids.sort_unstable();
        for row_id in row_ids {
            let columns = create_statement.get(&row_id).unwrap();
            let row_statement = format!("\tSELECT {}", columns.join(", "));
            row_statements.push(row_statement);
        }
        create_table_sql.insert(
            table_name.clone(),
            format!(
                "CREATE TABLE {table_name} AS (\n{}\n)",
                row_statements.join("\n\tUNION ALL\n")
            ),
        );
    }

    // Emit a single-line cardinalities stat if requested
    if crate::symbolic_nodes::utils::collect_stats_enabled() {
        println!("Cardinalities: {:?}", cardinalities);

        let list = cardinalities
            .into_iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",");
        println!("+++ input_cardinalities {},{}", slt_file_path, list);
    }
    create_table_sql
}

/// Collects Z3 results by table.
///
/// # Arguments
/// * `results` - A map of symbolic variable names to their values.
///
/// # Returns
/// A map of table names to their symbolic variable names.
fn collect_z3_results_by_table(
    results: &HashMap<String, SymbolicScalarValue>,
) -> HashMap<String, Vec<String>> {
    let mut table_results: HashMap<String, Vec<String>> = HashMap::new();
    for var_name in results.keys() {
        let table_name = var_name.split(".").next().unwrap();
        if table_results.contains_key(table_name) {
            table_results
                .get_mut(table_name)
                .unwrap()
                .push(var_name.clone());
        } else {
            table_results.insert(table_name.to_string(), vec![var_name.clone()]);
        }
    }
    table_results
}
