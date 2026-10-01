use bollard::errors::Error as BollardError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ContainerError {
    #[error("{0}")]
    ConfigError(String),
    #[error("{0}")]
    BollardError(String),
}

impl From<bollard::errors::Error> for ContainerError {
    fn from(err: bollard::errors::Error) -> Self {
        match err {
            BollardError::HyperResponseError { err } => {
                let err_msg = err.to_string();
                if err.is_canceled() {
                    if err_msg.contains("Permission denied") {
                        return ContainerError::BollardError("Permission denied when connecting to docker. \
                            Your user may not be in the docker group. To add your user to the docker group, \
                            run: 'sudo usermod -aG docker $USER'. You may need to log out and log back in for \
                            this to take effect.".to_string());
                    } else {
                        return ContainerError::BollardError(format!(
                            "Error connecting to docker {err_msg}. It's possible you do not have a valid '/var/run/docker.sock' socked available or configured."
                        ));
                    }
                }
                ContainerError::BollardError(format!("Error connecting to docker: {err_msg}"))
            }
            BollardError::DockerResponseServerError {
                status_code,
                message,
            } => {
                if status_code.eq(&404) {
                    ContainerError::BollardError(format!("Unable to find docker image: {message}"))
                } else {
                    ContainerError::BollardError(format!("Docker Service Error: {message}"))
                }
            }
            _ => ContainerError::BollardError(format!("Docker error: {err:?}")),
        }
    }
}
