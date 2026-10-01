use clap::Parser;
use dbt_test_containers::container::docker::{
    ContainerConfig, PortBinding, fetch_container_logs, initialize_container, shutdown_container,
};
use std::collections::HashMap;

#[derive(Parser, Debug, Clone)]
pub struct StartDbArgs {
    /// Keep the database alive after the command completes
    #[arg(long)]
    pub no_keep_alive: bool,
    /// Don't reuse the latest container
    #[arg(long)]
    pub no_reuse_latest: bool,
    /// Stop the database
    #[arg(long)]
    pub stop: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = StartDbArgs::parse();

    let config = ContainerConfig {
        image_name_base: "postgres-fs-tests".to_string(),
        image_uri: None,
        dockerfile_path: Some("fs/sa/crates/dbt-test-containers/docker/postgres/Dockerfile".into()),
        ro_mount_paths: vec![],
        rw_mount_path: None,
        port_bindings: HashMap::from_iter([(
            "5432/tcp".to_string(),
            Some(vec![PortBinding {
                host_ip: Some("0.0.0.0".to_string()),
                host_port: Some("5499/tcp".to_string()),
            }]),
        )]),
        network_mode: None,
        reuse_latest: !args.no_reuse_latest,
        container_id: None,
        cmd: None,
        env: vec![
            ("POSTGRES_USER".to_string(), "postgres".to_string()),
            ("POSTGRES_PASSWORD".to_string(), "postgres".to_string()),
            ("POSTGRES_DB".to_string(), "dbt".to_string()),
        ],
        build_args: vec![],
        bind_user: false,
    };
    let container = initialize_container(config).await?;
    if args.stop || args.no_keep_alive {
        // Fetch the logs (to be used if debugging)
        let _ = fetch_container_logs(&container.name).await?;
        shutdown_container(&container.name).await?;
    }

    Ok(())
}
