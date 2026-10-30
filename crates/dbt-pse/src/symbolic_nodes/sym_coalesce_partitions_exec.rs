/// Minimal symbolic node dedicated to CoalescePartitionsExec.
/// Behaves like a pass-through for symbolic expressions but has its own type
/// so we can customize execution behavior later if needed.
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use arrow::datatypes::SchemaRef;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::error::Result;
use datafusion::execution::context::TaskContext;
use datafusion::physical_plan::DisplayAs;
use datafusion::physical_plan::DisplayFormatType;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::physical_plan::ExecutionPlanProperties;
use datafusion::physical_plan::PlanProperties;
use datafusion::physical_plan::RecordBatchStream;
use datafusion::physical_plan::SendableRecordBatchStream;
use datafusion::physical_plan::Statistics;
use datafusion::physical_plan::coalesce_partitions::CoalescePartitionsExec;
use datafusion::physical_plan::metrics::MetricsSet;
use futures::Stream;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

use crate::symbolic_nodes::SQLColumn;
use crate::symbolic_nodes::SymbolicExpr;
use crate::symbolic_nodes::SymbolicScalarValue;
use crate::symbolic_nodes::get_row_and_column_expressions_from_node;
use crate::symbolic_nodes::sym_nodes::SymbolicNode;
use crate::symbolic_nodes::sym_nodes::sym_trace_enter;

#[derive(Debug)]
pub struct SymEvalCoalescePartitionsExec {
    child: Arc<dyn ExecutionPlan>,
    row_expression: Arc<Mutex<SymbolicExpr>>,
    column_expression: Arc<Mutex<Vec<SymbolicExpr>>>,
    table_columns: Arc<Mutex<HashMap<String, Vec<SQLColumn>>>>,
}

impl SymEvalCoalescePartitionsExec {
    pub fn new(child: Arc<dyn ExecutionPlan>) -> Self {
        Self {
            child,
            row_expression: Arc::new(Mutex::new(SymbolicExpr::Literal(SymbolicScalarValue::Null))),
            column_expression: Arc::new(Mutex::new(vec![])),
            table_columns: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl DisplayAs for SymEvalCoalescePartitionsExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::TreeRender => {
                write!(f, "SymEvalCoalescePartitionsExec")
            }
            DisplayFormatType::Verbose => {
                write!(
                    f,
                    "SymEvalCoalescePartitionsExec: row_expressions={:?}",
                    self.build_row_and_column_expressions()
                )
            }
        }
    }
}

impl SymbolicNode for SymEvalCoalescePartitionsExec {
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
        // coalesce partitions is a pass-through for symbolic purposes
        let child = self.child.children()[0].clone();
        get_row_and_column_expressions_from_node(&child)
    }
}

impl ExecutionPlan for SymEvalCoalescePartitionsExec {
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
        _: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        panic!("SymEvalCoalescePartitionsExec does not support with_new_children");
    }

    fn metrics(&self) -> Option<MetricsSet> {
        self.child.metrics()
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        // Implement a synchronous, sequential coalesce to avoid async spawn/laziness and duplication
        if let Some(cp) = self.child.as_any().downcast_ref::<CoalescePartitionsExec>() {
            let node_id = format!("{:p}", self);
            let _trace = sym_trace_enter(self.name(), node_id);
            if partition != 0 {
                return datafusion_common::internal_err!(
                    "SymEvalCoalescePartitionsExec invalid partition {partition}"
                );
            }

            let input = Arc::clone(cp.input());
            let input_partitions = input.output_partitioning().partition_count();
            if input_partitions == 0 {
                return datafusion_common::internal_err!(
                    "SymEvalCoalescePartitionsExec requires at least one input partition"
                );
            }

            // Create streams for all input partitions now; this also runs upstream symbolic executes
            let mut streams: Vec<SendableRecordBatchStream> = Vec::with_capacity(input_partitions);
            for part_i in 0..input_partitions {
                let s = input.execute(part_i, Arc::clone(&context))?;
                streams.push(s);
            }

            // Build and store our symbolic state now that upstream states are initialized
            let (rows, cols, tables) = self.build_row_and_column_expressions();
            self.update_row_expression(rows);
            self.update_column_expression(cols);
            self.update_table_columns(tables);

            // Return a sequential fan-in stream over the created partition streams
            let schema = self.schema();
            Ok(Box::pin(SequentialCoalesceStream::new(schema, streams)))
        } else {
            datafusion_common::internal_err!("Expected CoalescePartitionsExec")
        }
    }

    fn name(&self) -> &str {
        "SymEvalCoalescePartitionsExec"
    }

    fn partition_statistics(&self, partition: Option<usize>) -> Result<Statistics> {
        self.child.partition_statistics(partition)
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        self.child.properties()
    }
}

struct SequentialCoalesceStream {
    schema: SchemaRef,
    streams: Vec<SendableRecordBatchStream>,
    current: usize,
}

impl SequentialCoalesceStream {
    fn new(schema: SchemaRef, streams: Vec<SendableRecordBatchStream>) -> Self {
        Self {
            schema,
            streams,
            current: 0,
        }
    }
}

impl Stream for SequentialCoalesceStream {
    type Item = Result<RecordBatch>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let me = self.get_mut();
        loop {
            if me.current >= me.streams.len() {
                return Poll::Ready(None);
            }
            let stream = &mut me.streams[me.current];
            match Stream::poll_next(Pin::new(stream), cx) {
                Poll::Ready(Some(item)) => return Poll::Ready(Some(item)),
                Poll::Ready(None) => {
                    me.current += 1;
                    continue;
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl RecordBatchStream for SequentialCoalesceStream {
    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }
}
