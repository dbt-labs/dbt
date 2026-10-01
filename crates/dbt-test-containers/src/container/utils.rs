use super::error::ContainerError;
use super::{MIN_DOCKER_DESKTOP_VERSION, MIN_DOCKER_VERSION};
use bollard::errors::Error as BollardError;
use bollard::service::BuildInfo;
use dbt_common::stdfs::File;
use futures_core::Stream;
use futures_util::stream::StreamExt;
use regex::Regex;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use tar::Builder;
use walkdir::WalkDir;

const STEP_COUNTER_STR: &str = "Step";

/// Iterates over build output, awaiting the build to complete
pub async fn await_build_progress(
    build_image_stream: &mut (impl Stream<Item = Result<BuildInfo, BollardError>> + Unpin),
) -> Result<(), BollardError> {
    let mut current_progress: usize = 0;
    let mut progress_length: usize = 0;
    while let Some(build_image_result) = build_image_stream.next().await {
        match build_image_result {
            Ok(BuildInfo { stream, .. }) => {
                if let Some(stream) = stream
                    && stream.contains(STEP_COUNTER_STR)
                {
                    if progress_length > current_progress {
                        current_progress += 1;
                    } else if let Some(parsed_len) = get_progress_length(&stream) {
                        progress_length = parsed_len as usize;
                    }
                }
            }
            Err(e) => return Err(e)?,
        }
    }
    Ok(())
}

/// Checks a docker output message for a r'Step' indicator, returning the total number of steps
pub fn get_progress_length(msg: &str) -> Option<u64> {
    let re = Regex::new(r"Step (\d+)/(\d+)").unwrap();
    if let Some(captures) = re.captures(msg)
        && let Some(m) = captures.get(2)
    {
        return Some(m.as_str().parse::<u64>().unwrap());
    }
    None
}

/// Creates a tarball of the docker context directory
///
/// This is used when building containers from scratch, because the context
/// directory is sent to the docker daemon to be built
///
/// This also attempts to respect the .dockerignore file, but this filter
/// is unfinished / untested
pub fn create_docker_context_tarball<P: AsRef<Path>>(dir: P) -> Result<Vec<u8>, ContainerError> {
    // TODO: Find a more mature .dockerignore parser
    let ignore_path = dir.as_ref().join(".dockerignore");
    let mut ignore_patterns = Vec::new();

    if ignore_path.exists() {
        let file = File::open(ignore_path).unwrap();
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line = line.unwrap();
            if !line.trim().is_empty() && !line.starts_with('#') {
                ignore_patterns.push(line);
            }
        }
    }

    let mut tar = Builder::new(Vec::new());

    for entry in WalkDir::new(&dir) {
        let entry = entry.unwrap();
        let path = entry.path();

        // Check against .dockerignore patterns
        if path.is_dir()
            || ignore_patterns
                .iter()
                .any(|pattern| path.ends_with(pattern))
        {
            continue;
        }

        tar.append_path_with_name(path, path.strip_prefix(&dir).unwrap())
            .unwrap();
    }

    let mut c = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());

    let uncompressed = tar.into_inner().unwrap();

    c.write_all(&uncompressed).unwrap();

    Ok(c.finish().unwrap())
}

pub async fn get_docker() -> Result<bollard::Docker, ContainerError> {
    let docker = bollard::Docker::connect_with_local_defaults()?;
    // If this fails, we should continue
    docker.version().await?;

    if let Ok(docker_version) = docker.version().await {
        if let Some(platform) = &docker_version.platform {
            let re = Regex::new(r"Docker Desktop (\d+\.\d+\.\d+)").unwrap();
            if let Some(captures) = re.captures(&platform.name)
                && let Some(m) = captures.get(1)
            {
                match compare_versions(m.as_str(), MIN_DOCKER_DESKTOP_VERSION) {
                    std::cmp::Ordering::Greater | std::cmp::Ordering::Equal => {}
                    std::cmp::Ordering::Less => {
                        let target: &str = &format!(
                            "Docker Desktop version is less than the minimum supported version. Consider upgrading to at least '{MIN_DOCKER_DESKTOP_VERSION}'"
                        );
                        println!("{target}");
                        return Ok(docker);
                    }
                }
            }
        }
        if let Some(daemon_version) = docker_version.version {
            match compare_versions(&daemon_version, MIN_DOCKER_VERSION) {
                std::cmp::Ordering::Greater | std::cmp::Ordering::Equal => {}
                std::cmp::Ordering::Less => {
                    let target: &str = &format!(
                        "Docker daemon version is less than the minimum supported version. Consider upgrading Docker daemon to at least '{MIN_DOCKER_VERSION}'"
                    );
                    println!("{target}");
                }
            }
        }
    }
    Ok(docker)
}

fn compare_versions(version1: &str, version2: &str) -> std::cmp::Ordering {
    let v1_parts: Vec<u32> = version1.split('.').filter_map(|s| s.parse().ok()).collect();
    let v2_parts: Vec<u32> = version2.split('.').filter_map(|s| s.parse().ok()).collect();

    v1_parts.cmp(&v2_parts)
}
