pub mod docker;
pub mod model;
pub mod reconcile;
pub mod seed;
pub mod state;
pub mod storage;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use uuid::Uuid;

use super::{
    BranchInfo, ConnectionInfo, DatabaseBranchingBackend, DoctorCheck, DoctorReport, ProjectInfo,
};
use crate::config::{Config, LocalBackendConfig};
use docker::{DockerRuntime, ReserveBranchSpec, StartBranchSpec};
use model::BranchState;
use state::{NewBranch, NewProject, Store};
use storage::StorageCoordinator;

const DEFAULT_IMAGE: &str = "postgres:17";
const DEFAULT_PORT_RANGE_START: u16 = 55432;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);

pub struct LocalBackend {
    project_name: String,
    image: String,
    port_range_start: u16,
    pg_user: String,
    pg_password: String,
    pg_db: String,
    store: Mutex<Store>,
    runtime: DockerRuntime,
    storage: StorageCoordinator,
    data_root: PathBuf,
}

impl LocalBackend {
    pub async fn new(
        backend_name: &str,
        _config: &Config,
        local_config: Option<&LocalBackendConfig>,
    ) -> Result<Self> {
        let image = local_config
            .and_then(|c| c.image.as_deref())
            .unwrap_or(DEFAULT_IMAGE)
            .to_string();

        let port_range_start = local_config
            .and_then(|c| c.port_range_start)
            .unwrap_or(DEFAULT_PORT_RANGE_START);

        let pg_user = local_config
            .and_then(|c| c.postgres_user.as_deref())
            .unwrap_or("postgres")
            .to_string();

        let pg_password = local_config
            .and_then(|c| c.postgres_password.as_deref())
            .unwrap_or("postgres")
            .to_string();

        let pg_db = local_config
            .and_then(|c| c.postgres_db.as_deref())
            .unwrap_or("postgres")
            .to_string();

        let data_root = if let Some(root) = local_config.and_then(|c| c.data_root.as_deref()) {
            let expanded = shellexpand(root);
            PathBuf::from(expanded)
        } else {
            dirs::data_local_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
                .join("pgbranch")
        };

        // Ensure directories exist
        let projects_root = data_root.join("projects");
        tokio::fs::create_dir_all(&projects_root)
            .await
            .with_context(|| {
                format!(
                    "failed to create projects root: {}",
                    projects_root.display()
                )
            })?;

        let db_path = data_root.join("state.db");
        let store = Store::open(&db_path)
            .with_context(|| format!("failed to open state database: {}", db_path.display()))?;

        let runtime = DockerRuntime::new().context("failed to initialize Docker runtime")?;
        let storage = StorageCoordinator::new(projects_root.clone());

        let project_name = backend_name.to_string();

        Ok(Self {
            project_name,
            image,
            port_range_start,
            pg_user,
            pg_password,
            pg_db,
            store: Mutex::new(store),
            runtime,
            storage,
            data_root,
        })
    }

    fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap()
    }

    async fn ensure_project(&self) -> Result<model::Project> {
        if let Some(project) = self.store().get_project_by_name(&self.project_name)? {
            return Ok(project);
        }

        // Auto-create project
        let selection = self.storage.select_for_new_project().await;

        let project = self.store().create_project(NewProject {
            name: self.project_name.clone(),
            image: self.image.clone(),
            storage_backend: selection.backend,
            storage_config: selection.config,
        })?;

        log::info!(
            "Auto-created project '{}' with {} storage",
            self.project_name,
            project.storage_backend.as_str()
        );
        Ok(project)
    }

    async fn reconcile_project(&self, project: &model::Project) -> Result<()> {
        // Read branches from store (sync, releases lock before await)
        let branches = self.store().list_branches(&project.id)?;

        // Compute state changes (async, no store reference held)
        let changes = reconcile::compute_state_changes(&self.runtime, branches).await;

        // Apply changes (sync)
        if !changes.is_empty() {
            let store = self.store();
            for (branch_id, new_state) in changes {
                store.update_branch_state(&branch_id, new_state)?;
            }
        }

        Ok(())
    }

    fn connection_uri(&self, port: u16) -> String {
        format!(
            "postgresql://{}:{}@127.0.0.1:{}/{}",
            self.pg_user, self.pg_password, port, self.pg_db
        )
    }
}

#[async_trait]
impl DatabaseBranchingBackend for LocalBackend {
    async fn create_branch(
        &self,
        branch_name: &str,
        from_branch: Option<&str>,
    ) -> Result<BranchInfo> {
        let project = self.ensure_project().await?;
        self.reconcile_project(&project).await?;

        // Check if branch already exists
        if let Some(existing) = self.store().get_branch_by_name(&project.id, branch_name)? {
            if existing.state == BranchState::Running {
                return Ok(BranchInfo {
                    name: existing.name,
                    created_at: None,
                    parent_branch: None,
                    database_name: self.pg_db.clone(),
                    state: Some(existing.state.as_str().to_string()),
                });
            }
        }

        let branch_id = Uuid::new_v4().to_string();
        let data_dir = self
            .data_root
            .join("projects")
            .join(&project.id)
            .join("branches")
            .join(&branch_id)
            .join("pgdata");

        // Reserve container name and find port
        let reserved = self
            .runtime
            .reserve_branch(&ReserveBranchSpec {
                project_name: self.project_name.clone(),
                branch_name: branch_name.to_string(),
            })
            .await?;

        let start_port = self.store().next_port()?.max(self.port_range_start);
        let port = docker::pick_available_port(self.runtime.client(), start_port).await?;

        // Clone or create empty
        let parent = if let Some(from_name) = from_branch {
            self.store().get_branch_by_name(&project.id, from_name)?
        } else {
            // Try to clone from most recent branch
            let branches = self.store().list_branches(&project.id)?;
            branches
                .into_iter()
                .find(|b| b.state == BranchState::Running || b.state == BranchState::Stopped)
        };

        let storage_metadata = if let Some(ref parent_branch) = parent {
            // Pause parent if running
            let parent_running = self
                .runtime
                .container_status(&parent_branch.container_name)
                .await?
                == docker::ContainerStatus::Running;

            if parent_running {
                self.runtime
                    .pause_branch(&parent_branch.container_name)
                    .await?;
            }

            let result = self
                .storage
                .clone_branch_from_parent(&project, parent_branch, &branch_id, &data_dir)
                .await;

            if parent_running {
                self.runtime
                    .unpause_branch(&parent_branch.container_name)
                    .await?;
            }

            result?
        } else {
            self.storage
                .create_empty_branch(&project, &branch_id, &data_dir)
                .await?
        };

        // Persist to state
        let branch = self.store().create_branch(NewBranch {
            id: branch_id,
            project_id: project.id.clone(),
            name: branch_name.to_string(),
            parent_branch_id: parent.as_ref().map(|p| p.id.clone()),
            state: BranchState::Provisioning,
            data_dir: data_dir.to_string_lossy().to_string(),
            container_name: reserved.container_name.clone(),
            port,
            storage_metadata,
        })?;

        // Start container
        self.runtime
            .start_branch(&StartBranchSpec {
                image: project.image.clone(),
                container_name: reserved.container_name.clone(),
                data_dir,
                port,
                pg_user: self.pg_user.clone(),
                pg_password: self.pg_password.clone(),
                pg_db: self.pg_db.clone(),
            })
            .await?;

        // Wait for readiness
        self.runtime
            .wait_ready(
                &reserved.container_name,
                &self.pg_user,
                &self.pg_db,
                STARTUP_TIMEOUT,
            )
            .await?;

        // Update state
        self.store()
            .update_branch_state(&branch.id, BranchState::Running)?;

        Ok(BranchInfo {
            name: branch_name.to_string(),
            created_at: Some(Utc::now()),
            parent_branch: parent.as_ref().map(|p| p.name.clone()),
            database_name: self.pg_db.clone(),
            state: Some("running".to_string()),
        })
    }

    async fn delete_branch(&self, branch_name: &str) -> Result<()> {
        let project = self.ensure_project().await?;

        let branch = self
            .store()
            .get_branch_by_name(&project.id, branch_name)?
            .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", branch_name))?;

        // Remove container
        self.runtime.remove_branch(&branch.container_name).await?;

        // Delete storage data
        self.storage.delete_branch_data(&project, &branch).await?;

        // Delete from state
        self.store().delete_branch(&branch.id)?;

        Ok(())
    }

    async fn list_branches(&self) -> Result<Vec<BranchInfo>> {
        let project = self.ensure_project().await?;
        self.reconcile_project(&project).await?;

        let branches = self.store().list_branches(&project.id)?;

        // Build id→name map so we can resolve parent_branch_id to a name
        let id_to_name: std::collections::HashMap<&str, &str> = branches
            .iter()
            .map(|b| (b.id.as_str(), b.name.as_str()))
            .collect();

        Ok(branches
            .iter()
            .map(|b| BranchInfo {
                name: b.name.clone(),
                created_at: None,
                parent_branch: b
                    .parent_branch_id
                    .as_deref()
                    .and_then(|pid| id_to_name.get(pid))
                    .map(|name| name.to_string()),
                database_name: self.pg_db.clone(),
                state: Some(b.state.as_str().to_string()),
            })
            .collect())
    }

    async fn branch_exists(&self, branch_name: &str) -> Result<bool> {
        let project = self.ensure_project().await?;
        Ok(self
            .store()
            .get_branch_by_name(&project.id, branch_name)?
            .is_some())
    }

    async fn switch_to_branch(&self, branch_name: &str) -> Result<BranchInfo> {
        let project = self.ensure_project().await?;
        self.reconcile_project(&project).await?;

        let branch = self
            .store()
            .get_branch_by_name(&project.id, branch_name)?
            .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", branch_name))?;

        // Start if stopped
        if branch.state == BranchState::Stopped {
            self.runtime
                .start_branch(&StartBranchSpec {
                    image: project.image.clone(),
                    container_name: branch.container_name.clone(),
                    data_dir: PathBuf::from(&branch.data_dir),
                    port: branch.port,
                    pg_user: self.pg_user.clone(),
                    pg_password: self.pg_password.clone(),
                    pg_db: self.pg_db.clone(),
                })
                .await?;

            self.runtime
                .wait_ready(
                    &branch.container_name,
                    &self.pg_user,
                    &self.pg_db,
                    STARTUP_TIMEOUT,
                )
                .await?;
            self.store()
                .update_branch_state(&branch.id, BranchState::Running)?;
        }

        Ok(BranchInfo {
            name: branch.name,
            created_at: None,
            parent_branch: None,
            database_name: self.pg_db.clone(),
            state: Some("running".to_string()),
        })
    }

    async fn get_connection_info(&self, branch_name: &str) -> Result<ConnectionInfo> {
        let project = self.ensure_project().await?;

        let branch = self
            .store()
            .get_branch_by_name(&project.id, branch_name)?
            .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", branch_name))?;

        Ok(ConnectionInfo {
            host: "127.0.0.1".to_string(),
            port: branch.port,
            database: self.pg_db.clone(),
            user: self.pg_user.clone(),
            password: Some(self.pg_password.clone()),
            connection_string: Some(self.connection_uri(branch.port)),
        })
    }

    async fn start_branch(&self, branch_name: &str) -> Result<()> {
        let project = self.ensure_project().await?;

        let branch = self
            .store()
            .get_branch_by_name(&project.id, branch_name)?
            .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", branch_name))?;

        self.runtime
            .start_branch(&StartBranchSpec {
                image: project.image.clone(),
                container_name: branch.container_name.clone(),
                data_dir: PathBuf::from(&branch.data_dir),
                port: branch.port,
                pg_user: self.pg_user.clone(),
                pg_password: self.pg_password.clone(),
                pg_db: self.pg_db.clone(),
            })
            .await?;

        self.runtime
            .wait_ready(
                &branch.container_name,
                &self.pg_user,
                &self.pg_db,
                STARTUP_TIMEOUT,
            )
            .await?;
        self.store()
            .update_branch_state(&branch.id, BranchState::Running)?;

        Ok(())
    }

    async fn stop_branch(&self, branch_name: &str) -> Result<()> {
        let project = self.ensure_project().await?;

        let branch = self
            .store()
            .get_branch_by_name(&project.id, branch_name)?
            .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", branch_name))?;

        self.runtime.stop_branch(&branch.container_name).await?;
        self.store()
            .update_branch_state(&branch.id, BranchState::Stopped)?;

        Ok(())
    }

    async fn reset_branch(&self, branch_name: &str) -> Result<()> {
        let project = self.ensure_project().await?;

        let branch = self
            .store()
            .get_branch_by_name(&project.id, branch_name)?
            .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", branch_name))?;

        let was_running = branch.state == BranchState::Running;

        // Stop container
        self.runtime.stop_branch(&branch.container_name).await?;

        // Re-clone from parent if available
        if let Some(parent_id) = &branch.parent_branch_id {
            let parent = self
                .store()
                .list_branches(&project.id)?
                .into_iter()
                .find(|b| &b.id == parent_id);

            if let Some(parent_branch) = parent {
                let parent_running = self
                    .runtime
                    .container_status(&parent_branch.container_name)
                    .await?
                    == docker::ContainerStatus::Running;

                if parent_running {
                    self.runtime
                        .pause_branch(&parent_branch.container_name)
                        .await?;
                }

                let data_dir = PathBuf::from(&branch.data_dir);
                let new_metadata = self
                    .storage
                    .clone_branch_from_parent(&project, &parent_branch, &branch.id, &data_dir)
                    .await?;

                if parent_running {
                    self.runtime
                        .unpause_branch(&parent_branch.container_name)
                        .await?;
                }

                if let Some(metadata) = &new_metadata {
                    self.store()
                        .update_branch_storage_metadata(&branch.id, Some(metadata))?;
                }
            }
        }

        // Restart if it was running
        if was_running {
            self.runtime
                .start_branch(&StartBranchSpec {
                    image: project.image.clone(),
                    container_name: branch.container_name.clone(),
                    data_dir: PathBuf::from(&branch.data_dir),
                    port: branch.port,
                    pg_user: self.pg_user.clone(),
                    pg_password: self.pg_password.clone(),
                    pg_db: self.pg_db.clone(),
                })
                .await?;

            self.runtime
                .wait_ready(
                    &branch.container_name,
                    &self.pg_user,
                    &self.pg_db,
                    STARTUP_TIMEOUT,
                )
                .await?;
            self.store()
                .update_branch_state(&branch.id, BranchState::Running)?;
        } else {
            self.store()
                .update_branch_state(&branch.id, BranchState::Stopped)?;
        }

        Ok(())
    }

    fn supports_lifecycle(&self) -> bool {
        true
    }

    async fn test_connection(&self) -> Result<()> {
        let doctor = self.runtime.doctor().await;
        if !doctor.available {
            anyhow::bail!("Docker is not available: {}", doctor.detail);
        }
        Ok(())
    }

    async fn doctor(&self) -> Result<DoctorReport> {
        let mut checks = vec![];

        // Docker check
        let docker_result = self.runtime.doctor().await;
        checks.push(DoctorCheck {
            name: "Docker".to_string(),
            available: docker_result.available,
            detail: if let Some(version) = docker_result.version {
                format!("Docker {} available", version)
            } else {
                docker_result.detail
            },
        });

        // Storage check
        let storage_report = self.storage.doctor().await;
        for entry in &storage_report.entries {
            if entry.available || entry.selected {
                checks.push(DoctorCheck {
                    name: format!("Storage: {}", entry.kind),
                    available: entry.available,
                    detail: entry.detail.clone(),
                });
            }
        }

        checks.push(DoctorCheck {
            name: "Default storage".to_string(),
            available: true,
            detail: format!(
                "Using {} for new projects",
                storage_report.default_backend.as_str()
            ),
        });

        // State database
        checks.push(DoctorCheck {
            name: "State database".to_string(),
            available: true,
            detail: format!("{}/state.db", self.data_root.display()),
        });

        Ok(DoctorReport { checks })
    }

    async fn init_project(&self, _project_name: &str) -> Result<()> {
        let _project = self.ensure_project().await?;
        Ok(())
    }

    async fn seed_from_source(&self, branch_name: &str, source: &str) -> Result<()> {
        let project = self.ensure_project().await?;
        let branch = self
            .store()
            .get_branch_by_name(&project.id, branch_name)?
            .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", branch_name))?;
        let parsed = seed::parse_source(source)?;
        seed::seed_branch(
            self.runtime.client(),
            &parsed,
            &branch.container_name,
            &self.pg_user,
            &self.pg_db,
            &self.image,
        )
        .await
    }

    fn project_info(&self) -> Option<ProjectInfo> {
        let project = self
            .store()
            .get_project_by_name(&self.project_name)
            .ok()??;
        Some(ProjectInfo {
            name: project.name,
            storage_backend: Some(project.storage_backend.as_str().to_string()),
            image: Some(project.image),
        })
    }

    fn backend_name(&self) -> &'static str {
        "Local (Docker + CoW)"
    }

    fn supports_cleanup(&self) -> bool {
        true
    }

    fn max_branch_name_length(&self) -> usize {
        255
    }

    fn supports_destroy(&self) -> bool {
        true
    }

    async fn destroy_preview(&self) -> Result<Option<(String, Vec<String>)>> {
        let project = match self.store().get_project_by_name(&self.project_name)? {
            Some(p) => p,
            None => return Ok(None),
        };

        let branches = self.store().list_branches(&project.id)?;
        let branch_names: Vec<String> = branches.iter().map(|b| b.name.clone()).collect();

        Ok(Some((project.name.clone(), branch_names)))
    }

    async fn destroy_project(&self) -> Result<Vec<String>> {
        let project = self
            .store()
            .get_project_by_name(&self.project_name)?
            .ok_or_else(|| anyhow::anyhow!("Project '{}' not found", self.project_name))?;

        let branches = self.store().list_branches(&project.id)?;
        let branch_names: Vec<String> = branches.iter().map(|b| b.name.clone()).collect();

        // 1. Remove all Docker containers (best-effort)
        for branch in &branches {
            if let Err(e) = self.runtime.remove_branch(&branch.container_name).await {
                log::warn!(
                    "Failed to remove container '{}': {}",
                    branch.container_name,
                    e
                );
            }
        }

        // 2. Delete project-level storage data
        self.storage.delete_project_data(&project).await?;

        // 3. Delete project from SQLite (cascades to branches)
        self.store().delete_project(&project.id)?;

        Ok(branch_names)
    }
}

fn shellexpand(path: &str) -> String {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}/{}", home.display(), stripped);
        }
    }
    path.to_string()
}
