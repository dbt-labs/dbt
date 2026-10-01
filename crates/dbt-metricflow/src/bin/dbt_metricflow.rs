use std::process;

use dbt_metricflow::{
    Dialect, InMemoryMetricStore, SemanticQuerySpec, compile, parse_group_by_str,
    parse_order_by_str,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 || args[1] == "--help" || args[1] == "-h" {
        print_help();
        return;
    }

    match args[1].as_str() {
        "compile" => run_compile(&args[2..]),
        "example" => print_example(),
        other => {
            eprintln!("unknown command: {other}");
            eprintln!("run with --help for usage");
            process::exit(1);
        }
    }
}

fn run_compile(args: &[String]) {
    let mut manifest_path = None;
    let mut metrics = Vec::new();
    let mut group_by = Vec::new();
    let mut where_filters = Vec::new();
    let mut order_by = Vec::new();
    let mut limit = None;
    let mut dialect = Dialect::DuckDB;
    let mut time_constraint = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--manifest" | "-m" => {
                i += 1;
                manifest_path = Some(&args[i]);
            }
            "--metrics" => {
                i += 1;
                metrics = args[i].split(',').map(|s| s.trim().to_string()).collect();
            }
            "--group-by" | "--group_by" => {
                i += 1;
                for g in args[i].split(',') {
                    let g = g.trim();
                    match parse_group_by_str(g) {
                        Some(gb) => group_by.push(gb),
                        None => {
                            eprintln!("invalid group-by: {g:?}");
                            process::exit(1);
                        }
                    }
                }
            }
            "--where" => {
                i += 1;
                where_filters.push(expand_where_shorthand(&args[i]));
            }
            "--order-by" | "--order_by" => {
                i += 1;
                for o in args[i].split(',') {
                    let o = o.trim();
                    match parse_order_by_str(o) {
                        Some(ob) => order_by.push(ob),
                        None => {
                            eprintln!("invalid order-by: {o:?}");
                            process::exit(1);
                        }
                    }
                }
            }
            "--limit" => {
                i += 1;
                limit = Some(args[i].parse::<usize>().unwrap_or_else(|_| {
                    eprintln!("invalid --limit: {:?}", args[i]);
                    process::exit(1);
                }));
            }
            "--dialect" => {
                i += 1;
                dialect = args[i].parse().unwrap_or_else(|e| {
                    eprintln!("{e}");
                    process::exit(1);
                });
            }
            "--time-constraint" => {
                let start = args.get(i + 1).cloned().unwrap_or_default();
                let end = args.get(i + 2).cloned().unwrap_or_default();
                if start.is_empty() || end.is_empty() {
                    eprintln!("--time-constraint requires START END");
                    process::exit(1);
                }
                time_constraint = Some((start, end));
                i += 2;
            }
            "--help" | "-h" => {
                print_help();
                return;
            }
            other => {
                eprintln!("unknown flag: {other}");
                process::exit(1);
            }
        }
        i += 1;
    }

    let manifest_path = manifest_path.unwrap_or_else(|| {
        eprintln!("error: --manifest is required");
        process::exit(1);
    });

    if metrics.is_empty() {
        eprintln!("error: --metrics is required");
        process::exit(1);
    }

    let manifest_str = std::fs::read_to_string(manifest_path).unwrap_or_else(|e| {
        eprintln!("error reading {manifest_path}: {e}");
        process::exit(1);
    });
    let manifest: serde_json::Value = serde_json::from_str(&manifest_str).unwrap_or_else(|e| {
        eprintln!("error parsing manifest JSON: {e}");
        process::exit(1);
    });

    let mut store = InMemoryMetricStore::from_manifest(&manifest);
    let spec = SemanticQuerySpec {
        metrics,
        group_by,
        where_filters,
        order_by,
        limit,
        time_constraint,
        apply_group_by: true,
    };

    match compile(&mut store, &spec, dialect) {
        Ok(sql) => println!("{sql}"),
        Err(e) => {
            eprintln!("compile error: {e}");
            process::exit(1);
        }
    }
}

/// Expand CLI where shorthand into Jinja format.
///   `status=completed`         → `{{ Dimension('status') }} = 'completed'`
///   `metric_time:day>=2024-01` → `{{ TimeDimension('metric_time', 'day') }} >= '2024-01'`
fn expand_where_shorthand(s: &str) -> String {
    if s.contains("{{") {
        return s.to_string();
    }
    let ops = ["!=", ">=", "<=", "=", ">", "<"];
    let Some((op_pos, op)) = ops.iter().find_map(|op| s.find(op).map(|pos| (pos, *op))) else {
        return s.to_string();
    };
    let lhs = s[..op_pos].trim();
    let rhs = s[op_pos + op.len()..].trim();
    if lhs.is_empty() || rhs.is_empty() {
        return s.to_string();
    }
    let dim_ref = if let Some((name, granularity)) = lhs.split_once(':') {
        format!("{{{{ TimeDimension('{name}', '{granularity}') }}}}")
    } else {
        format!("{{{{ Dimension('{lhs}') }}}}")
    };
    let already_literal = rhs.parse::<f64>().is_ok()
        || (rhs.starts_with('\'') && rhs.ends_with('\''))
        || (rhs.starts_with('"') && rhs.ends_with('"'));
    let rhs_sql = if already_literal {
        rhs.to_string()
    } else if rhs.eq_ignore_ascii_case("true") || rhs.eq_ignore_ascii_case("false") {
        rhs.to_lowercase()
    } else {
        format!("'{rhs}'")
    };
    format!("{dim_ref} {op} {rhs_sql}")
}

fn print_example() {
    println!(
        r#"# Minimal semantic manifest (save as manifest.json):
{{
  "semantic_models": [
    {{
      "name": "orders",
      "node_relation": {{
        "alias": "orders",
        "schema_name": "public",
        "relation_name": "\"public\".\"orders\""
      }},
      "defaults": {{ "agg_time_dimension": "ordered_at" }},
      "primary_entity": "order",
      "entities": [
        {{ "name": "order",    "type": "primary", "expr": "order_id" }},
        {{ "name": "customer", "type": "foreign", "expr": "customer_id" }}
      ],
      "measures": [
        {{ "name": "order_count", "agg": "sum", "expr": "1" }},
        {{ "name": "revenue",     "agg": "sum", "expr": "amount" }}
      ],
      "dimensions": [
        {{ "name": "ordered_at", "type": "time",        "expr": "ordered_at",
           "type_params": {{ "time_granularity": "day" }} }},
        {{ "name": "status",     "type": "categorical", "expr": "status" }}
      ]
    }}
  ],
  "metrics": [
    {{
      "name": "order_count",
      "type": "simple",
      "type_params": {{
        "metric_aggregation_params": {{
          "semantic_model": "orders",
          "agg": "sum",
          "expr": "1",
          "agg_time_dimension": "ordered_at"
        }}
      }}
    }},
    {{
      "name": "revenue",
      "type": "simple",
      "type_params": {{
        "metric_aggregation_params": {{
          "semantic_model": "orders",
          "agg": "sum",
          "expr": "amount",
          "agg_time_dimension": "ordered_at"
        }}
      }}
    }}
  ],
  "project_configuration": {{
    "time_spines": [
      {{
        "node_relation": {{
          "relation_name": "\"public\".\"time_spine\""
        }},
        "primary_column": {{ "name": "ds", "time_granularity": "day" }}
      }}
    ]
  }}
}}

# Example queries:

  dbt-metricflow compile -m manifest.json --metrics order_count
  dbt-metricflow compile -m manifest.json --metrics revenue --group-by metric_time:day
  dbt-metricflow compile -m manifest.json --metrics revenue --group-by metric_time:month,status
  dbt-metricflow compile -m manifest.json --metrics order_count --where "status=completed"
  dbt-metricflow compile -m manifest.json --metrics revenue --group-by metric_time:day --dialect snowflake
  dbt-metricflow compile -m manifest.json --metrics order_count,revenue --group-by metric_time:day"#
    );
}

fn print_help() {
    eprintln!(
        r#"dbt-metricflow — standalone semantic query compiler

USAGE:
  dbt-metricflow compile [OPTIONS]
  dbt-metricflow example

COMMANDS:
  compile    Compile a metric query to SQL
  example    Print an example manifest and sample queries

OPTIONS:
  -m, --manifest <PATH>    Path to semantic manifest JSON (required)
  --metrics <m1,m2,...>    Comma-separated metric names (required)
  --group-by <specs>       Comma-separated: metric_time:day, entity__dim, dim
  --where <filter>         Where filter (repeatable). Shorthand: status=completed
                           or Jinja: {{ Dimension('status') }} = 'completed'
  --order-by <specs>       Comma-separated, prefix with - for DESC: -metric_time
  --limit <N>              Max rows
  --dialect <name>         duckdb (default), snowflake, redshift, bigquery, databricks
  --time-constraint S E    Constrain metric_time to [S, E]

EXAMPLES:
  dbt-metricflow compile -m manifest.json --metrics revenue
  dbt-metricflow compile -m manifest.json --metrics revenue --group-by metric_time:day
  dbt-metricflow compile -m manifest.json --metrics revenue --group-by metric_time:month,status
  dbt-metricflow compile -m manifest.json --metrics revenue --where "status=completed" --dialect snowflake
  dbt-metricflow example | head -80 > manifest.json  # generate example manifest"#
    );
}
