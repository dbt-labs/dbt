//! DBT related tasks.

use std::{collections::HashMap, path::PathBuf};

use async_trait::async_trait;
use dbt_common::{
    constants::{DBT_MANIFEST_JSON, DBT_TARGET_DIR_NAME},
    stdfs,
};
use dbt_test_containers::container::docker::{
    ContainerConfig, MountPoint, initialize_container, wait_for_container_stop,
};
use dbt_test_primitives::is_update_golden_files_mode;

use super::{ProjectEnv, Task, TestEnv, TestResult};

pub struct DbtRecordTask {
    setup_cmds: Vec<String>,
    record_cmds: Vec<String>,
}

impl DbtRecordTask {
    pub fn new(setup_cmds: &[impl ToString], record_cmds: &[impl ToString]) -> Self {
        Self {
            setup_cmds: setup_cmds.iter().map(|s| s.to_string()).collect(),
            record_cmds: record_cmds.iter().map(|s| s.to_string()).collect(),
        }
    }
}

#[async_trait]
impl Task for DbtRecordTask {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        test_env: &TestEnv,
        task_index: usize,
    ) -> TestResult<()> {
        // Check if docker is enabled
        let is_docker_enabled = std::env::var("IS_DOCKER_ENABLED").is_ok();
        if is_update_golden_files_mode() && is_docker_enabled {
            // Located in the dbt-test-containers crate, at `crates/dbt-test-containers/docker/dbt-core/Dockerfile`
            let path_to_dbt_dockerfile = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("dbt-test-containers")
                .join("docker")
                .join("dbt-core")
                .join("Dockerfile");
            let base_setup_config = ContainerConfig {
                image_name_base: "dbt-record".to_string(),
                image_uri: None,
                dockerfile_path: Some(path_to_dbt_dockerfile),
                ro_mount_paths: vec![],
                rw_mount_path: Some(MountPoint {
                    local_path: project_env.absolute_project_dir.clone(),
                    container_path: PathBuf::from("/usr/app/dbt"),
                }),
                port_bindings: HashMap::new(),
                network_mode: Some("host".to_string()),
                reuse_latest: false,
                container_id: None,
                cmd: None,
                env: vec![],
                build_args: vec![("dbt_third_party".to_string(), "dbt-postgres".to_string())],
                bind_user: true,
            };
            for setup_cmd in self.setup_cmds.iter() {
                let mut config = base_setup_config.clone();
                let setup_cmd_args = setup_cmd
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>();
                config.cmd = Some(setup_cmd_args);
                let container = initialize_container(config).await?;
                wait_for_container_stop(&container.name).await?;
            }
            let mut base_record_config = base_setup_config.clone();
            base_record_config.env = vec![
                ("DBT_RECORDER_TYPES".to_string(), "Available".to_string()),
                ("DBT_RECORDER_MODE".to_string(), "RECORD".to_string()),
            ];

            for (goldie_index, record_cmd) in (task_index..).zip(self.record_cmds.iter()) {
                let mut config = base_record_config.clone();
                let record_cmd_args = record_cmd
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>();
                config.cmd = Some(record_cmd_args);
                let container = initialize_container(config).await?;
                wait_for_container_stop(&container.name).await?;
                // Add a sleep to ensure the container has time to write the recordings & manifest.json to disk
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                // Get recordings & manifest.json from workspace
                let recordings =
                    stdfs::read_to_string(project_env.absolute_project_dir.join("recording.json"))?;
                let manifest = stdfs::read_to_string(
                    project_env
                        .absolute_project_dir
                        .join(DBT_TARGET_DIR_NAME)
                        .join(DBT_MANIFEST_JSON),
                )?;
                // Write recordings & manifest.json to golden files
                let recordings_path = test_env
                    .golden_dir
                    .join(format!("recording_{goldie_index}.json"));
                let manifest_path = test_env
                    .golden_dir
                    .join(format!("manifest_{goldie_index}.json"));
                stdfs::write(recordings_path, recordings)?;
                stdfs::write(manifest_path, manifest)?;
            }
        }
        Ok(())
    }
}
