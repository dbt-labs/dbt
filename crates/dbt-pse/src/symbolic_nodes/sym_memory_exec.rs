use crate::named_memory_exec::NamedMemoryExec;
use crate::symbolic_nodes::SymbolicExpr;
use crate::symbolic_nodes::sym_nodes::SymbolicNode;
use crate::symbolic_nodes::{SQLColumn, SymbolicScalarValue, SymbolicZ3Type};
/// Provides functionality to wrap MemoryExec nodes with symbolic nodes for
/// building symbolic expressions that represent memory table operations.
use arrow::datatypes::SchemaRef;
use datafusion::catalog::memory::DataSourceExec;
use datafusion::error::{DataFusionError, Result};
use datafusion::execution::context::TaskContext;
use datafusion::physical_plan::DisplayAs;
use datafusion::physical_plan::DisplayFormatType;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::physical_plan::PlanProperties;
use datafusion::physical_plan::SendableRecordBatchStream;
use datafusion::physical_plan::Statistics;
use datafusion::physical_plan::metrics::MetricsSet;
use datafusion_catalog::memory::MemorySourceConfig;
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

/// Symbolic node wrapper for DataFusion MemoryExec.
/// Handles translation of in-memory tables into symbolic form and maintains
/// the symbolic state during execution.
#[derive(Debug)]
pub struct SymEvalMemoryExec {
    child: Arc<dyn ExecutionPlan>,
    row_expression: Arc<Mutex<SymbolicExpr>>,
    column_expression: Arc<Mutex<Vec<SymbolicExpr>>>,
    table_columns: Arc<Mutex<HashMap<String, Vec<SQLColumn>>>>,
}

impl SymEvalMemoryExec {
    /// Creates a new SymEvalMemoryExec node.
    ///
    /// # Arguments
    /// * `child` - The MemoryExec node to wrap
    pub fn new(child: Arc<dyn ExecutionPlan>) -> Self {
        Self {
            child,
            row_expression: Arc::new(Mutex::new(SymbolicExpr::Literal(SymbolicScalarValue::Null))),
            column_expression: Arc::new(Mutex::new(vec![])),
            table_columns: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl DisplayAs for SymEvalMemoryExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::TreeRender => {
                write!(f, "SymEvalMemoryExec")
            }
            DisplayFormatType::Verbose => {
                write!(
                    f,
                    "SymEvalMemoryExec: row_expressions={:?}",
                    self.build_row_and_column_expressions()
                )
            }
        }
    }
}

impl SymbolicNode for SymEvalMemoryExec {
    fn get_child(&self) -> &Arc<dyn ExecutionPlan> {
        &self.child
    }

    fn get_row_expression(&self) -> SymbolicExpr {
        self.row_expression.lock().unwrap().clone()
    }

    fn get_column_expression(&self) -> Vec<SymbolicExpr> {
        self.column_expression.lock().unwrap().clone()
    }

    fn get_table_columns(&self) -> HashMap<String, Vec<SQLColumn>> {
        self.table_columns.lock().unwrap().clone()
    }

    fn update_row_expression(&self, new_constraints: SymbolicExpr) {
        let mut guard = self.row_expression.lock().unwrap();
        *guard = new_constraints;
    }

    fn update_column_expression(&self, new_sym_vars: Vec<SymbolicExpr>) {
        let mut guard = self.column_expression.lock().unwrap();
        *guard = new_sym_vars;
    }

    fn update_table_columns(&self, table_columns: HashMap<String, Vec<SQLColumn>>) {
        let mut guard = self.table_columns.lock().unwrap();
        *guard = table_columns;
    }

    fn build_row_and_column_expressions(
        &self,
    ) -> (
        SymbolicExpr,
        Vec<SymbolicExpr>,
        HashMap<String, Vec<SQLColumn>>,
    ) {
        if let Some(named_memory_exec) = self.child.as_any().downcast_ref::<NamedMemoryExec>() {
            if let Some(data_source_exec) = named_memory_exec
                .child()
                .as_any()
                .downcast_ref::<DataSourceExec>()
            {
                let Some(memory_exec) = data_source_exec
                    .data_source()
                    .as_any()
                    .downcast_ref::<MemorySourceConfig>()
                else {
                    panic!("Expected MemorySourceConfig");
                };

                // We need to get the original schema to preserve all column information
                // since projections might drop some columns
                let schema = memory_exec.original_schema();
                let table_name = named_memory_exec.table_name().to_string();

                // First build the list of all columns from the schema
                let mut column_expressions = vec![];
                let mut table_columns = HashMap::new();
                let mut sql_columns = vec![];

                // For each field in the schema, create symbolic variables and SQL column metadata
                for (i, field) in schema.fields().iter().enumerate() {
                    let z3_type = SymbolicZ3Type::from_column_type(field.data_type().clone());

                    // Only include symbolic variables for projected columns if there is a projection
                    if memory_exec.projection().is_some()
                        && memory_exec.projection().as_ref().unwrap().contains(&i)
                    {
                        column_expressions.push(SymbolicExpr::generic_var(
                            table_name.to_string(),
                            i,
                            field.data_type().clone(),
                            z3_type,
                            field.name().to_string(),
                        ));
                    }

                    // Always track all columns in the table metadata
                    sql_columns.push(SQLColumn {
                        table_name: table_name.to_string(),
                        column_name: field.name().to_string(),
                        arrow_type: field.data_type().clone(),
                    });
                }

                // Store the table columns and create an empty row expression
                table_columns.insert(table_name.to_string(), sql_columns);
                let row_expressions = SymbolicExpr::generic_row(
                    table_name,
                    Some(vec![SymbolicExpr::literal(SymbolicScalarValue::Boolean(
                        Some(true),
                    ))]),
                );

                return (row_expressions, column_expressions, table_columns);
            }
            panic!("Expected MemoryExec");
        }
        panic!("Expected NamedMemoryExec");
    }
}

impl ExecutionPlan for SymEvalMemoryExec {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.child.schema()
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![&self.child]
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        if children.len() != 1 {
            return Err(DataFusionError::Internal(
                "SymEvalMemoryExec expects exactly one child".into(),
            ));
        }
        Ok(Arc::new(Self {
            child: children[0].clone(),
            row_expression: self.row_expression.clone(),
            column_expression: self.column_expression.clone(),
            table_columns: self.table_columns.clone(),
        }))
    }

    fn metrics(&self) -> Option<MetricsSet> {
        self.child.metrics()
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        // Use the default implementation from SymbolicNode
        self.execute_symbolic(partition, context)
    }

    fn name(&self) -> &str {
        "SymEvalMemoryExec"
    }

    fn partition_statistics(&self, partition: Option<usize>) -> Result<Statistics> {
        self.child.partition_statistics(partition)
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        self.child.properties()
    }
}
