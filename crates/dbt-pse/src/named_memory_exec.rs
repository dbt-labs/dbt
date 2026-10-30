/// Provides functionality for injecting the table names from the logical plan into the physical plan.
use arrow::datatypes::SchemaRef;
use async_trait::async_trait;
use datafusion::error::{DataFusionError, Result};
use datafusion::execution::context::QueryPlanner;
use datafusion::execution::context::TaskContext;
use datafusion::execution::session_state::SessionState;
use datafusion::physical_expr::PhysicalExpr;
use datafusion::physical_plan::metrics::MetricsSet;
use datafusion::physical_plan::{
    DisplayAs, DisplayFormatType, ExecutionPlan, PlanProperties, SendableRecordBatchStream,
};
use datafusion::physical_planner::{DefaultPhysicalPlanner, ExtensionPlanner, PhysicalPlanner};
use datafusion_common::tree_node::{Transformed, TreeNode};
use datafusion_common::{DFSchema, DFSchemaRef};
use datafusion_expr::LogicalPlan;
use datafusion_expr::UserDefinedLogicalNode;
use datafusion_expr::logical_plan::Extension;
use datafusion_expr::{Expr, InvariantLevel};

use std::any::Any;
use std::hash::Hasher;
use std::sync::Arc;

/// Execution node that wraps a MemoryExec node with a table name.
/// This node passes through all data unchanged.
#[derive(Debug)]
pub struct NamedMemoryExec {
    /// The child execution plan
    child: Arc<dyn ExecutionPlan>,
    /// The table name
    table_name: String,
}

impl NamedMemoryExec {
    /// Creates a new NamedMemoryExec node.
    ///
    /// # Arguments
    /// * `child` - The execution plan whose schema needs to be stripped
    fn new(child: Arc<dyn ExecutionPlan>, table_name: String) -> Self {
        Self { child, table_name }
    }

    /// Returns the table name.
    #[allow(unused)]
    pub fn table_name(&self) -> &str {
        &self.table_name
    }

    /// Returns the child execution plan.
    #[allow(unused)]
    pub fn child(&self) -> &Arc<dyn ExecutionPlan> {
        &self.child
    }
}

impl ExecutionPlan for NamedMemoryExec {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.child.schema()
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        self.child.properties()
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![&self.child]
    }

    fn with_new_children(
        self: Arc<Self>,
        new_children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        assert_eq!(new_children.len(), 1);
        Ok(Arc::new(NamedMemoryExec::new(
            new_children[0].clone(),
            self.table_name.clone(),
        )))
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        // Just pass through the execution since we only modify the schema
        self.child.execute(partition, context)
    }

    fn name(&self) -> &str {
        "NamedMemoryExec"
    }

    fn metrics(&self) -> Option<MetricsSet> {
        self.child.metrics()
    }
}

impl DisplayAs for NamedMemoryExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::TreeRender => {
                write!(f, "NamedMemoryExec")
            }
            DisplayFormatType::Verbose => {
                write!(f, "NamedMemoryExec: table_name={}", self.table_name)
            }
        }
    }
}

/// Logical node that wraps a TableScan node with a table name.
#[derive(Debug, Clone)]
pub struct NamedTableScanNode {
    input: LogicalPlan,
    table_name: String,
}

impl NamedTableScanNode {
    pub fn new(input: LogicalPlan, table_name: String) -> Self {
        Self { input, table_name }
    }
}

impl UserDefinedLogicalNode for NamedTableScanNode {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "NamedTableScanNode"
    }

    fn inputs(&self) -> Vec<&LogicalPlan> {
        vec![&self.input]
    }

    fn schema(&self) -> &DFSchemaRef {
        self.input.schema()
    }

    fn check_invariants(&self, check: InvariantLevel) -> Result<()> {
        self.input.check_invariants(check)
    }

    fn expressions(&self) -> Vec<Expr> {
        vec![]
    }

    fn fmt_for_explain(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "NamedTableScanNode: {}", self.table_name)
    }

    fn with_exprs_and_inputs(
        &self,
        _: Vec<Expr>,
        inputs: Vec<LogicalPlan>,
    ) -> Result<Arc<dyn UserDefinedLogicalNode>> {
        Ok(Arc::new(Self {
            input: inputs[0].clone(),
            table_name: self.table_name.clone(),
        }))
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        state.write(self.table_name.as_bytes())
    }

    fn dyn_eq(&self, other: &dyn UserDefinedLogicalNode) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|other| self.table_name == other.table_name)
    }

    fn dyn_ord(&self, other: &dyn UserDefinedLogicalNode) -> Option<std::cmp::Ordering> {
        other
            .as_any()
            .downcast_ref::<Self>()
            .map(|other| self.table_name.cmp(&other.table_name))
    }
}

/// Physical planner for wrapping MemoryExec nodes with NamedMemoryExec nodes where
/// the table names are injected from the logical plan's TableScan nodes.
pub struct TestGenerationPhysicalPlanner {
    default_physical_planner: DefaultPhysicalPlanner,
}

impl TestGenerationPhysicalPlanner {
    pub fn new() -> Self {
        Self {
            // We need to add the TestGenerationExtensionPlanner to the physical planner
            // so that Datafusion knows how to plan the NamedTableScanNode into a NamedMemoryExec.
            default_physical_planner: DefaultPhysicalPlanner::with_extension_planners(vec![
                Arc::new(TestGenerationExtensionPlanner {}),
            ]),
        }
    }
}

impl Default for TestGenerationPhysicalPlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PhysicalPlanner for TestGenerationPhysicalPlanner {
    /// Create a physical plan from a logical plan
    async fn create_physical_plan(
        &self,
        logical_plan: &LogicalPlan,
        session_state: &SessionState,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        self.default_physical_planner
            .create_physical_plan(logical_plan, session_state)
            .await
    }

    fn create_physical_expr(
        &self,
        expr: &Expr,
        input_dfschema: &DFSchema,
        session_state: &SessionState,
    ) -> Result<Arc<dyn PhysicalExpr>> {
        self.default_physical_planner
            .create_physical_expr(expr, input_dfschema, session_state)
    }
}

/// Session state builder only accepts QueryPlanner so we wrap our physical planner in this.
/// See: https://github.com/apache/datafusion/issues/13943 for more details.
pub struct TestGenerationQueryPlanner {
    inner: TestGenerationPhysicalPlanner,
}

impl std::fmt::Debug for TestGenerationQueryPlanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TestGenerationQueryPlanner")
    }
}

impl TestGenerationQueryPlanner {
    pub fn new() -> Self {
        Self {
            inner: TestGenerationPhysicalPlanner::default(),
        }
    }

    /// Wraps any TableScan nodes in NamedTableScanNode.
    fn wrap_table_scan_node(&self, logical_plan: &LogicalPlan) -> LogicalPlan {
        // Transform the plan bottom-up, wrapping any TableScan nodes in NamedTableScanNode
        logical_plan
            .clone()
            .transform_up(|plan| match &plan {
                LogicalPlan::TableScan(scan) => {
                    let table_name = scan.table_name.clone().table().to_string();
                    Ok(Transformed::yes(LogicalPlan::Extension(Extension {
                        node: Arc::new(NamedTableScanNode::new(plan, table_name)),
                    })))
                }
                _ => Ok(Transformed::no(plan)),
            })
            .unwrap()
            .data
    }
}

impl Default for TestGenerationQueryPlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl QueryPlanner for TestGenerationQueryPlanner {
    async fn create_physical_plan(
        &self,
        logical_plan: &LogicalPlan,
        session_state: &SessionState,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let logical_plan = self.wrap_table_scan_node(logical_plan);
        self.inner
            .create_physical_plan(&logical_plan, session_state)
            .await
    }
}

/// Extension planner for NamedTableScanNode. This planner is used to plan
/// NamedTableScanNode into NamedMemoryExec such that the table name is copied from the
/// wrapped TableScan node.
pub struct TestGenerationExtensionPlanner;

#[async_trait::async_trait]
impl ExtensionPlanner for TestGenerationExtensionPlanner {
    async fn plan_extension(
        &self,
        _: &dyn PhysicalPlanner,
        node: &dyn UserDefinedLogicalNode,
        _: &[&LogicalPlan],
        physical_inputs: &[Arc<dyn ExecutionPlan>],
        _: &SessionState,
    ) -> Result<Option<Arc<dyn ExecutionPlan>>> {
        // Plan the NamedTableScanNode into a NamedMemoryExec.
        if let Some(named_table_scan_node) = node.as_any().downcast_ref::<NamedTableScanNode>() {
            return Ok(Some(Arc::new(NamedMemoryExec::new(
                physical_inputs[0].clone(),
                named_table_scan_node.table_name.clone(),
            ))));
        }
        Err(DataFusionError::Plan("Unsupported logical node".into()))
    }
}
