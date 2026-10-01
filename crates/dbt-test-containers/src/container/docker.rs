use super::error::ContainerError;
use super::utils::{await_build_progress, create_docker_context_tarball, get_docker};
use bollard::Docker;
use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, LogsOptions, WaitContainerOptions,
};
use bollard::image::{BuildImageOptions, CreateImageOptions, ListImagesOptions};
use bollard::service::HostConfig;
use futures_util::stream::StreamExt;
use std::collections::HashMap;
use std::path::PathBuf;

pub use bollard::secret::PortBinding;

pub const MAX_STARTUP_ATTEMPTS: u16 = 5;

#[derive(Debug, Clone)]
#[allow(unused)]
pub struct Container {
    pub name: String,
    pub output_mount_path: Option<PathBuf>,
    pub container_id: String,
    pub digest: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MountPoint {
    pub local_path: PathBuf,
    pub container_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ContainerConfig {
    pub image_name_base: String,
    pub image_uri: Option<String>,
    pub dockerfile_path: Option<PathBuf>,
    pub ro_mount_paths: Vec<MountPoint>,
    pub rw_mount_path: Option<MountPoint>,
    pub port_bindings: HashMap<String, Option<Vec<PortBinding>>>,
    pub network_mode: Option<String>,
    pub reuse_latest: bool,
    pub container_id: Option<String>,
    pub cmd: Option<Vec<String>>,
    pub env: Vec<(String, String)>,
    pub build_args: Vec<(String, String)>,
    pub bind_user: bool,
}

/// Initialize Container
///
/// This is the main entrypoint for starting a container.
///
/// If a dockerfile is provided, it will be built and run. If an image URI is provided, it will be
/// either sourced locally or pulled from remote and run.
///
/// The container expects a set of ro_mount_paths (i.e. read only absolute paths) which point to folders that
/// must be made available to the container as well as an output_mount_path which is a read-write path.
#[allow(clippy::cognitive_complexity)]
pub async fn initialize_container(config: ContainerConfig) -> Result<Container, ContainerError> {
    let ContainerConfig {
        image_name_base,
        image_uri,
        dockerfile_path,
        ro_mount_paths,
        rw_mount_path: output_mount_path,
        port_bindings,
        network_mode,
        reuse_latest,
        container_id,
        cmd,
        env,
        build_args,
        bind_user,
    } = config;

    let docker = get_docker().await?;
    if cfg!(windows) {
        return Err(ContainerError::ConfigError(
            "Docker Container on Windows is not currently supported".to_string(),
        ));
    }
    // --------------------------------------------------------
    // Check for existing container
    // --------------------------------------------------------
    let random_tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    let new_image_name_tag = format!("fs-{image_name_base}-{random_tag}");
    let check_prefix = format!("/fs-{image_name_base}");
    let mut existing_containers = docker
        .list_containers::<String>(Some(ListContainersOptions::<String> {
            filters: vec![("name".to_string(), vec![check_prefix.to_string()])]
                .into_iter()
                .collect(),
            ..Default::default()
        }))
        .await?;
    let mut existing_container = None;
    let image_name_tag = if !existing_containers.is_empty() {
        if existing_containers.len() > 1 {
            println!(
                "Found {} running containers for '{}'. Stopping all but latest.",
                existing_containers.len(),
                image_name_base
            );
        } else {
            println!(
                "Found {} running container for '{}'.",
                existing_containers.len(),
                image_name_base
            );
        }
        let container_id =
            if let Some(latest_container) = existing_containers.iter().max_by_key(|c| c.created) {
                if reuse_latest {
                    latest_container.id.clone()
                } else {
                    container_id.clone()
                }
            } else {
                container_id.clone()
            };
        for container in existing_containers.drain(..) {
            if reuse_latest && container.id == container_id {
                existing_container = Some(container);
            } else if let Some(container_id) = &container.id {
                shutdown_container(container_id).await?;
            }
        }
        if let Some(container) = &existing_container {
            container
                .id
                .clone()
                .expect("Container ID not found for running container")
        } else {
            new_image_name_tag
        }
    } else {
        new_image_name_tag
    };
    let container_id = match existing_container {
        Some(container) => container
            .id
            .expect("Container ID not found for running container"),
        None => {
            // Compute paths that will be mounted into the container
            let image_name = match (image_uri, dockerfile_path) {
                // --------------------------------------------------------
                // Reading from local or remote image
                // --------------------------------------------------------
                (Some(image_uri), None) => {
                    println!("local image cache for '{image_uri}'");
                    let check_local_images = &docker
                        .list_images(Some(ListImagesOptions::<String> {
                            filters: HashMap::from([(
                                "before".to_string(),
                                vec![image_uri.to_string()],
                            )]),
                            ..Default::default()
                        }))
                        .await;
                    let options = Some(CreateImageOptions {
                        from_image: image_uri.clone(),
                        ..Default::default()
                    });

                    let mut create_image_stream = docker.create_image(options, None, None);
                    if check_local_images.is_err() {
                        println!("Downloading image '{image_name_base}' from '{image_uri}'");
                    } else {
                        println!("Refreshing image '{image_name_base}' from '{image_uri}'");
                    }
                    while let Some(create_image_result) = create_image_stream.next().await {
                        match create_image_result {
                            Ok(_) => {}
                            Err(e) => return Err(e)?,
                        }
                    }

                    println!("Loaded image '{image_uri}'");
                    image_uri
                }
                // --------------------------------------------------------
                // Building from dockerfile
                // --------------------------------------------------------
                (None, Some(dockerfile_path)) => {
                    // Assume dockerfile is in the same context directory
                    let dockerfile = dockerfile_path
                        .file_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_string();
                    let context_dir = dockerfile_path.parent().unwrap();
                    let options: BuildImageOptions<String> = BuildImageOptions {
                        dockerfile,
                        buildargs: build_args.into_iter().collect(),
                        t: image_name_base.clone(),
                        ..Default::default()
                    };
                    let tar = create_docker_context_tarball(context_dir)?;
                    let mut build_image_stream =
                        docker.build_image(options, None, Some(tar.into()));
                    await_build_progress(&mut build_image_stream).await?;
                    println!("building container '{}'", &image_name_base);
                    image_name_base.clone()
                }
                _ => {
                    return Err(ContainerError::ConfigError(
                        "Must specify one of image_uri or dockerfile".to_string(),
                    ));
                }
            };
            // --------------------------------------------------------
            // Configure Container
            // --------------------------------------------------------

            ro_mount_paths
                .iter()
                .find_map(|p| {
                    if !p.local_path.is_absolute() || !p.local_path.is_dir() {
                        Some(p.local_path.to_str().unwrap().to_string())
                    } else {
                        None
                    }
                })
                .map_or(Ok(()), |p| {
                    Err(ContainerError::ConfigError(format!(
                        "Mount point '{p}' must be an absolute path and a directory"
                    )))
                })?;

            let mut binds: Vec<String> = ro_mount_paths
                .into_iter()
                .map(|p| {
                    format!(
                        "{}:{}:ro",
                        p.local_path.to_str().unwrap(),
                        p.container_path.to_str().unwrap(),
                    )
                })
                .collect();
            if let Some(output_mount_path) = &output_mount_path {
                binds.push(format!(
                    "{}:{}:rw",
                    output_mount_path.local_path.to_str().unwrap(),
                    output_mount_path.container_path.to_str().unwrap()
                ));
            }
            let mut startup_attempts = 0;
            let mut container_id = None;
            loop {
                let host_config = HostConfig {
                    binds: Some(binds.clone()),
                    port_bindings: Some(port_bindings.clone()),
                    network_mode: network_mode.clone(),
                    ..Default::default()
                };
                let user_string = if bind_user {
                    #[cfg(not(windows))]
                    {
                        let uid = uzers::get_current_uid();
                        let gid = uzers::get_current_gid();
                        Some(format!("{uid}:{gid}"))
                    }
                    #[cfg(windows)]
                    None
                } else {
                    None
                };

                let container_config: Config<String> = Config {
                    image: Some(image_name.clone()),
                    user: user_string,
                    tty: Some(true),
                    attach_stdin: Some(true),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    open_stdin: Some(true),
                    host_config: Some(host_config),
                    env: Some(
                        env.iter()
                            .map(|(key, value)| format!("{key}={value}"))
                            .collect(),
                    ),
                    cmd: cmd.clone(),
                    ..Default::default()
                };
                // --------------------------------------------------------
                // Starting Container
                // --------------------------------------------------------

                let options: CreateContainerOptions<String> = CreateContainerOptions {
                    name: image_name_tag.clone(),
                    ..Default::default()
                };
                match docker
                    .create_container::<String, String>(Some(options), container_config)
                    .await
                {
                    Ok(container_res) => {
                        println!("Loaded container '{}'", &image_name_tag);
                        println!("container ID: '{}'", &container_res.id);
                        match docker
                            .start_container::<String>(&container_res.id, None)
                            .await
                        {
                            Ok(_) => {
                                container_id = Some(container_res.id);
                                println!("Started container '{}'", &image_name_tag);
                                break;
                            }
                            Err(e) => {
                                println!("Error starting container: {e:?}");
                                if startup_attempts < MAX_STARTUP_ATTEMPTS {
                                    startup_attempts += 1;
                                    continue;
                                } else {
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        println!("Error creating container: {e:?}");
                        if startup_attempts < MAX_STARTUP_ATTEMPTS {
                            startup_attempts += 1;
                            continue;
                        } else {
                            break;
                        }
                    }
                }
            }
            if let Some(container_id) = container_id {
                container_id
            } else {
                return Err(ContainerError::BollardError(format!(
                    "Unable to start container '{image_name_base}'. Exceeded Max Startup Attempts: {MAX_STARTUP_ATTEMPTS}"
                )));
            }
        }
    };

    Ok(Container {
        container_id,
        output_mount_path: output_mount_path.map(|p| p.local_path),
        name: image_name_tag,
        digest: None,
    })
}

// Wait for container to finish
pub async fn wait_for_container_stop(container_name: &str) -> Result<(), ContainerError> {
    let docker = Docker::connect_with_local_defaults().unwrap();
    // Check if container is running first
    let container_inspect = docker.inspect_container(container_name, None).await?;
    if let Some(state) = container_inspect.state
        && !state.running.unwrap_or(false)
    {
        // Container is not running
        return Ok(());
    }
    let options = Some(WaitContainerOptions {
        condition: "not-running",
    });
    let mut stream = docker.wait_container(container_name, options);
    while let Some(stream) = stream.next().await {
        match stream {
            Ok(_) => {}
            Err(e) => match e {
                bollard::errors::Error::DockerContainerWaitError { error, .. } => {
                    return Err(ContainerError::BollardError(format!(
                        "Container wait error: {error}"
                    )));
                }
                _ => {
                    return Err(ContainerError::BollardError(e.to_string()));
                }
            },
        }
    }
    Ok(())
}

// Fetch container logs
pub async fn fetch_container_logs(container_name: &str) -> Result<Vec<String>, ContainerError> {
    let docker = Docker::connect_with_local_defaults().unwrap();
    let mut logs = vec![];
    let mut logs_stream = docker.logs(
        container_name,
        Some(LogsOptions::<String> {
            follow: false,
            stdout: true,
            stderr: true,
            ..Default::default()
        }),
    );
    while let Some(log) = logs_stream.next().await {
        logs.push(
            log.map_err(|e| ContainerError::BollardError(e.to_string()))?
                .to_string(),
        );
    }
    Ok(logs)
}

/// Shutdown Container
pub async fn shutdown_container(container_name: &str) -> Result<(), ContainerError> {
    let id = container_name.to_string();
    let docker = Docker::connect_with_local_defaults().unwrap();
    let _ = tokio::task::spawn(async move {
        docker.stop_container(&id, None).await.unwrap();
    })
    .await;
    Ok(())
}
