use arrow::array::{Array, ArrayRef, BooleanArray};
use arrow_schema::{Fields, Schema};
use datafusion::physical_expr::{LexOrdering, PhysicalSortExpr, ScalarFunctionExpr};
use datafusion::physical_plan::{
    ColumnarValue, ExecutionPlan, PhysicalExpr,
    coalesce_partitions::CoalescePartitionsExec,
    filter::{FilterExec, FilterExecBuilder},
    joins::{
        CrossJoinExec, HashJoinExec, NestedLoopJoinExec, SortMergeJoinExec, SymmetricHashJoinExec,
        utils::JoinFilter,
    },
    metrics::{ExecutionPlanMetricsSet, Metric, MetricValue},
    placeholder_row::PlaceholderRowExec,
    repartition::RepartitionExec,
    union::{InterleaveExec, UnionExec},
    unnest::UnnestExec,
    windows::BoundedWindowAggExec,
};
use datafusion_common::ScalarValue;
use datafusion_common::error::{DataFusionError, Result};

use std::{
    any::Any,
    cell::RefCell,
    collections::HashMap,
    fmt,
    hash::{Hash, Hasher},
    sync::Arc,
};

use arrow::{datatypes::DataType, record_batch::RecordBatch};
use datafusion::physical_expr::expressions::CaseExpr;
use datafusion_catalog::memory::DataSourceExec;

use crate::named_memory_exec::NamedMemoryExec;

thread_local! {
    /// Rows matched by each WHEN condition on this thread, keyed by the
    /// condition's metric. An expression is evaluated on one thread, so the
    /// ELSE tracker can tell which rows its own evaluation matched while other
    /// partitions bump the shared metrics concurrently.
    static WHEN_MATCHES: RefCell<HashMap<*const Metric, usize>> = RefCell::new(HashMap::new());
}

fn when_matches(metric: &Arc<Metric>) -> usize {
    WHEN_MATCHES.with(|m| m.borrow().get(&Arc::as_ptr(metric)).copied().unwrap_or(0))
}

/// A physical expression that wraps another expression and tracks when it is evaluated
#[derive(Debug)]
struct CoveredFuncCallExpr {
    inner: Arc<dyn PhysicalExpr>,
    metric: Arc<Metric>,
}

impl PhysicalExpr for CoveredFuncCallExpr {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn data_type(&self, schema: &Schema) -> Result<DataType> {
        self.inner.data_type(schema)
    }

    fn nullable(&self, schema: &Schema) -> Result<bool> {
        self.inner.nullable(schema)
    }

    fn evaluate(&self, batch: &RecordBatch) -> Result<ColumnarValue> {
        // bump exactly once per partition
        if let MetricValue::Count { count, .. } = &self.metric.value() {
            count.add(1);
        }
        self.inner.evaluate(batch)
    }

    fn children(&self) -> Vec<&Arc<dyn PhysicalExpr>> {
        vec![&self.inner]
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn PhysicalExpr>>,
    ) -> Result<Arc<dyn PhysicalExpr>> {
        let [inner] = <[_; 1]>::try_from(children).map_err(|children| {
            DataFusionError::Internal(format!("Expected 1 child, got {}", children.len()))
        })?;
        Ok(Arc::new(Self {
            inner,
            metric: self.metric.clone(),
        }))
    }

    fn fmt_sql(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt_sql(f)
    }
}

impl fmt::Display for CoveredFuncCallExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl Hash for CoveredFuncCallExpr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.hash(state);
    }
}

impl PartialEq for CoveredFuncCallExpr {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for CoveredFuncCallExpr {}

/// A physical expression that wraps CASE WHEN conditions and tracks branch coverage
#[derive(Debug)]
struct CaseWhenConditionExpr {
    inner: Arc<dyn PhysicalExpr>,
    metric: Arc<Metric>, // Row counting: number of rows that matched this condition
}

impl PhysicalExpr for CaseWhenConditionExpr {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn data_type(&self, schema: &Schema) -> Result<DataType> {
        self.inner.data_type(schema)
    }

    fn nullable(&self, schema: &Schema) -> Result<bool> {
        self.inner.nullable(schema)
    }

    fn evaluate(&self, batch: &RecordBatch) -> Result<ColumnarValue> {
        let cv = self.inner.evaluate(batch)?; // delegate

        // For simple CASE expressions, we'll get the condition value directly
        // For searched CASE expressions, we'll get a boolean array
        let n_true = match &cv {
            ColumnarValue::Array(arr) => {
                if let Some(bool_arr) = arr.as_any().downcast_ref::<BooleanArray>() {
                    // Handle boolean conditions (searched CASE expression)
                    let mut count = 0;
                    for i in 0..bool_arr.len() {
                        if bool_arr.is_valid(i) && bool_arr.value(i) {
                            count += 1;
                        }
                    }
                    count
                } else {
                    // For simple CASE expressions, any non-null value is considered a match
                    // because DataFusion will have already done the equality comparison
                    // between the CASE expression value and the WHEN clause value
                    arr.len() - arr.null_count()
                }
            }
            ColumnarValue::Scalar(scalar) => match scalar {
                ScalarValue::Boolean(Some(true)) => batch.num_rows(),
                ScalarValue::Boolean(_) => 0,
                scalar => {
                    if !scalar.is_null() {
                        batch.num_rows()
                    } else {
                        0
                    }
                }
            },
        };

        // bump row counting metric
        if let MetricValue::Count { count, .. } = &self.metric.value() {
            count.add(n_true);
        }
        WHEN_MATCHES.with(|m| {
            *m.borrow_mut().entry(Arc::as_ptr(&self.metric)).or_default() += n_true;
        });

        // return the *same* condition value so CaseExpr can use it
        Ok(cv)
    }

    fn children(&self) -> Vec<&Arc<dyn PhysicalExpr>> {
        vec![&self.inner]
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn PhysicalExpr>>,
    ) -> Result<Arc<dyn PhysicalExpr>> {
        let [inner] = <[_; 1]>::try_from(children).map_err(|children| {
            DataFusionError::Internal(format!("Expected 1 child, got {}", children.len()))
        })?;
        Ok(Arc::new(Self {
            inner,
            metric: self.metric.clone(),
        }))
    }

    fn fmt_sql(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt_sql(f)
    }
}

impl fmt::Display for CaseWhenConditionExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl Hash for CaseWhenConditionExpr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.hash(state);
    }
}

impl PartialEq for CaseWhenConditionExpr {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for CaseWhenConditionExpr {}

/// A wrapper that tracks ELSE branch usage by monitoring WHEN condition metrics
#[derive(Debug)]
struct CaseElseTracker {
    inner: Arc<dyn PhysicalExpr>,
    when_metrics: Vec<Arc<Metric>>,
    else_metric: Arc<Metric>,
    initial_when_counts: Vec<usize>, // Track initial counts to detect increments
}

impl PhysicalExpr for CaseElseTracker {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn data_type(&self, schema: &Schema) -> Result<DataType> {
        self.inner.data_type(schema)
    }

    fn nullable(&self, schema: &Schema) -> Result<bool> {
        self.inner.nullable(schema)
    }

    fn evaluate(&self, batch: &RecordBatch) -> Result<ColumnarValue> {
        // Capture the rows the WHEN conditions matched on this thread so far
        let initial_counts: Vec<usize> = self.when_metrics.iter().map(when_matches).collect();

        // Evaluate the CASE expression (this will trigger WHEN condition evaluations)
        let result = self.inner.evaluate(batch)?;

        // Calculate how many rows used ELSE by subtracting WHEN matches from total rows
        let total_when_increments: usize = self
            .when_metrics
            .iter()
            .zip(initial_counts)
            .map(|(metric, initial)| when_matches(metric).saturating_sub(initial))
            .sum();

        let else_rows = batch.num_rows().saturating_sub(total_when_increments);
        if else_rows > 0
            && let MetricValue::Count { count, .. } = &self.else_metric.value()
        {
            count.add(else_rows);
        }

        Ok(result)
    }

    fn children(&self) -> Vec<&Arc<dyn PhysicalExpr>> {
        vec![&self.inner]
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn PhysicalExpr>>,
    ) -> Result<Arc<dyn PhysicalExpr>> {
        let [inner] = <[_; 1]>::try_from(children).map_err(|children| {
            DataFusionError::Internal(format!("Expected 1 child, got {}", children.len()))
        })?;

        Ok(Arc::new(Self {
            inner,
            when_metrics: self.when_metrics.clone(),
            else_metric: self.else_metric.clone(),
            initial_when_counts: self.initial_when_counts.clone(),
        }) as Arc<dyn PhysicalExpr>)
    }

    fn fmt_sql(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt_sql(f)
    }
}

impl fmt::Display for CaseElseTracker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl Hash for CaseElseTracker {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.hash(state);
    }
}

impl PartialEq for CaseElseTracker {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for CaseElseTracker {}

/// ----------------------------------------------------------------
///   **PHYSICAL-PLAN  REWRITE**  (post-planner, pre-execution)
/// ----------------------------------------------------------------
/// Recursively replace every ScalarFunctionExpr with a CoveredExpr
fn wrap_function_call_expr(
    expr: Arc<dyn PhysicalExpr>,
    metrics: &ExecutionPlanMetricsSet,
    seen_functions: &mut HashMap<String, Arc<Metric>>,
) -> Result<Arc<dyn PhysicalExpr>> {
    // (1) children first
    let new_children = expr
        .children()
        .into_iter()
        .map(|c| wrap_function_call_expr(Arc::clone(c), metrics, seen_functions))
        .collect::<Result<Vec<_>>>()?;
    let expr = expr.with_new_children(new_children)?;

    // (2) is this node a scalar function?
    if let Some(fun) = expr.as_any().downcast_ref::<ScalarFunctionExpr>() {
        // create / reuse one counter per distinct function name
        if let Some(metric) = seen_functions.get(fun.name()) {
            return Ok(Arc::new(CoveredFuncCallExpr {
                inner: expr,
                metric: metric.clone(),
            }));
        }
        let metric_name = format!("coverage::function::{}", fun.name());
        let metric = Metric::new(
            MetricValue::Count {
                name: std::borrow::Cow::Owned(metric_name),
                count: datafusion::physical_plan::metrics::Count::new(),
            },
            None, // Use None to track across all partitions
        );

        let metric_arc = Arc::new(metric);
        seen_functions.insert(fun.name().to_string(), metric_arc.clone());
        metrics.register(metric_arc.clone());
        return Ok(Arc::new(CoveredFuncCallExpr {
            inner: expr,
            metric: metric_arc,
        }));
    }

    Ok(expr)
}

/// Recursively replace every CaseExpr with branch coverage tracking
fn wrap_case_when_expr(
    expr: Arc<dyn PhysicalExpr>,
    metrics: &ExecutionPlanMetricsSet,
    current_case: &mut usize,
) -> Result<Arc<dyn PhysicalExpr>> {
    // (1) children first
    let new_children = expr
        .children()
        .into_iter()
        .map(|c| wrap_case_when_expr(Arc::clone(c), metrics, current_case))
        .collect::<Result<Vec<_>>>()?;
    let expr = expr.with_new_children(new_children)?;

    // (2) is this node a CASE expression?
    if let Some(case_expr) = expr.as_any().downcast_ref::<CaseExpr>() {
        let case_id = *current_case;
        *current_case += 1;

        // Create metrics for each WHEN condition and wrap them
        let mut when_metrics = Vec::new();
        let mut new_when_then_expr = Vec::new();

        for (when_idx, (when_cond, then_val)) in case_expr.when_then_expr().iter().enumerate() {
            // Create metric for this WHEN condition
            let metric_name = format!("coverage::case::cs_{case_id}__when_{when_idx}");
            let metric = Metric::new(
                MetricValue::Count {
                    name: std::borrow::Cow::Owned(metric_name),
                    count: datafusion::physical_plan::metrics::Count::new(),
                },
                None,
            );
            let metric_arc = Arc::new(metric);
            metrics.register(metric_arc.clone());
            when_metrics.push(metric_arc.clone());

            // Wrap the WHEN condition with coverage tracking
            let wrapped_when_cond = Arc::new(CaseWhenConditionExpr {
                inner: when_cond.clone(),
                metric: metric_arc,
            }) as Arc<dyn PhysicalExpr>;

            // Use the already-processed THEN value (then_val is already processed)
            new_when_then_expr.push((wrapped_when_cond, then_val.clone()));
        }

        // Use the already-processed ELSE expression (already processed via children)
        let new_else_expr = case_expr.else_expr().cloned();

        // Create new CaseExpr with wrapped conditions
        let new_case_expr = Arc::new(CaseExpr::try_new(
            case_expr.expr().cloned(),
            new_when_then_expr,
            new_else_expr.clone(),
        )?);

        // Always track ELSE coverage, even if there's no explicit ELSE clause
        // (implicit ELSE returns NULL for unmatched rows)
        let else_metric_name = format!("coverage::case::cs_{case_id}__else");
        let else_metric = Metric::new(
            MetricValue::Count {
                name: std::borrow::Cow::Owned(else_metric_name),
                count: datafusion::physical_plan::metrics::Count::new(),
            },
            None,
        );
        let else_metric_arc = Arc::new(else_metric);
        metrics.register(else_metric_arc.clone());

        return Ok(Arc::new(CaseElseTracker {
            inner: new_case_expr,
            when_metrics,
            else_metric: else_metric_arc,
            initial_when_counts: Vec::new(),
        }) as Arc<dyn PhysicalExpr>);
    }

    Ok(expr)
}

macro_rules! is_node_type_any_of {
    ($plan:expr, $($t:ty),* $(,)?) => {
        $(
            $plan.as_any().downcast_ref::<$t>().is_some() ||
        )*
        false
    };
}

/// Generic function to transform expressions in a physical plan using a closure
pub(crate) fn transform_plan_expressions<F>(
    plan: Arc<dyn ExecutionPlan>,
    transform_expr: &mut F,
) -> Result<Arc<dyn ExecutionPlan>>
where
    F: FnMut(Arc<dyn PhysicalExpr>) -> Result<Arc<dyn PhysicalExpr>>,
{
    use datafusion::physical_expr::aggregate::AggregateExprBuilder;
    use datafusion::physical_plan::{
        aggregates::{AggregateExec, PhysicalGroupBy},
        empty::EmptyExec,
        filter::{FilterExec, FilterExecBuilder},
        joins::NestedLoopJoinExec,
        projection::ProjectionExec,
        sorts::sort::SortExec,
        windows::WindowAggExec,
    };

    // dbg!(&plan);
    // dbg!(&plan.children());

    // 1. rewrite children first
    let new_inputs = plan
        .children()
        .into_iter()
        .map(|child| transform_plan_expressions(Arc::clone(child), transform_expr))
        .collect::<Result<Vec<_>>>()?;
    let plan = plan.with_new_children(new_inputs)?;

    // 2. replace expressions based on node type
    if let Some(proj) = plan.as_any().downcast_ref::<ProjectionExec>() {
        let new_exprs = proj
            .expr()
            .iter()
            .map(|e| Ok((transform_expr(e.expr.clone())?, e.alias.clone())))
            .collect::<Result<Vec<_>>>()?;
        return Ok(Arc::new(ProjectionExec::try_new(
            new_exprs,
            plan.children()[0].clone(),
        )?));
    }

    if let Some(filt) = plan.as_any().downcast_ref::<FilterExec>() {
        let new_pred = transform_expr(filt.predicate().clone())?;
        return Ok(Arc::new(
            FilterExecBuilder::from(filt)
                .with_predicate(new_pred)
                .with_input(plan.children()[0].clone())
                .build()?,
        ));
    }

    if let Some(_window) = plan.as_any().downcast_ref::<WindowAggExec>() {
        todo!("WindowAggExec coverage collection")
    }

    if let Some(sort) = plan.as_any().downcast_ref::<SortExec>() {
        let new_sort_exprs = sort
            .expr()
            .iter()
            .map(|e| {
                Ok(PhysicalSortExpr {
                    expr: transform_expr(e.expr.clone())?,
                    options: e.options,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        return Ok(Arc::new(
            SortExec::new(
                LexOrdering::new(new_sort_exprs).expect("sort is non-empty"),
                plan.children()[0].clone(),
            )
            .with_preserve_partitioning(sort.preserve_partitioning())
            .with_fetch(sort.fetch()),
        ));
    }

    // Handle aggregate exec: transform group, aggregate inputs and filters
    if let Some(agg) = plan.as_any().downcast_ref::<AggregateExec>() {
        // Transform group-by expressions
        let new_group_pairs = agg
            .group_expr()
            .expr()
            .iter()
            .map(|(e, name)| Ok((transform_expr(e.clone())?, name.clone())))
            .collect::<Result<Vec<_>>>()?;

        // Rebuild PhysicalGroupBy preserving null_expr and groups
        let new_group = PhysicalGroupBy::new(
            new_group_pairs,
            agg.group_expr().null_expr().to_vec(),
            agg.group_expr().groups().to_vec(),
            agg.group_expr().has_grouping_set(),
        );

        // Transform each aggregate expression's input children by rebuilding via builder
        let input_schema = agg.input_schema();
        // not sure how remove redunancy from the `transform_expr` closure below
        #[allow(clippy::redundant_closure)]
        let new_aggr_exprs = agg
            .aggr_expr()
            .iter()
            .map(|ae| {
                let fun = Arc::new(ae.fun().clone());
                let new_args = ae
                    .expressions()
                    .into_iter()
                    .map(|c| transform_expr(c))
                    .collect::<Result<Vec<_>>>()?;
                let mut builder = AggregateExprBuilder::new(fun, new_args)
                    .schema(Arc::clone(&input_schema))
                    .alias(ae.name().to_string());
                if !ae.order_bys().is_empty() {
                    builder = builder.order_by(ae.order_bys().to_vec());
                }
                if ae.ignore_nulls() {
                    builder = builder.with_ignore_nulls(true);
                }
                if ae.is_distinct() {
                    builder = builder.with_distinct(true);
                }
                if ae.is_reversed() {
                    builder = builder.with_reversed(true);
                }
                Ok(Arc::new(builder.build()?))
            })
            .collect::<Result<Vec<_>>>()?;

        // Transform FILTER expressions per aggregate
        let new_filter_exprs = agg
            .filter_expr()
            .iter()
            .map(|opt| match opt {
                Some(e) => Ok(Some(transform_expr(e.clone())?)),
                None => Ok(None),
            })
            .collect::<Result<Vec<_>>>()?;

        return Ok(Arc::new(AggregateExec::try_new(
            *agg.mode(),
            new_group,
            new_aggr_exprs,
            new_filter_exprs,
            plan.children()[0].clone(),
            input_schema,
        )?));
    }

    if let Some(join) = plan.as_any().downcast_ref::<NestedLoopJoinExec>()
        && let Some(filter) = join.filter()
    {
        let new_filter = transform_expr(filter.expression().clone())?;
        let new_filter = JoinFilter::new(
            new_filter,
            filter.column_indices().to_vec(),
            filter.schema().clone(),
        );
        return Ok(Arc::new(NestedLoopJoinExec::try_new(
            plan.children()[0].clone(),
            plan.children()[1].clone(),
            Some(new_filter),
            join.join_type(),
            join.projection().as_deref().map(<[usize]>::to_vec),
        )?));
    }

    if let Some(join) = plan.as_any().downcast_ref::<HashJoinExec>() {
        let new_filter = if let Some(filter) = join.filter() {
            let new_filter = transform_expr(filter.expression().clone())?;
            Some(JoinFilter::new(
                new_filter,
                filter.column_indices().to_vec(),
                filter.schema().clone(),
            ))
        } else {
            None
        };

        let new_on = join
            .on()
            .iter()
            .map(|(e1, e2)| Ok((transform_expr(e1.clone())?, transform_expr(e2.clone())?)))
            .collect::<Result<Vec<_>>>()?;
        return Ok(Arc::new(HashJoinExec::try_new(
            plan.children()[0].clone(),
            plan.children()[1].clone(),
            new_on,
            new_filter,
            join.join_type(),
            join.projection.as_deref().map(<[usize]>::to_vec),
            *join.partition_mode(),
            join.null_equality,
            join.null_aware,
        )?));
    }

    // These are ok to skip because they do not carry expressions
    if is_node_type_any_of!(
        plan,
        InterleaveExec,
        PlaceholderRowExec,
        UnionExec,
        EmptyExec,
        DataSourceExec,
        NamedMemoryExec,
        CoalescePartitionsExec,
        RepartitionExec,
        UnnestExec,
        BoundedWindowAggExec, // TODO handle WindowExpr
        NestedLoopJoinExec,   // handled above
        CrossJoinExec,        // doesn't carry expressions
    ) {
        return Ok(plan);
    }

    // all other nodes are not yet implemented
    Err(DataFusionError::Execution(format!(
        "Unsupported node kind: {plan:#?}",
    )))
}

/// Walk the plan tree and rewrite every expression-carrying node.
/// TODO: fill in all the missing nodes
fn wrap_function_coverage_metrics(
    plan: Arc<dyn ExecutionPlan>,
    metrics: &ExecutionPlanMetricsSet,
    seen_functions: &mut HashMap<String, Arc<Metric>>,
) -> Result<Arc<dyn ExecutionPlan>> {
    let mut transform_expr =
        |expr: Arc<dyn PhysicalExpr>| wrap_function_call_expr(expr, metrics, seen_functions);

    transform_plan_expressions(plan, &mut transform_expr)
}

/// Wraps a plan with CASE WHEN coverage tracking
fn wrap_case_when_coverage_metrics(
    plan: Arc<dyn ExecutionPlan>,
    metrics: &ExecutionPlanMetricsSet,
    current_case: &mut usize,
) -> Result<Arc<dyn ExecutionPlan>> {
    let mut transform_expr =
        |expr: Arc<dyn PhysicalExpr>| wrap_case_when_expr(expr, metrics, current_case);

    transform_plan_expressions(plan, &mut transform_expr)
}

#[derive(Debug)]
struct BranchPredicateExpr {
    inner: Arc<dyn PhysicalExpr>,
    c_true: Arc<Metric>, // Row counting: number of rows that evaluated to true
}

impl fmt::Display for BranchPredicateExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl Hash for BranchPredicateExpr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.hash(state);
    }
}

impl PartialEq for BranchPredicateExpr {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for BranchPredicateExpr {}

impl PhysicalExpr for BranchPredicateExpr {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn data_type(&self, schema: &Schema) -> Result<DataType> {
        self.inner.data_type(schema)
    }

    fn nullable(&self, schema: &Schema) -> Result<bool> {
        self.inner.nullable(schema)
    }

    fn evaluate(&self, batch: &RecordBatch) -> Result<ColumnarValue> {
        let cv = self.inner.evaluate(batch)?; // delegate

        // Convert ColumnarValue → BooleanArray
        let arr: ArrayRef = match cv.clone() {
            ColumnarValue::Array(a) => a,
            ColumnarValue::Scalar(sv) => {
                // replicate the scalar to match batch size
                Arc::new(sv.to_array_of_size(batch.num_rows())?)
            }
        };

        let bool_arr = arr.as_any().downcast_ref::<BooleanArray>().ok_or_else(|| {
            DataFusionError::Execution("Predicate did not evaluate to BooleanArray".into())
        })?;

        // Count rows that evaluated to true
        let mut n_true = 0;
        for i in 0..bool_arr.len() {
            if bool_arr.is_valid(i) && bool_arr.value(i) {
                n_true += 1;
            }
        }

        // bump row counting metric
        if let MetricValue::Count { count, .. } = &self.c_true.value() {
            count.add(n_true);
        }

        // return the *same* predicate value so FilterExec can use it
        Ok(cv)
    }

    fn children(&self) -> Vec<&Arc<dyn PhysicalExpr>> {
        vec![&self.inner]
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn PhysicalExpr>>,
    ) -> Result<Arc<dyn PhysicalExpr>> {
        let [inner] = <[_; 1]>::try_from(children).map_err(|children| {
            DataFusionError::Internal(format!("Expected 1 child, got {}", children.len()))
        })?;
        Ok(Arc::new(Self {
            inner,
            c_true: self.c_true.clone(),
        }))
    }

    fn fmt_sql(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt_sql(f)
    }
}

/// Wraps a filter plan with coverage tracking
fn wrap_branch_coverage_metrics(
    plan: Arc<dyn ExecutionPlan>,
    metrics: &ExecutionPlanMetricsSet,
    current_branch: &mut usize,
) -> Result<Arc<dyn ExecutionPlan>> {
    // 1) recurse first
    let new_inputs = plan
        .children()
        .into_iter()
        .map(|c| wrap_branch_coverage_metrics(Arc::clone(c), metrics, current_branch))
        .collect::<Result<Vec<_>>>()?;
    let plan = plan.with_new_children(new_inputs)?;

    // 2) is this FilterExec?
    if let Some(f) = plan.as_any().downcast_ref::<FilterExec>() {
        // one metric family per filter (unique pointer identity is enough)
        let id = format!("br_{current_branch}");
        *current_branch += 1;

        // Row counting metric: number of rows that evaluated to true
        let c_true = Metric::new(
            MetricValue::Count {
                name: std::borrow::Cow::Owned(format!("coverage::branch::{id}__true")),
                count: datafusion::physical_plan::metrics::Count::new(),
            },
            None,
        );
        let c_true_arc = Arc::new(c_true);
        metrics.register(c_true_arc.clone());

        let pred = Arc::new(BranchPredicateExpr {
            inner: f.predicate().clone(),
            c_true: c_true_arc,
        });

        return Ok(Arc::new(
            FilterExecBuilder::from(f)
                .with_predicate(pred)
                .with_input(plan.children()[0].clone())
                .build()?,
        ));
    }

    // unlike wrap_function_coverage_metrics, we don't need to error out
    // since we are interested in filters only
    Ok(plan)
}

#[derive(Debug)]
struct JoinPredicateExpr {
    inner: Arc<dyn PhysicalExpr>,
    c_match: Arc<Metric>, // Row counting: number of rows that successfully matched
}

impl fmt::Display for JoinPredicateExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl Hash for JoinPredicateExpr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.hash(state);
    }
}

impl PartialEq for JoinPredicateExpr {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for JoinPredicateExpr {}

impl PhysicalExpr for JoinPredicateExpr {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn data_type(&self, schema: &Schema) -> Result<DataType> {
        self.inner.data_type(schema)
    }

    fn nullable(&self, schema: &Schema) -> Result<bool> {
        self.inner.nullable(schema)
    }

    fn evaluate(&self, batch: &RecordBatch) -> Result<ColumnarValue> {
        let cv = self.inner.evaluate(batch)?; // delegate

        // Convert ColumnarValue → BooleanArray
        let arr: ArrayRef = match cv.clone() {
            ColumnarValue::Array(a) => a,
            ColumnarValue::Scalar(sv) => {
                // replicate the scalar to match batch size
                Arc::new(sv.to_array_of_size(batch.num_rows())?)
            }
        };

        let bool_arr = arr.as_any().downcast_ref::<BooleanArray>().ok_or_else(|| {
            DataFusionError::Execution("Join predicate did not evaluate to BooleanArray".into())
        })?;

        // Count rows that successfully matched
        let mut n_match = 0;
        for i in 0..bool_arr.len() {
            if bool_arr.is_valid(i) && bool_arr.value(i) {
                n_match += 1;
            }
        }

        // bump counter for successful matches
        if let MetricValue::Count { count, .. } = &self.c_match.value() {
            count.add(n_match);
        }

        // return the *same* predicate value so JoinExec can use it
        Ok(cv)
    }

    fn children(&self) -> Vec<&Arc<dyn PhysicalExpr>> {
        vec![&self.inner]
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn PhysicalExpr>>,
    ) -> Result<Arc<dyn PhysicalExpr>> {
        let [inner] = <[_; 1]>::try_from(children).map_err(|children| {
            DataFusionError::Internal(format!("Expected 1 child, got {}", children.len()))
        })?;
        Ok(Arc::new(Self {
            inner,
            c_match: self.c_match.clone(),
        }))
    }

    fn fmt_sql(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt_sql(f)
    }
}

/// Wraps a join plan with coverage tracking
/// TODO: fill in all the other join node kinds
fn wrap_join_coverage_metrics(
    plan: Arc<dyn ExecutionPlan>,
    metrics: &ExecutionPlanMetricsSet,
    current_join: &mut usize,
) -> Result<Arc<dyn ExecutionPlan>> {
    // 1) recurse first
    let new_inputs = plan
        .children()
        .into_iter()
        .map(|c| wrap_join_coverage_metrics(Arc::clone(c), metrics, current_join))
        .collect::<Result<Vec<_>>>()?;
    let plan = plan.with_new_children(new_inputs)?;

    // 2) is this NestedLoopJoinExec?
    if let Some(j) = plan.as_any().downcast_ref::<NestedLoopJoinExec>()
        && let Some(filter) = j.filter()
    {
        // one metric family per join (unique pointer identity is enough)
        let id = format!("jn_{current_join}");
        *current_join += 1;

        // Row counting metric: number of rows that successfully matched
        let c_match = Metric::new(
            MetricValue::Count {
                name: std::borrow::Cow::Owned(format!("coverage::join::{id}__match")),
                count: datafusion::physical_plan::metrics::Count::new(),
            },
            None,
        );
        let c_match_arc = Arc::new(c_match);
        metrics.register(c_match_arc.clone());

        let pred = Arc::new(JoinPredicateExpr {
            inner: filter.expression().clone(),
            c_match: c_match_arc,
        });

        // Create a new join filter with our wrapped predicate
        let new_filter = JoinFilter::new(
            pred,
            filter.column_indices().to_vec(),
            filter.schema().clone(),
        );

        // Create a new join exec with our wrapped filter
        return {
            Ok(Arc::new(NestedLoopJoinExec::try_new(
                plan.children()[0].clone(),
                plan.children()[1].clone(),
                Some(new_filter),
                j.join_type(),
                j.projection().as_deref().map(<[usize]>::to_vec),
            )?))
        };
    }

    // 3) is this HashJoinExec?
    if let Some(j) = plan.as_any().downcast_ref::<HashJoinExec>() {
        // one metric family per join (unique pointer identity is enough)
        let id = format!("jn_{current_join}");
        *current_join += 1;

        // Row counting metric: number of rows that successfully matched
        let c_match = Metric::new(
            MetricValue::Count {
                name: std::borrow::Cow::Owned(format!("coverage::join::{id}__match")),
                count: datafusion::physical_plan::metrics::Count::new(),
            },
            None,
        );
        let c_match_arc = Arc::new(c_match);
        metrics.register(c_match_arc.clone());

        // Handle filter - create vacuous "true" filter if None
        let filter_expr = if let Some(filter) = j.filter() {
            filter.expression().clone()
        } else {
            // Create a vacuous "true" expression
            Arc::new(datafusion::physical_expr::expressions::Literal::new(
                ScalarValue::Boolean(Some(true)),
            )) as Arc<dyn PhysicalExpr>
        };

        let pred = Arc::new(JoinPredicateExpr {
            inner: filter_expr,
            c_match: c_match_arc,
        });

        // Create a new join filter with our wrapped predicate
        let new_filter = JoinFilter::new(
            pred,
            j.filter()
                .map(|f| f.column_indices().to_vec())
                .unwrap_or_default(),
            j.filter()
                .map(|f| f.schema().clone())
                .unwrap_or_else(|| Arc::new(Schema::new(Fields::empty()))), // Empty schema for vacuous filter
        );

        // Create a new join exec with our wrapped filter
        return Ok(Arc::new(HashJoinExec::try_new(
            plan.children()[0].clone(),
            plan.children()[1].clone(),
            j.on().to_vec(),
            Some(new_filter),
            j.join_type(),
            j.projection.as_deref().map(<[usize]>::to_vec),
            *j.partition_mode(),
            j.null_equality,
            j.null_aware,
        )?));
    }

    // 4) is this CrossJoinExec? No additional metrics; children already wrapped
    if plan.as_any().downcast_ref::<CrossJoinExec>().is_some() {
        return Ok(plan);
    }

    // TODO: implement coverage tracking for these join nodes
    if is_node_type_any_of!(plan, SortMergeJoinExec, SymmetricHashJoinExec) {
        return Err(DataFusionError::Execution(format!(
            "Unsupported join node kind: {plan:#?}",
        )));
    }

    // unlike wrap_function_coverage_metrics, we don't need to error out
    // since we are interested in join nodes only
    Ok(plan)
}

pub fn wrap_coverage_metrics(
    plan: Arc<dyn ExecutionPlan>,
    metrics: &ExecutionPlanMetricsSet,
) -> Result<Arc<dyn ExecutionPlan>> {
    let plan = wrap_function_coverage_metrics(plan, metrics, &mut HashMap::new())?;
    let plan = wrap_case_when_coverage_metrics(plan, metrics, &mut 0)?;
    let plan = wrap_branch_coverage_metrics(plan, metrics, &mut 0)?;
    let plan = wrap_join_coverage_metrics(plan, metrics, &mut 0)?;
    Ok(plan)
}
