use super::*;
use adbc_core::error::Result as AdbcResult;
use adbc_core::options::OptionStatement;
use arrow_array::{RecordBatchIterator, RecordBatchReader};
use dbt_adbc::Statement;
use serde_json::json;
use std::sync::Mutex;

#[derive(Default)]
struct CapturedUpdate {
    options: BTreeMap<String, String>,
    executions: usize,
}

#[derive(Clone)]
struct CapturingConnection(Arc<Mutex<CapturedUpdate>>);

impl Connection for CapturingConnection {
    fn new_statement(&mut self) -> AdbcResult<Box<dyn Statement>> {
        Ok(Box::new(self.clone()))
    }

    fn cancel(&mut self) -> AdbcResult<()> {
        unimplemented!()
    }

    fn commit(&mut self) -> AdbcResult<()> {
        unimplemented!()
    }

    fn rollback(&mut self) -> AdbcResult<()> {
        unimplemented!()
    }
}

impl Statement for CapturingConnection {
    fn set_option(&mut self, key: OptionStatement, value: OptionValue) -> AdbcResult<()> {
        if let (OptionStatement::Other(key), OptionValue::String(value)) = (key, value) {
            self.0.lock().unwrap().options.insert(key, value);
        }
        Ok(())
    }

    fn set_sql_query(&mut self, _sql: &str) -> AdbcResult<()> {
        Ok(())
    }

    fn execute(&mut self) -> AdbcResult<Box<dyn RecordBatchReader + Send + '_>> {
        self.0.lock().unwrap().executions += 1;
        Ok(Box::new(RecordBatchIterator::new(
            std::iter::empty(),
            Arc::new(Schema::empty()),
        )))
    }

    fn bind(&mut self, _batch: RecordBatch) -> AdbcResult<()> {
        unimplemented!()
    }

    fn bind_stream(&mut self, _reader: Box<dyn RecordBatchReader + Send>) -> AdbcResult<()> {
        unimplemented!()
    }

    fn execute_update(&mut self) -> AdbcResult<Option<i64>> {
        unimplemented!()
    }

    fn execute_schema(&mut self) -> AdbcResult<Schema> {
        unimplemented!()
    }

    fn execute_partitions(&mut self) -> AdbcResult<adbc_core::PartitionedResult> {
        unimplemented!()
    }

    fn get_parameter_schema(&self) -> AdbcResult<Schema> {
        unimplemented!()
    }

    fn prepare(&mut self) -> AdbcResult<()> {
        unimplemented!()
    }

    fn set_substrait_plan(&mut self, _plan: &[u8]) -> AdbcResult<()> {
        unimplemented!()
    }

    fn cancel(&mut self) -> AdbcResult<()> {
        unimplemented!()
    }
}

fn update_columns(columns: serde_json::Value) -> CapturedUpdate {
    let adapter = AdapterImpl::new(engine(Bigquery), None);
    let env = Environment::new();
    let state = State::new_for_env(&env);
    let relation: Arc<dyn BaseRelation> = Arc::new(Relation::new(
        Bigquery,
        "project".to_string(),
        "dataset".to_string(),
        "table".to_string(),
    ));
    let captured = Arc::new(Mutex::new(CapturedUpdate::default()));
    let mut conn = CapturingConnection(captured.clone());
    let columns = serde_json::from_value(columns).unwrap();
    adapter
        .update_columns_descriptions(
            &state,
            &mut conn,
            &relation,
            columns,
            CancellationToken::never_cancels(),
        )
        .unwrap();
    std::mem::take(&mut *captured.lock().unwrap())
}

fn assert_update(
    columns: serde_json::Value,
    descriptions: serde_json::Value,
    policy_tags: serde_json::Value,
) {
    let captured = update_columns(columns);
    assert_eq!(captured.executions, 1);
    assert_eq!(
        captured.options[QUERY_DESTINATION_TABLE],
        "project.dataset.table"
    );
    for (key, expected) in [
        (UPDATE_TABLE_COLUMNS_DESCRIPTION, descriptions),
        (UPDATE_TABLE_COLUMNS_POLICY_TAGS, policy_tags),
    ] {
        let actual: serde_json::Value = serde_json::from_str(&captured.options[key]).unwrap();
        assert_eq!(actual, expected, "{key}");
    }
}

#[test]
fn nested_descriptions_without_documented_parent() {
    assert_update(
        json!({
            "customer.email": {"name": "customer.email", "description": "Email address"},
            "customer.address.city": {"name": "customer.address.city", "description": "City"}
        }),
        json!({"customer.email": "Email address", "customer.address.city": "City"}),
        json!({}),
    );
}

#[test]
fn nested_policy_tags_without_descriptions() {
    let tag = "projects/project/locations/us/taxonomies/taxonomy/policyTags/email";
    assert_update(
        json!({
            "customer.email": {
                "name": "customer.email", "policy_tags": [tag, {"masking_policy": "mask_email"}]
            }
        }),
        json!({}),
        json!({"customer.email": [tag]}),
    );
}

#[test]
fn parent_and_nested_metadata_are_preserved() {
    let tag = "projects/project/locations/us/taxonomies/taxonomy/policyTags/email";
    assert_update(
        json!({
            "id": {"name": "id", "description": "Identifier", "policy_tags": [tag]},
            "customer": {"name": "customer", "description": "Customer"},
            "customer.email": {
                "name": "customer.email", "description": "Email address", "policy_tags": [tag]
            },
            "items": {"name": "items", "data_type": "array"},
            "items.details": {"name": "items.details", "description": "Details"},
            "items.details.sku": {"name": "items.details.sku", "description": ""},
            "undocumented": {"name": "undocumented"}
        }),
        json!({
            "id": "Identifier", "customer": "Customer", "customer.email": "Email address",
            "items.details": "Details", "items.details.sku": ""
        }),
        json!({"id": [tag], "customer.email": [tag]}),
    );
}

#[test]
fn no_metadata_skips_execution() {
    for columns in [
        json!({}),
        json!({
            "customer.email": {"name": "customer.email"},
            "customer.phone": {"name": "customer.phone", "policy_tags": []},
            "customer.address": {
                "name": "customer.address", "policy_tags": [{"masking_policy": "mask_address"}]
            }
        }),
    ] {
        let captured = update_columns(columns);
        assert_eq!(captured.executions, 0);
        assert!(captured.options.is_empty());
    }
}
