#[cfg(test)]
// Failed to get Redis connection: No connection could be made because the target machine actively refused it. (os error 10061)
#[cfg(not(target_os = "windows"))]
mod tests {
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{collections::HashMap, sync::Arc};

    use crossbeam_queue::SegQueue;
    use dbt_common::collections::SccHashMap;
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
            Contention::Waiting { .. } => {
                println!("[Worker {owner_id}] Waiting for Task: {node_id}")
            }
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

    async fn acquire_execute_guard(
        task_cache: Arc<TaskCacheRedis<MultiplexedConnection>>,
        node_id: &str,
        owner_id: &str,
    ) -> dbt_state::node_session::ExecutionGuard {
        let erased: Arc<dyn TaskCache> = task_cache;
        let session = NodeCacheSession::new(erased, node_id.to_string(), owner_id.to_string());
        match session.acquire("", &|_| {}).await.unwrap() {
            Acquired::Execute(guard) => guard,
            _ => panic!("expected to own the slot for {node_id}"),
        }
    }

    #[dbt_runtime::test]
    async fn test_redis_release_leaves_node_not_started() {
        let Some(task_cache) = redis_cache_or_skip(
            "test_release_",
            "test_redis_release_leaves_node_not_started",
        )
        .await
        else {
            return;
        };
        let key = "release_test_node";
        task_cache.invalidate_task(key).await.unwrap();

        let guard = acquire_execute_guard(task_cache.clone(), key, "123/invocation-a").await;
        let (state, _) = task_cache.get_task_state(key).await.unwrap();
        assert_eq!(state, TaskState::InProgress);

        guard.release().await.unwrap();

        // No `stop` was written, so the next acquirer takes the slot directly
        // instead of finding a `Completed` entry to evict.
        let (state, _) = task_cache.get_task_state(key).await.unwrap();
        assert_eq!(state, TaskState::NotStarted);
        assert!(task_cache.get_stop(key).await.is_err());
        let guard = acquire_execute_guard(task_cache.clone(), key, "456/invocation-b").await;
        guard.release().await.unwrap();
    }

    #[dbt_runtime::test]
    async fn test_redis_drop_without_release_leaves_node_not_started() {
        let Some(task_cache) = redis_cache_or_skip(
            "test_drop_cleanup_",
            "test_redis_drop_without_release_leaves_node_not_started",
        )
        .await
        else {
            return;
        };
        let key = "drop_cleanup_test_node";
        task_cache.invalidate_task(key).await.unwrap();

        let guard = acquire_execute_guard(task_cache.clone(), key, "123/invocation-a").await;
        let (state, _) = task_cache.get_task_state(key).await.unwrap();
        assert_eq!(state, TaskState::InProgress);

        // Simulate an early `?` return: drop the guard without calling
        // `release` or `finalize`. `Drop` must still CAS-delete the `start`
        // key so the next acquirer finds `NotStarted` instead of waiting out
        // the heartbeat staleness window, same as an explicit `release()`.
        drop(guard);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let (state, _) = task_cache.get_task_state(key).await.unwrap();
            if state == TaskState::NotStarted {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "drop should CAS-delete the start key like release() does"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(task_cache.get_stop(key).await.is_err());
    }

    #[dbt_runtime::test]
    async fn test_redis_release_does_not_delete_another_owners_start() {
        let Some(task_cache) = redis_cache_or_skip(
            "test_release_takeover_",
            "test_redis_release_does_not_delete_another_owners_start",
        )
        .await
        else {
            return;
        };
        let key = "release_takeover_test_node";
        task_cache.invalidate_task(key).await.unwrap();

        let stale_guard = acquire_execute_guard(task_cache.clone(), key, "123/invocation-a").await;

        // Another run judged us stale, evicted our `start` and took the slot.
        let (_, (our_start, _, _)) = task_cache.get_task_state(key).await.unwrap();
        task_cache.try_cleanup_task(key, &our_start).await.unwrap();
        let takeover = TaskValue::new_now("456/invocation-b".into(), Some("".into()));
        assert!(
            task_cache
                .try_start_task(key, &takeover, &None)
                .await
                .unwrap()
        );

        stale_guard.release().await.unwrap();

        let start = task_cache.get_start(key).await.unwrap();
        assert_eq!(start.owner_id, "456/invocation-b");
    }

    /// Mirrors `test_redis_release_does_not_delete_another_owners_start`, but
    /// for the heartbeat write instead of the release-time delete: a worker
    /// evicted as stale may not know it lost the lock yet (its own heartbeat
    /// loop keeps running), and that loop must not be able to clobber the new
    /// owner's heartbeat once someone else holds the slot.
    #[dbt_runtime::test]
    async fn test_redis_reclaimed_owners_heartbeat_does_not_clobber_new_owner() {
        let Some(task_cache) = redis_cache_or_skip(
            "test_heartbeat_takeover_",
            "test_redis_reclaimed_owners_heartbeat_does_not_clobber_new_owner",
        )
        .await
        else {
            return;
        };
        let key = "heartbeat_takeover_test_node";
        task_cache.invalidate_task(key).await.unwrap();

        // Owner A acquires the slot; this starts its background heartbeat loop.
        let guard_a = acquire_execute_guard(task_cache.clone(), key, "123/invocation-a").await;

        // Wait for A's loop to land its first heartbeat write.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let heartbeat_before = loop {
            if let Ok(heartbeat) = task_cache.get_heartbeat(key).await {
                break heartbeat;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "owner A should have written its first heartbeat by now"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        };
        assert_eq!(heartbeat_before.owner_id, "123/invocation-a");

        // Another run judges A stale, evicts it, and takes the slot — but A's
        // own heartbeat loop is still alive (e.g. it was just slow, not
        // actually dead) and keeps trying to write on its own schedule.
        let (_, (start_a, _, _)) = task_cache.get_task_state(key).await.unwrap();
        task_cache.try_cleanup_task(key, &start_a).await.unwrap();
        let start_b = TaskValue::new_now("456/invocation-b".into(), Some("hash-b".into()));
        assert!(
            task_cache
                .try_start_task(key, &start_b, &None)
                .await
                .unwrap()
        );

        // Give A's loop time for at least one more write attempt (its
        // backoff is at most INITIAL_DELAY=2s at this point); that attempt
        // must lose the CAS against the now-superseded start value it holds,
        // leaving the heartbeat entry exactly as A last (legitimately) wrote
        // it rather than refreshed with a newer timestamp.
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let heartbeat_after = task_cache.get_heartbeat(key).await.unwrap();
        assert_eq!(
            heartbeat_after.timestamp, heartbeat_before.timestamp,
            "a reclaimed owner's heartbeat write must not land once another owner holds the lock"
        );

        // A's own cleanup must not disturb B's slot either.
        guard_a.release().await.unwrap();
        let start = task_cache.get_start(key).await.unwrap();
        assert_eq!(start.owner_id, "456/invocation-b");
    }

    /// Mirrors the heartbeat takeover test, but for `finalize`'s stop/info
    /// write: a reclaimed owner that was only slow (not actually dead) may
    /// still call `finalize` after another owner has already taken the slot
    /// and completed it. That late write must not overwrite the new owner's
    /// completion with the stale one.
    #[dbt_runtime::test]
    async fn test_redis_reclaimed_owners_finalize_does_not_clobber_new_owners_completion() {
        let Some(task_cache) = redis_cache_or_skip(
            "test_stop_takeover_",
            "test_redis_reclaimed_owners_finalize_does_not_clobber_new_owners_completion",
        )
        .await
        else {
            return;
        };
        let key = "stop_takeover_test_node";
        task_cache.invalidate_task(key).await.unwrap();

        // Owner A acquires the slot but is slow; another run judges it stale,
        // evicts it, and takes over.
        let guard_a = acquire_execute_guard(task_cache.clone(), key, "123/invocation-a").await;
        let (_, (start_a, _, _)) = task_cache.get_task_state(key).await.unwrap();
        task_cache.try_cleanup_task(key, &start_a).await.unwrap();
        let start_b = TaskValue::new_now("456/invocation-b".into(), Some("hash-b".into()));
        assert!(
            task_cache
                .try_start_task(key, &start_b, &None)
                .await
                .unwrap()
        );

        // Owner B runs to completion.
        let stop_b = TaskValue::new_now("456/invocation-b".into(), Some("hash-b".into()));
        let info_b = dbt_state::task_cache::TaskInfo {
            upstreams: HashMap::new(),
        };
        task_cache
            .stop_task(key, &stop_b, &info_b, &start_b)
            .await
            .unwrap();

        // A's own `finalize` only now lands, carrying its own stale result.
        // It must not clobber B's completion.
        guard_a.finalize(HashMap::new()).await.unwrap();

        let stop = task_cache.get_stop(key).await.unwrap();
        assert_eq!(
            stop.owner_id, "456/invocation-b",
            "a reclaimed owner's finalize must not overwrite another owner's completion"
        );
        assert_eq!(stop.value, Some("hash-b".to_string()));
    }

    /// `write_completed` pre-seeds a Completed entry for a node whose relation
    /// already exists in the warehouse. It must not stomp on a slot someone
    /// else already won — neither the real run actually building the node,
    /// nor another concurrent pre-seed call for the same node.
    #[dbt_runtime::test]
    async fn test_redis_write_completed_does_not_clobber_an_in_progress_slot() {
        let Some(task_cache) = redis_cache_or_skip(
            "test_write_completed_takeover_",
            "test_redis_write_completed_does_not_clobber_an_in_progress_slot",
        )
        .await
        else {
            return;
        };
        let key = "write_completed_takeover_test_node";
        task_cache.invalidate_task(key).await.unwrap();

        // Owner A is genuinely in progress on this node.
        let guard_a = acquire_execute_guard(task_cache.clone(), key, "123/invocation-a").await;

        // A pre-seed pass for the same node, from a different owner, loses
        // the race and must back off instead of overwriting A's slot.
        let erased: Arc<dyn TaskCache> = task_cache.clone();
        let session = NodeCacheSession::new(erased, key.to_string(), "456/preseed".to_string());
        session
            .write_completed("preseed-hash", HashMap::new())
            .await
            .unwrap();

        let (state, (start, _, _)) = task_cache.get_task_state(key).await.unwrap();
        assert_eq!(state, TaskState::InProgress);
        assert_eq!(start.unwrap().owner_id, "123/invocation-a");
        assert!(
            task_cache.get_stop(key).await.is_err(),
            "write_completed must not write a stop entry for a slot it didn't win"
        );

        guard_a.release().await.unwrap();
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
        let indegree = Arc::new(SccHashMap::<String, AtomicUsize>::default());

        for (id, children) in nodes.iter() {
            indegree
                .insert_sync(id.to_string(), AtomicUsize::new(children.len()))
                .expect("node ids are unique");
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
            if indegree.read_sync(id, |_, count| count.load(Ordering::SeqCst) == 0) == Some(true) {
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
                            if indegree.read_sync(parent_id, |_, count| {
                                count.fetch_sub(1, Ordering::SeqCst) == 1
                            }) == Some(true)
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
        cache.stop_task(key, &stop, &info, &start).await.unwrap();

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
        task_cache
            .stop_task(key, &stop_value, &info, &start_value)
            .await
            .unwrap();

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
        task_cache
            .stop_task(key, &stop_value, &info, &start_value)
            .await
            .unwrap();

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

    /// A later step of the same run re-claiming a node must not inherit the
    /// overdue heartbeat an earlier step left behind. Eviction only deletes
    /// `start`, so this relies on each invocation having its own owner ID
    /// (`resolve_lock_owner_id` appends the invocation ID to the run ID).
    #[dbt_runtime::test]
    async fn test_redis_reclaim_ignores_prior_step_heartbeat() {
        let prefix = "test_reclaim_heartbeat_";
        let Some(task_cache) =
            redis_cache_or_skip(prefix, "test_redis_reclaim_ignores_prior_step_heartbeat").await
        else {
            return;
        };

        let key = "reclaim_heartbeat_test_node";
        task_cache.invalidate_task(key).await.unwrap();

        // Step 1 of run 123 builds the node and releases it.
        let step_1 = "123/invocation-step-1";
        let start_1 = TaskValue::new_now(step_1.into(), Some("hash".into()));
        assert!(
            task_cache
                .try_start_task(key, &start_1, &None)
                .await
                .unwrap()
        );
        let stop_1 = TaskValue::new_now(step_1.into(), Some("hash".into()));
        let info = dbt_state::task_cache::TaskInfo {
            upstreams: HashMap::new(),
        };
        task_cache
            .stop_task(key, &stop_1, &info, &start_1)
            .await
            .unwrap();

        // Its last heartbeat stays behind, long overdue.
        let overdue = TaskValue::new(
            step_1.into(),
            chrono::Utc::now() - chrono::Duration::minutes(10),
            Some("hash".into()),
        );
        let mut con = redis::Client::open(redis_uri())
            .unwrap()
            .get_connection()
            .unwrap();
        let heartbeat_key = format!("{prefix}fs-orc:1:1:redis-test/node/{{{key}}}/heartbeat");
        let _: () = redis::cmd("SET")
            .arg(&heartbeat_key)
            .arg(serde_json::to_string(&overdue).unwrap())
            .query(&mut con)
            .unwrap();

        // Step 2 of the same run evicts the completed entry and re-claims it.
        task_cache
            .try_cleanup_task(key, &Some(start_1))
            .await
            .unwrap();
        let start_2 = TaskValue::new_now("123/invocation-step-2".into(), Some("hash".into()));
        assert!(
            task_cache
                .try_start_task(key, &start_2, &None)
                .await
                .unwrap()
        );

        // Before step 2 writes its own heartbeat, other runs must see it as
        // in progress, not stale.
        let (state, _) = task_cache.get_task_state(key).await.unwrap();
        assert_eq!(state, TaskState::InProgress);
    }

    /// Two independent acquirers (own `TaskCacheRedis` connection each, standing
    /// in for two separate dbt processes) racing for the same node against one
    /// shared Redis: only one gets `Acquired::Execute` while the other blocks,
    /// and the waiter wins the slot once the holder releases rather than
    /// waiting indefinitely.
    #[dbt_runtime::test]
    async fn test_redis_two_independent_acquirers_race_and_waiter_wins_after_release() {
        let prefix = "test_two_acquirers_";
        let Some(cache_a) = redis_cache_or_skip(
            prefix,
            "test_redis_two_independent_acquirers_race_and_waiter_wins_after_release",
        )
        .await
        else {
            return;
        };
        let Some(cache_b) = redis_cache_or_skip(
            prefix,
            "test_redis_two_independent_acquirers_race_and_waiter_wins_after_release",
        )
        .await
        else {
            return;
        };

        let key = "two_acquirers_race_node";
        cache_a.invalidate_task(key).await.unwrap();

        let session_a = NodeCacheSession::new(
            cache_a.clone() as Arc<dyn TaskCache>,
            key.to_string(),
            "123/invocation-a".to_string(),
        );
        let guard_a = match session_a.acquire("hash-a", &|_| {}).await.unwrap() {
            Acquired::Execute(guard) => guard,
            _ => panic!("expected the first acquirer to win the slot immediately"),
        };

        let waiting_notify = Arc::new(tokio::sync::Notify::new());
        let notify_for_b = waiting_notify.clone();
        let observed_blocking_run_id = Arc::new(std::sync::Mutex::new(None));
        let observed_for_b = observed_blocking_run_id.clone();
        let cache_b_erased: Arc<dyn TaskCache> = cache_b.clone();
        let key_for_b = key.to_string();
        let b_handle = tokio::spawn(async move {
            let session_b =
                NodeCacheSession::new(cache_b_erased, key_for_b, "456/invocation-b".to_string());
            let on_contention = move |c: Contention| {
                if let Contention::Waiting { blocking_run_id } = c {
                    *observed_for_b.lock().unwrap() = Some(blocking_run_id);
                    notify_for_b.notify_one();
                }
            };
            session_b.acquire("hash-b", &on_contention).await
        });

        tokio::time::timeout(std::time::Duration::from_secs(5), waiting_notify.notified())
            .await
            .expect(
                "second acquirer should observe contention while the first still holds the lock",
            );

        assert_eq!(
            observed_blocking_run_id.lock().unwrap().clone(),
            Some(Some("123".to_string())),
            "the waiter should see the first acquirer's run ID, extracted from its <run_id>/<invocation_id> owner"
        );

        let (state, (start, _, _)) = cache_a.get_task_state(key).await.unwrap();
        assert_eq!(state, TaskState::InProgress);
        assert_eq!(start.unwrap().owner_id, "123/invocation-a");

        guard_a.release().await.unwrap();

        let acquired_b = tokio::time::timeout(std::time::Duration::from_secs(5), b_handle)
            .await
            .expect("second acquirer should finish shortly after the first released the lock")
            .expect("acquire task should not panic")
            .expect("acquire should not error");

        match acquired_b {
            Acquired::Execute(guard_b) => guard_b.release().await.unwrap(),
            _ => panic!("expected the second acquirer to win the slot after the first released it"),
        }
    }
}
