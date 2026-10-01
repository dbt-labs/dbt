use crate::{
    TelemetryOutputFlags,
    emit::{create_info_span, create_root_info_span},
    init::create_tracing_subcriber_with_layer,
    layer::{ConsumerLayer, MiddlewareLayer},
    metrics::{MetricKey, get_metric},
    tests::mocks::{MockDynSpanEvent, MockMiddleware, TestLayer, test_data_layer},
};

const TEST_WARNING_METRIC: MetricKey = MetricKey::from_raw(1);

#[derive(Debug, Clone, PartialEq)]
struct TestExtension {
    value: u64,
}

#[test]
fn data_provider_isolates_roots_and_shares_within_tree() {
    let trace_id = rand::random::<u128>();
    let (test_layer, ..) = TestLayer::new();

    let test_metric = TEST_WARNING_METRIC;

    // Middleware will increment metric and store extension on each span
    let middleware = MockMiddleware::new().with_span_start(move |span, data_provider| {
        let value = if span.span_name.contains("root1") {
            10
        } else if span.span_name.contains("root2") {
            20
        } else {
            0
        };

        if value > 0 {
            data_provider.increment_metric(test_metric, value);
            let _ = data_provider.init_root(TestExtension { value });
        }

        Some(span)
    });

    let test_layer = test_layer.with_span_start(move |span, data_provider| {
        let expected_value = if span.span_name.ends_with("1") {
            10
        } else {
            20
        };

        // Check metric is as expected
        let metric = data_provider.get_metric(test_metric);
        assert_eq!(
            metric, expected_value,
            "{} should see metric={}",
            span.span_name, expected_value
        );
        // Check extension is as expected
        data_provider.with_root(|ext: &TestExtension| {
            assert_eq!(
                ext.value, expected_value,
                "{} should see extension value={}",
                span.span_name, expected_value
            );
        });
    });

    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::once(Box::new(middleware) as MiddlewareLayer),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        // First root span tree: root1 -> child1 -> grandchild1
        let root1_guard = create_root_info_span(MockDynSpanEvent {
            name: "root1".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        })
        .entered();

        let metric1_after_root = get_metric(test_metric);
        assert_eq!(metric1_after_root, 10, "root1 should have metric=10");

        create_info_span(MockDynSpanEvent {
            name: "child1".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        })
        .in_scope(|| {
            let metric1_in_child = get_metric(test_metric);
            assert_eq!(metric1_in_child, 10, "child1 should see root1 metric=10");

            create_info_span(MockDynSpanEvent {
                name: "grandchild1".to_string(),
                flags: TelemetryOutputFlags::empty(),
                ..Default::default()
            })
            .in_scope(|| {
                let metric1_in_grandchild = get_metric(test_metric);
                assert_eq!(
                    metric1_in_grandchild, 10,
                    "grandchild1 should see root1 metric=10"
                );
            });
        });

        drop(root1_guard);

        // Second root span tree: root2 -> child2 -> grandchild2
        let _root2_guard = create_root_info_span(MockDynSpanEvent {
            name: "root2".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        })
        .entered();

        let metric2_after_root = get_metric(test_metric);
        assert_eq!(metric2_after_root, 20, "root2 should have metric=20");

        create_info_span(MockDynSpanEvent {
            name: "child2".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        })
        .in_scope(|| {
            let metric2_in_child = get_metric(test_metric);
            assert_eq!(metric2_in_child, 20, "child2 should see root2 metric=20");

            create_info_span(MockDynSpanEvent {
                name: "grandchild2".to_string(),
                flags: TelemetryOutputFlags::empty(),
                ..Default::default()
            })
            .in_scope(|| {
                let metric2_in_grandchild = get_metric(test_metric);
                assert_eq!(
                    metric2_in_grandchild, 20,
                    "grandchild2 should see root2 metric=20"
                );
            });
        });
    });
}

#[derive(Debug, Clone, PartialEq)]
struct CustomExtension {
    counter: u64,
    message: String,
}

#[test]
fn data_provider_with_root_and_with_mut() {
    let trace_id = rand::random::<u128>();
    let (test_layer, ..) = TestLayer::new();

    // Middleware will insert custom extension on root span and verify final value on end
    let middleware = MockMiddleware::new()
        .with_span_start(move |span, data_provider| {
            if span.span_name.contains("root") {
                // Test insert
                data_provider.init_root(CustomExtension {
                    counter: 0,
                    message: "initial".to_string(),
                });
            }

            Some(span)
        })
        .with_span_end(move |span, data_provider| {
            if span.span_name.contains("root") {
                // Verify we can read the final value
                data_provider.with_root(|ext: &CustomExtension| {
                    assert_eq!(ext.counter, 2);
                    assert_eq!(ext.message, "updated 2");
                });
            }

            Some(span)
        });

    // Consumer will mutate the extension and verify
    let test_layer = test_layer.with_span_start(move |span, data_provider| {
        if span.span_name.ends_with("child1") {
            // Verify we can read the initial value
            data_provider.with_root(|ext: &CustomExtension| {
                assert_eq!(ext.counter, 0);
                assert_eq!(ext.message, "initial");
            });

            // Test with_mut by updating the extension
            data_provider.with_root_mut(|ext: &mut CustomExtension| {
                ext.counter += 1;
                ext.message = format!("updated {}", ext.counter);
            });
        }

        if span.span_name.ends_with("child2") {
            // Verify we can read the initial value
            data_provider.with_root(|ext: &CustomExtension| {
                assert_eq!(ext.counter, 1);
                assert_eq!(ext.message, "updated 1");
            });

            // Test get_mut
            data_provider.with_root_mut(|ext: &mut CustomExtension| {
                ext.counter += 1;
                ext.message = format!("updated {}", ext.counter);
            });
        }
    });

    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::once(Box::new(middleware) as MiddlewareLayer),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let _root_guard = create_root_info_span(MockDynSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        })
        .entered();

        // First child should increment counter to 1
        create_info_span(MockDynSpanEvent {
            name: "child1".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        });

        // Second child should increment counter to 2
        create_info_span(MockDynSpanEvent {
            name: "child2".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        });
    });
}

#[test]
fn data_provider_init_replaces_existing() {
    let trace_id = rand::random::<u128>();
    let (test_layer, ..) = TestLayer::new();

    let middleware = MockMiddleware::new().with_span_start(move |span, data_provider| {
        if span.span_name.contains("root") {
            // Insert first value
            let old = data_provider.init_root(CustomExtension {
                counter: 1,
                message: "first".to_string(),
            });
            assert!(old.is_none(), "First insert should return None");

            // Insert second value, should replace first
            let old = data_provider.init_root(CustomExtension {
                counter: 2,
                message: "second".to_string(),
            });
            assert!(old.is_some(), "Second insert should return Some");
            assert_eq!(old.unwrap().counter, 1);

            // Verify current value
            data_provider.with_root(|ext: &CustomExtension| {
                assert_eq!(ext.counter, 2);
                assert_eq!(ext.message, "second");
            });
        }

        Some(span)
    });

    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::once(Box::new(middleware) as MiddlewareLayer),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let _root_guard = create_root_info_span(MockDynSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        })
        .entered();
    });
}

#[derive(Debug, Clone, PartialEq)]
struct AncestorExtension {
    counter: u64,
}

#[test]
fn data_provider_init_ancestor_data_on_matching_span() {
    let trace_id = rand::random::<u128>();
    let (test_layer, ..) = TestLayer::new();

    // Middleware will init ancestor data on spans with MockDynSpanEvent attributes
    let middleware = MockMiddleware::new().with_span_start(move |span, data_provider| {
        if span.span_name.contains("child") {
            // Init data on this MockDynSpanEvent span
            let old = data_provider.init_cur(AncestorExtension { counter: 42 });
            assert!(old.is_none(), "First init should return None");
        }
        Some(span)
    });

    // Consumer will verify the data exists on ancestor span with matching attributes
    let test_layer = test_layer.with_span_start(move |span, data_provider| {
        if span.span_name.contains("grandchild") {
            // Should find the extension on ancestor (child) span with MockDynSpanEvent
            data_provider.with_ancestor_ext::<MockDynSpanEvent, _>(
                |attrs, ext: &AncestorExtension| {
                    assert_eq!(attrs.name, "child");
                    assert_eq!(ext.counter, 42);
                },
            );
        }
    });

    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::once(Box::new(middleware) as MiddlewareLayer),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let _root_guard = create_root_info_span(MockDynSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        })
        .entered();

        let _child_guard = create_info_span(MockDynSpanEvent {
            name: "child".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        })
        .entered();

        create_info_span(MockDynSpanEvent {
            name: "grandchild".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        });
    });
}

#[test]
fn data_provider_with_ancestor_ext_finds_closest() {
    let trace_id = rand::random::<u128>();
    let (test_layer, ..) = TestLayer::new();

    // Middleware will init ancestor data on root and child spans with MockDynSpanEvent
    let middleware = MockMiddleware::new().with_span_start(move |span, data_provider| {
        if span.span_name.contains("root") {
            data_provider.init_cur(AncestorExtension { counter: 10 });
        } else if span.span_name.contains("child") {
            data_provider.init_cur(AncestorExtension { counter: 20 });
        }
        Some(span)
    });

    // Consumer will verify the closest ancestor is found. We are testing subtlety here:
    // - on span start of child, the closest ancestor with initialized attributes is root
    // - but on span end, child can see it's own initialized attributes as closest ancestor
    let test_layer = test_layer
        .with_span_start(move |span, data_provider| {
            if span.span_name.contains("grandchild") {
                // Should find the child extension (closest MockDynSpanEvent span)
                data_provider.with_ancestor_ext::<MockDynSpanEvent, _>(
                    |_attrs, ext: &AncestorExtension| {
                        assert_eq!(ext.counter, 20, "Should find closest ancestor");
                    },
                );
            } else if span.span_name.contains("child") {
                // Should find the child extension (closest MockDynSpanEvent span)
                data_provider.with_ancestor_ext::<MockDynSpanEvent, _>(
                    |_attrs, ext: &AncestorExtension| {
                        assert_eq!(ext.counter, 10, "Should find closest ancestor");
                    },
                );
            }
        })
        .with_span_end(move |span, data_provider| {
            if span.span_name.contains("grandchild") || span.span_name.contains("child") {
                // Should find the child extension (closest MockDynSpanEvent span)
                data_provider.with_ancestor_ext::<MockDynSpanEvent, _>(
                    |_attrs, ext: &AncestorExtension| {
                        assert_eq!(ext.counter, 20, "Should find closest ancestor");
                    },
                );
            }
        });

    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::once(Box::new(middleware) as MiddlewareLayer),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let _root_guard = create_root_info_span(MockDynSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        })
        .entered();

        let _child_guard = create_info_span(MockDynSpanEvent {
            name: "child".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        })
        .entered();

        create_info_span(MockDynSpanEvent {
            name: "grandchild".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        });
    });
}

#[test]
fn data_provider_with_ancestor_ext_mut_modifies_closest() {
    let trace_id = rand::random::<u128>();
    let (test_layer, ..) = TestLayer::new();

    // Middleware will init ancestor data on child span with MockDynSpanEvent
    let middleware = MockMiddleware::new().with_span_start(move |span, data_provider| {
        if span.span_name.contains("child") {
            data_provider.init_cur(AncestorExtension { counter: 0 });
        }
        Some(span)
    });

    // Consumer will mutate and verify ancestor data from grandchildren
    let test_layer = test_layer.with_span_start(move |span, data_provider| {
        if span.span_name.contains("grandchild1") || span.span_name.contains("grandchild2") {
            // Increment the counter on the ancestor with MockDynSpanEvent
            data_provider.with_ancestor_ext_mut::<MockDynSpanEvent, _>(
                |ext: &mut AncestorExtension| {
                    ext.counter += 1;
                },
            );
        }

        if span.span_name.contains("grandchild2") {
            // Verify the counter was incremented by previous grandchildren
            let mut counter = 0;
            data_provider.with_ancestor_ext::<MockDynSpanEvent, _>(
                |_attrs, ext: &AncestorExtension| {
                    counter = ext.counter;
                },
            );
            assert!(
                counter >= 2,
                "Counter should be incremented by grandchildren, got {}",
                counter
            );
        }
    });

    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::once(Box::new(middleware) as MiddlewareLayer),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let _root_guard = create_root_info_span(MockDynSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        })
        .entered();

        let _child_guard = create_info_span(MockDynSpanEvent {
            name: "child".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        })
        .entered();

        {
            let _gc1 = create_info_span(MockDynSpanEvent {
                name: "grandchild1".to_string(),
                flags: TelemetryOutputFlags::empty(),
                ..Default::default()
            })
            .entered();
        }

        {
            let _gc2 = create_info_span(MockDynSpanEvent {
                name: "grandchild2".to_string(),
                flags: TelemetryOutputFlags::empty(),
                ..Default::default()
            })
            .entered();
        }
    });
}

#[test]
fn data_provider_with_ancestor_attrs_accesses_closest_attrs() {
    let trace_id = rand::random::<u128>();
    let (test_layer, ..) = TestLayer::new();

    let test_layer = test_layer
        .with_span_start(move |span, data_provider| {
            if span.span_name.contains("leaf") {
                data_provider.with_ancestor_attrs::<MockDynSpanEvent>(|attrs| {
                    assert_eq!(attrs.name, "child");
                });

                data_provider.with_ancestor_attrs_mut::<MockDynSpanEvent>(|attrs| {
                    attrs.name = "child updated".to_string();
                });
            }
        })
        .with_span_end(move |span, data_provider| {
            if span.span_name.contains("child") {
                data_provider.with_ancestor_attrs::<MockDynSpanEvent>(|attrs| {
                    assert_eq!(attrs.name, "child updated");
                });
            }
        });

    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::empty::<MiddlewareLayer>(),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let _root_guard = create_root_info_span(MockDynSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        })
        .entered();

        let _child_guard = create_info_span(MockDynSpanEvent {
            name: "child".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        })
        .entered();

        create_info_span(MockDynSpanEvent {
            name: "leaf".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        });
    });
}

#[test]
fn data_provider_ancestor_apis_return_none_when_not_found() {
    let trace_id = rand::random::<u128>();
    let (test_layer, ..) = TestLayer::new();

    let test_layer = test_layer.with_span_start(move |span, data_provider| {
        if span.span_name.contains("child") {
            // Try to access non-existent extension (no init was done)
            // Should be a no-op - closure not called
            let mut called = false;
            data_provider.with_ancestor_ext::<MockDynSpanEvent, AncestorExtension>(
                |_attrs, _ext| {
                    called = true;
                },
            );
            assert!(
                !called,
                "Closure should not be called when extension not found"
            );

            // Try to mutate non-existent extension - should be a no-op
            data_provider.with_ancestor_ext_mut::<MockDynSpanEvent, _>(
                |ext: &mut AncestorExtension| {
                    ext.counter += 1;
                },
            );
            // No assertion needed - it's a no-op if not found
        }
    });

    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::empty::<MiddlewareLayer>(),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let _root_guard = create_root_info_span(MockDynSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        })
        .entered();

        create_info_span(MockDynSpanEvent {
            name: "child".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        });
    });
}
