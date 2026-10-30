use std::collections::HashMap;

use crate::symbolic_nodes::NodeCounts;

/// Per-step timing and hole counts for concolic test generation.
#[derive(Default, Debug, Clone, serde::Serialize)]
pub struct ConcolicProfile {
    /// Time spent creating tables (CTAS), in milliseconds.
    pub create_tables_ms: u64,
    /// Time spent wrapping the physical plan with symbolic nodes, in milliseconds.
    pub wrap_plan_ms: u64,
    /// Time spent collecting constraints, in milliseconds.
    pub collect_constraints_ms: u64,
    /// Time spent lowering constraints to first-order logic for Z3, in milliseconds.
    pub lowering_fol_ms: Vec<u64>,
    /// Time spent in the Z3 solver, in milliseconds.
    pub z3_solver_ms: Vec<u64>,
    /// Number of symbolic holes before test input generation.
    pub initial_holes: usize,
    /// Number of symbolic holes after test input generation (0 when solved).
    pub final_holes: usize,
    /// Number of Satisfiable solutions found.
    pub sat_solutions: usize,
    /// Number of Unsatisfiable solutions found.
    pub unsat_solutions: usize,
    /// Number of Unknown solutions found.
    pub unknown_solutions: usize,
    /// Number of iterations in the solver.
    pub solver_iterations: usize,
    /// Number of symbolic variables.
    pub symbolic_variables: Vec<usize>,
    /// Number of FOL AST nodes
    pub fol_ast_nodes: Vec<usize>,
    /// Global iteration time
    pub global_iteration_time: u64,
    /// Node counts for the physical plan.
    pub node_counts: NodeCounts,
    /// Depth of the logical plan tree (longest path from root to leaf).
    pub logical_plan_depth: usize,
    /// Width of the logical plan tree (max number of children at any node).
    pub logical_plan_width: usize,
    /// Final cardinality of the table.
    pub final_cardinality: usize,
    /// Coverage before test input generation.
    pub coverage_before: HashMap<String, u64>,
    /// Coverage after test input generation.
    pub coverage_after: HashMap<String, u64>,
}

impl ConcolicProfile {
    pub fn new() -> Self {
        Self {
            create_tables_ms: 0,
            wrap_plan_ms: 0,
            collect_constraints_ms: 0,
            lowering_fol_ms: Vec::new(),
            z3_solver_ms: Vec::new(),
            initial_holes: 0,
            final_holes: 0,
            sat_solutions: 0,
            unsat_solutions: 0,
            unknown_solutions: 0,
            solver_iterations: 0,
            symbolic_variables: Vec::new(),
            fol_ast_nodes: Vec::new(),
            global_iteration_time: 0,
            node_counts: NodeCounts::default(),
            logical_plan_depth: 0,
            logical_plan_width: 0,
            final_cardinality: 0,
            coverage_before: HashMap::new(),
            coverage_after: HashMap::new(),
        }
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}
