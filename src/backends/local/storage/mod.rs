pub mod local_driver;
pub mod zfs_driver;
pub mod zfs_setup;

use std::path::Path;

use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};

use super::model::{Branch, Project, StorageBackend};

#[derive(Debug, Clone)]
pub struct StorageSelection {
    pub backend: StorageBackend,
    pub config: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageDoctorEntry {
    pub kind: String,
    pub available: bool,
    pub detail: String,
    pub selected: bool,
}

#[derive(Debug, Clone)]
pub struct StorageDoctorReport {
    pub entries: Vec<StorageDoctorEntry>,
    pub default_backend: StorageBackend,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZfsProjectConfig {
    pub root_dataset: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZfsBranchMetadata {
    pub dataset: String,
    pub origin_snapshot: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StorageCoordinator {
    projects_root: std::path::PathBuf,
    local: local_driver::LocalDriver,
    zfs: zfs_driver::ZfsDriver,
}

impl StorageCoordinator {
    pub fn new(projects_root: std::path::PathBuf) -> Self {
        Self {
            local: local_driver::LocalDriver::new(),
            zfs: zfs_driver::ZfsDriver::new(),
            projects_root,
        }
    }

    pub async fn doctor(&self) -> StorageDoctorReport {
        let zfs_report = self.zfs.detect(&self.projects_root).await;
        let apfs_report = self.local.detect_apfs(&self.projects_root).await;
        let reflink_report = self.local.detect_reflink(&self.projects_root).await;

        let default_backend = if zfs_report.available {
            StorageBackend::Zfs
        } else if apfs_report.available {
            StorageBackend::ApfsClone
        } else if reflink_report.available {
            StorageBackend::Reflink
        } else {
            StorageBackend::Copy
        };

        let entries = vec![
            StorageDoctorEntry {
                kind: StorageBackend::Zfs.as_str().to_string(),
                available: zfs_report.available,
                detail: zfs_report.detail,
                selected: default_backend == StorageBackend::Zfs,
            },
            StorageDoctorEntry {
                kind: StorageBackend::ApfsClone.as_str().to_string(),
                available: apfs_report.available,
                detail: apfs_report.detail,
                selected: default_backend == StorageBackend::ApfsClone,
            },
            StorageDoctorEntry {
                kind: StorageBackend::Reflink.as_str().to_string(),
                available: reflink_report.available,
                detail: reflink_report.detail,
                selected: default_backend == StorageBackend::Reflink,
            },
            StorageDoctorEntry {
                kind: StorageBackend::Copy.as_str().to_string(),
                available: true,
                detail: "portable full copy fallback".to_string(),
                selected: default_backend == StorageBackend::Copy,
            },
        ];

        StorageDoctorReport {
            entries,
            default_backend,
        }
    }

    pub async fn select_for_new_project(&self) -> StorageSelection {
        let report = self.doctor().await;

        match report.default_backend {
            StorageBackend::Zfs => {
                let zfs_report = self.zfs.detect(&self.projects_root).await;
                if let Some(root_dataset) = zfs_report.root_dataset {
                    let config = ZfsProjectConfig { root_dataset };
                    return StorageSelection {
                        backend: StorageBackend::Zfs,
                        config: Some(
                            serde_json::to_string(&config).unwrap_or_else(|_| "{}".to_string()),
                        ),
                    };
                }
                StorageSelection {
                    backend: StorageBackend::Copy,
                    config: None,
                }
            }
            other => StorageSelection {
                backend: other,
                config: None,
            },
        }
    }

    pub async fn create_empty_branch(
        &self,
        project: &Project,
        branch_id: &str,
        data_dir: &Path,
    ) -> anyhow::Result<Option<String>> {
        match project.storage_backend {
            StorageBackend::Zfs => {
                let config = parse_zfs_config(project)?;
                self.zfs
                    .create_empty(project, &config, branch_id, data_dir)
                    .await
            }
            StorageBackend::ApfsClone => {
                self.local
                    .prepare_empty(data_dir, local_driver::LocalMode::ApfsClone)
                    .await?;
                Ok(None)
            }
            StorageBackend::Reflink => {
                self.local
                    .prepare_empty(data_dir, local_driver::LocalMode::Reflink)
                    .await?;
                Ok(None)
            }
            StorageBackend::Copy => {
                self.local
                    .prepare_empty(data_dir, local_driver::LocalMode::Copy)
                    .await?;
                Ok(None)
            }
        }
    }

    pub async fn clone_branch_from_parent(
        &self,
        project: &Project,
        parent: &Branch,
        child_branch_id: &str,
        child_data_dir: &Path,
    ) -> anyhow::Result<Option<String>> {
        match project.storage_backend {
            StorageBackend::Zfs => {
                let config = parse_zfs_config(project)?;
                self.zfs
                    .clone_from_parent(project, &config, parent, child_branch_id, child_data_dir)
                    .await
            }
            StorageBackend::ApfsClone => {
                self.local
                    .clone_dir(
                        std::path::PathBuf::from(&parent.data_dir).as_path(),
                        child_data_dir,
                        local_driver::LocalMode::ApfsClone,
                    )
                    .await?;
                Ok(None)
            }
            StorageBackend::Reflink => {
                self.local
                    .clone_dir(
                        std::path::PathBuf::from(&parent.data_dir).as_path(),
                        child_data_dir,
                        local_driver::LocalMode::Reflink,
                    )
                    .await?;
                Ok(None)
            }
            StorageBackend::Copy => {
                self.local
                    .clone_dir(
                        std::path::PathBuf::from(&parent.data_dir).as_path(),
                        child_data_dir,
                        local_driver::LocalMode::Copy,
                    )
                    .await?;
                Ok(None)
            }
        }
    }

    pub async fn delete_branch_data(
        &self,
        project: &Project,
        branch: &Branch,
    ) -> anyhow::Result<()> {
        match project.storage_backend {
            StorageBackend::Zfs => {
                let config = parse_zfs_config(project)?;
                self.zfs.delete_branch(project, &config, branch).await
            }
            StorageBackend::ApfsClone | StorageBackend::Reflink | StorageBackend::Copy => {
                self.local
                    .remove_dir(std::path::PathBuf::from(&branch.data_dir).as_path())
                    .await
            }
        }
    }

    pub async fn delete_project_data(&self, project: &Project) -> anyhow::Result<()> {
        match project.storage_backend {
            StorageBackend::Zfs => {
                let config = parse_zfs_config(project)?;
                let project_dataset = format!("{}/projects/{}", config.root_dataset, project.id);
                // Recursively destroy the entire project dataset and children
                let output = tokio::process::Command::new("zfs")
                    .args(["destroy", "-r", "-f", &project_dataset])
                    .output()
                    .await;
                if let Err(e) = output {
                    log::warn!("failed to destroy ZFS dataset '{}': {}", project_dataset, e);
                } else if let Ok(ref out) = output {
                    if !out.status.success() {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        log::warn!(
                            "zfs destroy failed for '{}': {}",
                            project_dataset,
                            stderr.trim()
                        );
                    }
                }
                // Also remove the local project directory if it exists
                let project_dir = self.projects_root.join(&project.id);
                if tokio::fs::metadata(&project_dir).await.is_ok() {
                    tokio::fs::remove_dir_all(&project_dir)
                        .await
                        .with_context(|| {
                            format!(
                                "failed to remove project directory '{}'",
                                project_dir.display()
                            )
                        })?;
                }
            }
            StorageBackend::ApfsClone | StorageBackend::Reflink | StorageBackend::Copy => {
                let project_dir = self.projects_root.join(&project.id);
                if tokio::fs::metadata(&project_dir).await.is_ok() {
                    tokio::fs::remove_dir_all(&project_dir)
                        .await
                        .with_context(|| {
                            format!(
                                "failed to remove project directory '{}'",
                                project_dir.display()
                            )
                        })?;
                }
            }
        }
        Ok(())
    }
}

fn parse_zfs_config(project: &Project) -> anyhow::Result<ZfsProjectConfig> {
    let raw = project
        .storage_config
        .as_ref()
        .ok_or_else(|| anyhow!("project '{}' missing ZFS storage config", project.id))?;

    serde_json::from_str::<ZfsProjectConfig>(raw).map_err(|err| {
        anyhow!(
            "invalid ZFS storage config for project '{}': {err}",
            project.id
        )
    })
}
