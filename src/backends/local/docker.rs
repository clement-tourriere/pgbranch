use std::collections::HashMap;
use std::{collections::HashSet, path::PathBuf, time::Duration};

use anyhow::{anyhow, Context};
use bollard::exec::StartExecOptions;
use bollard::models::{
    ContainerCreateBody, ContainerStateStatusEnum, ExecConfig, HostConfig, PortBinding, PortMap,
};
use bollard::query_parameters::{
    CreateContainerOptions, CreateImageOptions, ListContainersOptions, RemoveContainerOptions,
    StopContainerOptions,
};
use bollard::Docker;
use futures_util::TryStreamExt;
use tokio::time::{sleep, Instant};

const PGDATA_CONTAINER_PATH: &str = "/var/lib/postgresql/data";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerStatus {
    NotFound,
    Running,
    Paused,
    Exited,
    Other(String),
}

#[derive(Debug, Clone)]
pub struct ReserveBranchSpec {
    pub project_name: String,
    pub branch_name: String,
}

#[derive(Debug, Clone)]
pub struct ReservedBranchRuntime {
    pub container_name: String,
}

#[derive(Debug, Clone)]
pub struct StartBranchSpec {
    pub image: String,
    pub container_name: String,
    pub data_dir: PathBuf,
    pub port: u16,
    pub pg_user: String,
    pub pg_password: String,
    pub pg_db: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DockerDoctorResult {
    pub available: bool,
    pub detail: String,
    pub version: Option<String>,
}

pub struct DockerRuntime {
    client: Docker,
}

impl DockerRuntime {
    pub fn new() -> anyhow::Result<Self> {
        let client =
            Docker::connect_with_local_defaults().context("failed to connect to Docker daemon")?;
        Ok(Self { client })
    }

    pub fn client(&self) -> &Docker {
        &self.client
    }

    pub async fn doctor(&self) -> DockerDoctorResult {
        match self.client.version().await {
            Ok(info) => {
                let version = info.version.unwrap_or_default();
                DockerDoctorResult {
                    available: true,
                    detail: "Docker engine reachable".to_string(),
                    version: Some(version),
                }
            }
            Err(err) => DockerDoctorResult {
                available: false,
                detail: format!("Docker engine unreachable: {err}"),
                version: None,
            },
        }
    }

    pub async fn reserve_branch(
        &self,
        spec: &ReserveBranchSpec,
    ) -> anyhow::Result<ReservedBranchRuntime> {
        let raw = format!(
            "pgbranch-{}-{}",
            sanitize(&spec.project_name),
            sanitize(&spec.branch_name)
        );
        // Docker container names must be <= 128 chars
        let container_name = if raw.len() > 128 {
            raw[..128].trim_end_matches('-').to_string()
        } else {
            raw
        };

        Ok(ReservedBranchRuntime { container_name })
    }

    pub async fn ensure_image(&self, image: &str) -> anyhow::Result<()> {
        // Check if image exists locally
        if self.client.inspect_image(image).await.is_ok() {
            return Ok(());
        }

        // Parse image:tag
        let (from_image, tag) = if let Some((name, tag)) = image.rsplit_once(':') {
            (name.to_string(), Some(tag.to_string()))
        } else {
            (image.to_string(), None)
        };

        let options = CreateImageOptions {
            from_image: Some(from_image),
            tag,
            ..Default::default()
        };

        // Pull and consume the stream to completion
        self.client
            .create_image(Some(options), None, None)
            .try_collect::<Vec<_>>()
            .await
            .with_context(|| format!("failed to pull docker image '{image}'"))?;

        Ok(())
    }

    pub async fn container_status(&self, container_name: &str) -> anyhow::Result<ContainerStatus> {
        match self
            .client
            .inspect_container(
                container_name,
                None::<bollard::query_parameters::InspectContainerOptions>,
            )
            .await
        {
            Ok(info) => {
                let status = info.state.and_then(|s| s.status);
                match status {
                    Some(ContainerStateStatusEnum::RUNNING) => Ok(ContainerStatus::Running),
                    Some(ContainerStateStatusEnum::PAUSED) => Ok(ContainerStatus::Paused),
                    Some(ContainerStateStatusEnum::EXITED)
                    | Some(ContainerStateStatusEnum::CREATED) => Ok(ContainerStatus::Exited),
                    Some(other) => Ok(ContainerStatus::Other(other.to_string())),
                    None => Ok(ContainerStatus::Other("unknown".to_string())),
                }
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(ContainerStatus::NotFound),
            Err(err) => Err(anyhow!(
                "failed to inspect container '{container_name}': {err}"
            )),
        }
    }

    pub async fn start_branch(&self, spec: &StartBranchSpec) -> anyhow::Result<()> {
        self.ensure_image(&spec.image).await?;

        match self.container_status(&spec.container_name).await? {
            ContainerStatus::Running => return Ok(()),
            ContainerStatus::Paused => {
                self.unpause_branch(&spec.container_name).await?;
                return Ok(());
            }
            ContainerStatus::Exited | ContainerStatus::Other(_) => {
                self.client
                    .start_container(
                        &spec.container_name,
                        None::<bollard::query_parameters::StartContainerOptions>,
                    )
                    .await
                    .with_context(|| {
                        format!("failed to start container '{}'", spec.container_name)
                    })?;
                return Ok(());
            }
            ContainerStatus::NotFound => {}
        }

        // Create and start a new container
        let mount = format!("{}:{PGDATA_CONTAINER_PATH}", spec.data_dir.display());

        let mut port_bindings: PortMap = HashMap::new();
        port_bindings.insert(
            "5432/tcp".to_string(),
            Some(vec![PortBinding {
                host_ip: Some("0.0.0.0".to_string()),
                host_port: Some(spec.port.to_string()),
            }]),
        );

        let mut labels = HashMap::new();
        labels.insert("pgbranch.managed".to_string(), "true".to_string());

        let config = ContainerCreateBody {
            image: Some(spec.image.clone()),
            user: get_host_uid_gid(),
            env: Some(vec![
                format!("POSTGRES_USER={}", spec.pg_user),
                format!("POSTGRES_PASSWORD={}", spec.pg_password),
                format!("POSTGRES_DB={}", spec.pg_db),
            ]),
            labels: Some(labels),
            host_config: Some(HostConfig {
                binds: Some(vec![mount]),
                port_bindings: Some(port_bindings),
                ..Default::default()
            }),
            ..Default::default()
        };

        let options = CreateContainerOptions {
            name: Some(spec.container_name.clone()),
            ..Default::default()
        };

        self.client
            .create_container(Some(options), config)
            .await
            .with_context(|| format!("failed to create container '{}'", spec.container_name))?;

        self.client
            .start_container(
                &spec.container_name,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await
            .with_context(|| format!("failed to start container '{}'", spec.container_name))?;

        Ok(())
    }

    pub async fn stop_branch(&self, container_name: &str) -> anyhow::Result<()> {
        match self.container_status(container_name).await? {
            ContainerStatus::NotFound | ContainerStatus::Exited | ContainerStatus::Other(_) => {
                return Ok(())
            }
            ContainerStatus::Paused => {
                self.unpause_branch(container_name).await?;
            }
            ContainerStatus::Running => {}
        }

        let options = StopContainerOptions {
            t: Some(20),
            ..Default::default()
        };

        self.client
            .stop_container(container_name, Some(options))
            .await
            .with_context(|| format!("failed to stop container '{container_name}'"))?;

        Ok(())
    }

    pub async fn pause_branch(&self, container_name: &str) -> anyhow::Result<()> {
        match self.container_status(container_name).await? {
            ContainerStatus::Running => {}
            _ => return Ok(()),
        }

        self.client
            .pause_container(container_name)
            .await
            .with_context(|| format!("failed to pause container '{container_name}'"))?;

        Ok(())
    }

    pub async fn unpause_branch(&self, container_name: &str) -> anyhow::Result<()> {
        match self.container_status(container_name).await? {
            ContainerStatus::Paused => {}
            _ => return Ok(()),
        }

        self.client
            .unpause_container(container_name)
            .await
            .with_context(|| format!("failed to unpause container '{container_name}'"))?;

        Ok(())
    }

    pub async fn remove_branch(&self, container_name: &str) -> anyhow::Result<()> {
        if matches!(
            self.container_status(container_name).await?,
            ContainerStatus::NotFound
        ) {
            return Ok(());
        }

        let options = RemoveContainerOptions {
            force: true,
            ..Default::default()
        };

        self.client
            .remove_container(container_name, Some(options))
            .await
            .with_context(|| format!("failed to remove container '{container_name}'"))?;

        Ok(())
    }

    pub async fn wait_ready(
        &self,
        container_name: &str,
        pg_user: &str,
        pg_db: &str,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let deadline = Instant::now() + timeout;

        loop {
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "timed out waiting for postgres readiness in '{container_name}'"
                ));
            }

            match self.container_status(container_name).await? {
                ContainerStatus::NotFound => {
                    return Err(anyhow!("container '{container_name}' does not exist"));
                }
                ContainerStatus::Running => {
                    if self
                        .exec_check(container_name, &["pg_isready", "-U", pg_user, "-d", pg_db])
                        .await
                    {
                        return Ok(());
                    }
                }
                _ => {}
            }

            sleep(Duration::from_millis(500)).await;
        }
    }

    /// Run a command inside a container and return true if it exits successfully.
    async fn exec_check(&self, container_name: &str, cmd: &[&str]) -> bool {
        let config = ExecConfig {
            cmd: Some(cmd.iter().map(|s| s.to_string()).collect()),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            ..Default::default()
        };

        let exec = match self.client.create_exec(container_name, config).await {
            Ok(e) => e,
            Err(_) => return false,
        };

        let start_opts = Some(StartExecOptions {
            detach: false,
            ..Default::default()
        });

        // Must consume the output stream to completion before inspect_exec
        // will report the correct exit code
        match self.client.start_exec(&exec.id, start_opts).await {
            Ok(bollard::exec::StartExecResults::Attached { mut output, .. }) => {
                while output.try_next().await.ok().flatten().is_some() {}
            }
            Ok(bollard::exec::StartExecResults::Detached) => {}
            Err(_) => return false,
        }

        // Check exit code
        match self.client.inspect_exec(&exec.id).await {
            Ok(info) => info.exit_code == Some(0),
            Err(_) => false,
        }
    }
}

pub async fn pick_available_port(client: &Docker, start_port: u16) -> anyhow::Result<u16> {
    let docker_ports = docker_published_ports(client).await;
    let mut port = start_port;

    for _ in 0..1000 {
        if docker_ports.contains(&port) {
            port = port.saturating_add(1);
            if port == u16::MAX {
                break;
            }
            continue;
        }

        if is_port_available(port).await {
            return Ok(port);
        }

        port = port.saturating_add(1);
        if port == u16::MAX {
            break;
        }
    }

    Err(anyhow!(
        "failed to find available port starting from {start_port}"
    ))
}

async fn is_port_available(port: u16) -> bool {
    if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        drop(listener);
        return true;
    }
    false
}

async fn docker_published_ports(client: &Docker) -> HashSet<u16> {
    let options = ListContainersOptions {
        all: false,
        ..Default::default()
    };

    let containers = match client.list_containers(Some(options)).await {
        Ok(c) => c,
        Err(_) => return HashSet::new(),
    };

    let mut ports = HashSet::new();
    for container in containers {
        if let Some(port_list) = container.ports {
            for port in port_list {
                if let Some(public_port) = port.public_port {
                    ports.insert(public_port);
                }
            }
        }
    }

    ports
}

/// Returns the current host user's UID:GID as a string (e.g. "1000:1000").
/// This ensures files created inside the container are owned by the host user,
/// allowing `cp` operations on the bind-mounted pgdata directory to succeed.
#[cfg(unix)]
fn get_host_uid_gid() -> Option<String> {
    let uid = std::process::Command::new("id").arg("-u").output().ok()?;
    let gid = std::process::Command::new("id").arg("-g").output().ok()?;
    if uid.status.success() && gid.status.success() {
        let u = String::from_utf8_lossy(&uid.stdout).trim().to_string();
        let g = String::from_utf8_lossy(&gid.stdout).trim().to_string();
        Some(format!("{}:{}", u, g))
    } else {
        None
    }
}

#[cfg(not(unix))]
fn get_host_uid_gid() -> Option<String> {
    None
}

fn sanitize(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
        } else {
            output.push('-');
        }
    }

    while output.contains("--") {
        output = output.replace("--", "-");
    }

    let trimmed = output.trim_matches('-').to_string();
    if trimmed.is_empty() {
        return "project".to_string();
    }
    trimmed
}
