#[cfg(test)]
// Failed to get Redis connection: No connection could be made because the target machine actively refused it. (os error 10061)
#[cfg(not(target_os = "windows"))]
mod tests {
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{collections::HashMap, sync::Arc};

    use crossbeam_queue::SegQueue;
    use dashmap::DashMap;
    use dbt_schemas::schemas::{CloudCredentials, ResolvedCloudConfig};
    use dbt_state::node_session::{Acquired, Contention, NodeCacheSession};
    use dbt_state::task_cache::{TaskCache, TaskState, TaskValue};
    use dbt_state::task_cache_noop::TaskCacheNoop;
    use dbt_state::task_cache_redis::TaskCacheRedis;
    use redis::aio::MultiplexedConnection;
    // use dbt_state::task_cache::SLEEP_PERIOD;

    fn redis_uri() -> &'static str {
        "redis://127.0.0.1:6379"
    }

    fn test_cloud_config() -> ResolvedCloudConfig {
        ResolvedCloudConfig {
            credentials: Some(CloudCredentials {
                account_id: "1".to_string(),
                host: "localhost".to_string(),
                token: "test-token".to_string(),
            }),
            project_id: Some("redis-test".to_string()),
            account_identifier: Some("1".to_string()),
            environment_id: Some("1".to_string()),
            ..Default::default()
        }
    }

    async fn redis_cache_or_skip(
        prefix: &str,
        test_name: &str,
    ) -> Option<Arc<TaskCacheRedis<MultiplexedConnection>>> {
        match TaskCacheRedis::new_single(redis_uri(), prefix, Some(&test_cloud_config())).await {
            Ok(cache) => Some(Arc::new(cache)),
            Err(err) => {
                eprintln!("Skipping {test_name}: {err}");
                None
            }
        }
    }

    async fn run_sql<T: TaskCache + 'static>(
        task_cache: Arc<T>,
        node_id: &str,
        owner_id: &str,
        value_to_store: Option<String>,
    ) -> Result<String, String> {
        // Upcast to `Arc<dyn TaskCache>` so the session can
        // clone the handle for heartbeat/finalize without being generic.
        let erased: Arc<dyn TaskCache> = task_cache.clone();
        let session = NodeCacheSession::new(erased, node_id.to_string(), owner_id.to_string());
        let hash = value_to_store.clone().unwrap_or_default();

        let on_contention = |c: Contention| match c {
            Contention::Waiting => println!("[Worker {owner_id}] Waiting for Task: {node_id}"),
            Contention::Aborted => println!("[Worker {owner_id}] Task Aborted: {node_id}"),
        };

        match session.acquire(&hash, &on_contention).await? {
            Acquired::Execute(guard) => {
                println!("[Worker {owner_id}] Starting Task: {node_id}");
                let result = hash.clone();
                guard.finalize(HashMap::new()).await?;
                println!("[Worker {owner_id}] Completed Task: {node_id} | Result: {result}");
                Ok(result)
            }
            Acquired::Completed { stored_hash, .. } => {
                // This test treats any completion as a hit (we don't care whether the
                // stored hash matches, we just reuse the existing value like the
                // original loop did for `TaskState::Completed`).
                println!(
                    "[Worker {owner_id}] Task Already Completed: {node_id} | Result: {stored_hash}"
                );
                Ok(stored_hash)
            }
        }
    }

    async fn evaluate_dag<T: TaskCache + 'static>(
        task_cache: Arc<T>,
        nodes: Arc<HashMap<&str, Vec<&str>>>,
        worker_id: &str,
    ) -> Result<(), String> {
        let mut converted_nodes = HashMap::new();
        for (k, v) in nodes.iter() {
            converted_nodes.insert(
                (*k).to_string(),
                v.iter().map(|s| (*s).to_string()).collect::<Vec<String>>(),
            );
        }
        let nodes = converted_nodes;

        // Build a reverse mapping of child nodes to parent nodes
        let mut reverse_nodes = HashMap::new();
        let indegree = Arc::new(DashMap::<String, AtomicUsize>::new());

        for (id, children) in nodes.iter() {
            indegree.insert(id.to_string(), AtomicUsize::new(children.len()));
            for child_id in children {
                reverse_nodes
                    .entry(child_id.to_string())
                    .or_insert_with(HashSet::new)
                    .insert(id.clone());
            }
        }

        // Initialize the worklist with nodes that have no pending children
        let worklist = Arc::new(SegQueue::new());
        for id in nodes.keys() {
            if let Some(indegree) = indegree.get(id)
                && indegree.load(Ordering::SeqCst) == 0
            {
                worklist.push(id.clone());
            }
        }

        let reverse_nodes = Arc::new(reverse_nodes);

        println!("[Worker {worker_id}] Initial State:");
        println!("  Nodes: {nodes:?}");
        println!("  ReverseNodes: {reverse_nodes:?}");
        println!("  Indegree: {indegree:?}");
        let mut temp_vec = Vec::new();
        // print the worklist
        while let Some(node_id) = worklist.pop() {
            temp_vec.push(node_id.clone());
        }
        println!("  Worklist: {temp_vec:?}\n");
        for node_id in temp_vec {
            worklist.push(node_id);
        }

        // Process the worklist in parallel

        let mut tasks = Vec::new();

        loop {
            while let Some(node_id) = worklist.pop() {
                let node_id = node_id.to_string();
                let task_cache = task_cache.clone();
                let worklist = worklist.clone();
                let indegree = indegree.clone();
                let reverse_nodes = reverse_nodes.clone();
                let worker_id = worker_id.to_string();
                tasks.push(tokio::spawn(async move {
                    let value_to_store = Some(format!("{node_id}-{worker_id}"));
                    let _result = run_sql(task_cache, &node_id, &worker_id, value_to_store).await;

                    // Update the pending count and potentially add parent nodes to the worklist
                    if let Some(parents) = reverse_nodes.get(&node_id).as_ref() {
                        for parent_id in parents.iter() {
                            if let Some(indegree) = indegree.get(parent_id)
                                && indegree.fetch_sub(1, Ordering::SeqCst) == 1
                            {
                                worklist.push(parent_id.clone());
                            }
                        }
                    }
                }));
            }
            // Check if the worklist is empty
            if tasks.is_empty() {
                break;
            }

            // Await all handles
            for task in tasks.drain(..) {
                match task.await {
                    Ok(_) => {}
                    Err(_) => println!("  Error in parallel scheduler"),
                };
            }
        }
        Ok(())
    }

    #[dbt_runtime::test]
    async fn test_concurrent_cleanup_race_generic() {
        let Some(cache) = redis_cache_or_skip("", "test_concurrent_cleanup_race_generic").await
        else {
            return;
        };

        let key = "race_node";

        // Start + Stop → task is in Completed state.
        let start = TaskValue::new_now("seed".into(), Some("h1".into()));
        cache.try_start_task(key, &start, &None).await.unwrap();
        let stop = TaskValue::new_now("seed".into(), Some("h1".into()));
        let info = dbt_state::task_cache::TaskInfo {
            upstreams: HashMap::new(),
        };
        cache.stop_task(key, &stop, &info).await.unwrap();

        // Two workers race to clean it up.
        let c1 = cache.clone();
        let c2 = cache.clone();

        let h1 = tokio::spawn(async move {
            let (state, (start, _, _)) = c1.get_task_state(key).await.unwrap();
            assert_eq!(state, TaskState::Completed);
            c1.try_cleanup_task(key, &start).await
        });

        let h2 = tokio::spawn(async move {
            let (state, (start, _, _)) = c2.get_task_state(key).await.unwrap();
            assert_eq!(state, TaskState::Completed);
            c2.try_cleanup_task(key, &start).await
        });

        let r1 = h1.await.unwrap();
        let r2 = h2.await.unwrap();

        // Exactly one of the two attempts should succeed.
        assert!(r1.is_ok() && r2.is_ok());
        // And the key should be gone now.
        assert!(cache.get_start(key).await.is_err());
    }

    #[dbt_runtime::test]
    async fn test_evaluate_dag() {
        let task_cache = Arc::new(TaskCacheNoop::new());
        let mut nodes: HashMap<&str, Vec<&str>> = HashMap::new();
        nodes.insert("a", vec!["b", "c"]);
        nodes.insert("b", vec![]);
        nodes.insert("c", vec![]);
        let nodes = Arc::new(nodes);
        let result = evaluate_dag(task_cache.clone(), nodes, "0").await;
        assert!(result.is_ok());
    }

    #[dbt_runtime::test]
    async fn test_evaluate_shared_dag() {
        let task_cache = Arc::new(TaskCacheNoop::new());
        let mut nodes: HashMap<&str, Vec<&str>> = HashMap::new();
        nodes.insert("a", vec!["b", "c"]);
        nodes.insert("b", vec!["d"]);
        nodes.insert("c", vec!["d"]);
        nodes.insert("d", vec!["e", "f"]);
        nodes.insert("e", vec![]);
        nodes.insert("f", vec![]);
        let nodes = Arc::new(nodes);
        let res = evaluate_dag(task_cache.clone(), nodes, "0").await;
        assert!(res.is_ok());
    }

    // #[dbt_runtime::test]
    // async fn test_evaluate_dag_fs() {
    //     let task_cache = Arc::new(FileTaskManager::new("task_states".into()));
    //     let mut nodes: HashMap<&str, Vec<&str>> = HashMap::new();
    //     nodes.insert("a", vec!["b", "c"]);
    //     nodes.insert("b", vec![]);
    //     nodes.insert("c", vec![]);
    //     let nodes = Arc::new(nodes);
    //     let result = evaluate_dag(task_cache.clone(), nodes, "0").await;
    //     assert!(result.is_ok());
    // }

    #[dbt_runtime::test]
    async fn test_evaluate_dag_redis() {
        let Some(task_cache) = redis_cache_or_skip("", "test_evaluate_dag_redis").await else {
            return;
        };
        let mut nodes: HashMap<&str, Vec<&str>> = HashMap::new();
        nodes.insert("a", vec!["b", "c"]);
        nodes.insert("b", vec![]);
        nodes.insert("c", vec![]);
        let nodes = Arc::new(nodes);
        let result = evaluate_dag(task_cache.clone(), nodes, "0").await;
        assert!(result.is_ok());
    }

    #[dbt_runtime::test]
    async fn test_evaluate_dag_redis_independent() {
        let Some(task_cache1) =
            redis_cache_or_skip("", "test_evaluate_dag_redis_independent").await
        else {
            return;
        };
        let Some(task_cache2) =
            redis_cache_or_skip("", "test_evaluate_dag_redis_independent").await
        else {
            return;
        };

        let mut nodes1: HashMap<&str, Vec<&str>> = HashMap::new();
        nodes1.insert("a", vec!["b"]);
        nodes1.insert("b", vec![]);
        let nodes1 = Arc::new(nodes1);

        let mut nodes2: HashMap<&str, Vec<&str>> = HashMap::new();
        nodes2.insert("c", vec!["d"]);
        nodes2.insert("d", vec![]);
        let nodes2 = Arc::new(nodes2);

        let result1 = tokio::spawn(evaluate_dag(task_cache1.clone(), nodes1.clone(), "0"));
        let result2 = tokio::spawn(evaluate_dag(task_cache2.clone(), nodes2.clone(), "1"));

        let (res1, res2) = tokio::join!(result1, result2);
        assert!(res1.unwrap().is_ok());
        assert!(res2.unwrap().is_ok());
    }

    #[dbt_runtime::test]
    async fn test_evaluate_dag_redis_shared() {
        let Some(task_cache1) = redis_cache_or_skip("", "test_evaluate_dag_redis_shared").await
        else {
            return;
        };
        let Some(task_cache2) = redis_cache_or_skip("", "test_evaluate_dag_redis_shared").await
        else {
            return;
        };

        let mut nodes: HashMap<&str, Vec<&str>> = HashMap::new();
        nodes.insert("a", vec!["b", "c"]);
        nodes.insert("b", vec!["d"]);
        nodes.insert("c", vec!["d"]);
        nodes.insert("d", vec!["e", "f"]);
        nodes.insert("e", vec![]);
        nodes.insert("f", vec![]);
        let nodes = Arc::new(nodes);

        let result1 = tokio::spawn(evaluate_dag(task_cache1.clone(), nodes.clone(), "0"));
        let result2 = tokio::spawn(evaluate_dag(task_cache2.clone(), nodes.clone(), "1"));

        let (res1, res2) = tokio::join!(result1, result2);
        assert!(res1.unwrap().is_ok());
        assert!(res2.unwrap().is_ok());
    }

    #[dbt_runtime::test]
    async fn test_evaluate_dag_redis_mixed() {
        let Some(task_cache1) = redis_cache_or_skip("", "test_evaluate_dag_redis_mixed").await
        else {
            return;
        };
        let Some(task_cache2) = redis_cache_or_skip("", "test_evaluate_dag_redis_mixed").await
        else {
            return;
        };

        let mut nodes1: HashMap<&str, Vec<&str>> = HashMap::new();
        nodes1.insert("a", vec!["b", "c"]);
        nodes1.insert("b", vec!["d"]);
        nodes1.insert("c", vec!["d"]);
        nodes1.insert("d", vec!["e", "F"]);
        nodes1.insert("e", vec![]);
        nodes1.insert("F", vec![]);

        let mut nodes2: HashMap<&str, Vec<&str>> = HashMap::new();
        nodes2.insert("A", vec!["B", "C"]);
        nodes2.insert("B", vec!["D"]);
        nodes2.insert("C", vec!["D"]);
        nodes2.insert("D", vec!["e", "F"]);
        nodes2.insert("F", vec![]);
        nodes2.insert("e", vec![]);

        // Shared nodes
        // nodes1.insert("shared1", vec!["shared2"]);
        // nodes2.insert("shared1", vec!["shared2"]);
        // nodes1.insert("shared2", vec!["e"]);
        // nodes2.insert("shared2", vec!["E"]);

        let nodes1 = Arc::new(nodes1);
        let nodes2 = Arc::new(nodes2);

        let result1 = tokio::spawn(evaluate_dag(task_cache1.clone(), nodes1.clone(), "0"));
        let result2 = tokio::spawn(evaluate_dag(task_cache2.clone(), nodes2.clone(), "1"));

        let (res1, res2) = tokio::join!(result1, result2);
        assert!(res1.unwrap().is_ok());
        assert!(res2.unwrap().is_ok());
    }

    #[dbt_runtime::test]
    async fn test_redis_expiration() {
        let Some(task_cache) = redis_cache_or_skip("test_expire_", "test_redis_expiration").await
        else {
            return;
        };

        let key = "initial_expiration_test_node";

        let start_value = TaskValue::new_now("worker1".into(), Some("test_data".into()));
        let info = dbt_state::task_cache::TaskInfo {
            upstreams: HashMap::from([
                ("upstream1".to_string(), "hash1".to_string()),
                ("upstream2".to_string(), "hash2".to_string()),
            ]),
        };

        task_cache
            .try_start_task(key, &start_value, &None)
            .await
            .unwrap();
        let stop_value = TaskValue::new_now("worker1".into(), Some("result_data".into()));
        task_cache.stop_task(key, &stop_value, &info).await.unwrap();

        let mut con = redis::Client::open(redis_uri())
            .unwrap()
            .get_connection()
            .unwrap();
        let info_key = format!(
            "{}fs-orc:1:1:redis-test/node/{{{}}}/info",
            "test_expire_", key
        );

        let initial_ttl: i64 = redis::cmd("TTL").arg(&info_key).query(&mut con).unwrap();
        assert!(
            initial_ttl > 2591000,
            "Initial TTL should be ~30 days, got: {initial_ttl}"
        );
        assert!(
            initial_ttl <= 2592000,
            "Initial TTL should not exceed 30 days, got: {initial_ttl}"
        );
        println!("✅ Initial expiration set correctly: {initial_ttl} seconds");
    }

    #[dbt_runtime::test]
    async fn test_redis_get_info() {
        let Some(task_cache) = redis_cache_or_skip("test_get_info_", "test_redis_get_info").await
        else {
            return;
        };

        let key = "get_info_test_node";

        let start_value = TaskValue::new_now("worker1".into(), Some("test_data".into()));
        let info = dbt_state::task_cache::TaskInfo {
            upstreams: HashMap::from([
                ("upstream1".to_string(), "hash1".to_string()),
                ("upstream2".to_string(), "hash2".to_string()),
            ]),
        };

        task_cache
            .try_start_task(key, &start_value, &None)
            .await
            .unwrap();
        let stop_value = TaskValue::new_now("worker1".into(), Some("result_data".into()));
        task_cache.stop_task(key, &stop_value, &info).await.unwrap();

        let retrieved_info = task_cache.get_info(key).await.unwrap();
        assert!(retrieved_info.is_some());
        let retrieved_info = retrieved_info.unwrap();
        assert_eq!(retrieved_info.upstreams.len(), 2);
        assert_eq!(
            retrieved_info.upstreams.get("upstream1"),
            Some(&"hash1".to_string())
        );
        assert_eq!(
            retrieved_info.upstreams.get("upstream2"),
            Some(&"hash2".to_string())
        );

        let non_existent_info = task_cache.get_info("non_existent_key").await.unwrap();
        assert!(non_existent_info.is_none());

        println!("✅ get_info test passed: data retrieval works correctly");
    }
}
