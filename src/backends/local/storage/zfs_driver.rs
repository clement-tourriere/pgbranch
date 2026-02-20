use std::{ffi::OsString, path::Path};

use anyhow::{anyhow, Context};
use tokio::process::Command;
use uuid::Uuid;

use super::{ZfsBranchMetadata, ZfsProjectConfig};
use crate::backends::local::model::{Branch, Project};

#[derive(Debug, Clone)]
pub struct BackendDetection {
    pub available: bool,
    pub detail: String,
    pub root_dataset: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct ZfsDriver;

impl ZfsDriver {
    pub fn new() -> Self {
        Self
    }

    pub async fn detect(&self, projects_root: &Path) -> BackendDetection {
        if !cfg!(target_os = "linux") {
            return BackendDetection {
                available: false,
                detail: "ZFS backend is only supported on Linux".to_string(),
                root_dataset: None,
            };
        }

        let list_output = match zfs_output(["list", "-H", "-o", "name,mountpoint"]).await {
            Ok(output) => output,
            Err(err) => {
                return BackendDetection {
                    available: false,
                    detail: format!("unable to run zfs list: {err}"),
                    root_dataset: None,
                };
            }
        };

        if !list_output.status.success() {
            return BackendDetection {
                available: false,
                detail: format!(
                    "zfs list failed: {}",
                    String::from_utf8_lossy(&list_output.stderr).trim()
                ),
                root_dataset: None,
            };
        }

        let dataset = if let Ok(explicit) = std::env::var("PGBRANCH_ZFS_DATASET") {
            if explicit.trim().is_empty() {
                None
            } else {
                Some(explicit)
            }
        } else {
            detect_dataset_from_mountpoints(
                projects_root,
                &String::from_utf8_lossy(&list_output.stdout),
            )
        };

        let Some(root_dataset) = dataset else {
            return BackendDetection {
                available: false,
                detail: format!(
                    "no ZFS dataset found for '{}' (set PGBRANCH_ZFS_DATASET to force one)",
                    projects_root.display()
                ),
                root_dataset: None,
            };
        };

        let probe_name = format!("{root_dataset}/pgbranch_probe_{}", Uuid::new_v4());
        let create_probe = zfs_output_os(vec![
            OsString::from("create"),
            OsString::from("-p"),
            OsString::from(probe_name.clone()),
        ])
        .await;

        match create_probe {
            Ok(output) if output.status.success() => {
                let _ = zfs_output_os(vec![
                    OsString::from("destroy"),
                    OsString::from("-r"),
                    OsString::from(probe_name),
                ])
                .await;

                BackendDetection {
                    available: true,
                    detail: format!("ZFS available with root dataset '{root_dataset}'"),
                    root_dataset: Some(root_dataset),
                }
            }
            Ok(output) => {
                let _ = zfs_output_os(vec![
                    OsString::from("destroy"),
                    OsString::from("-r"),
                    OsString::from(probe_name),
                ])
                .await;

                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let detail = if stderr.contains("may only be mounted by root")
                    || stderr.contains("Insufficient privileges")
                {
                    format!(
                        "ZFS dataset detected ('{root_dataset}') but mounting child datasets requires root privileges on Linux: {stderr}"
                    )
                } else {
                    format!(
                        "ZFS dataset detected ('{root_dataset}') but create permission probe failed: {stderr}"
                    )
                };

                BackendDetection {
                    available: false,
                    detail,
                    root_dataset: Some(root_dataset),
                }
            }
            Err(err) => BackendDetection {
                available: false,
                detail: format!(
                    "ZFS dataset detected ('{root_dataset}') but probe command failed: {err}"
                ),
                root_dataset: Some(root_dataset),
            },
        }
    }

    pub async fn create_empty(
        &self,
        project: &Project,
        config: &ZfsProjectConfig,
        branch_id: &str,
        data_dir: &Path,
    ) -> anyhow::Result<Option<String>> {
        let branch_root = branch_root_from_data_dir(data_dir)?;
        let project_dataset = project_dataset_name(config, &project.id);
        let branch_dataset = branch_dataset_name(config, &project.id, branch_id);

        ensure_dataset_exists(&project_dataset).await?;
        ensure_dataset_exists(&format!("{project_dataset}/branches")).await?;
        ensure_dataset_absent(&branch_dataset).await?;

        create_dataset_with_mountpoint(&branch_dataset, branch_root).await?;
        tokio::fs::create_dir_all(data_dir)
            .await
            .with_context(|| format!("failed to create '{}'", data_dir.display()))?;

        let metadata = ZfsBranchMetadata {
            dataset: branch_dataset,
            origin_snapshot: None,
        };

        Ok(Some(
            serde_json::to_string(&metadata).context("failed to serialize ZFS branch metadata")?,
        ))
    }

    pub async fn clone_from_parent(
        &self,
        project: &Project,
        config: &ZfsProjectConfig,
        parent: &Branch,
        child_branch_id: &str,
        child_data_dir: &Path,
    ) -> anyhow::Result<Option<String>> {
        let parent_metadata = parse_zfs_branch_metadata(parent)?;
        let child_branch_root = branch_root_from_data_dir(child_data_dir)?;

        let child_dataset = branch_dataset_name(config, &project.id, child_branch_id);
        ensure_dataset_absent(&child_dataset).await?;

        let snapshot_name = format!("pgbranch_{}", short_id(child_branch_id));
        let snapshot_full = format!("{}@{}", parent_metadata.dataset, snapshot_name);

        zfs_output_os(vec![
            OsString::from("snapshot"),
            OsString::from(snapshot_full.clone()),
        ])
        .await
        .with_context(|| format!("failed to create ZFS snapshot '{snapshot_full}'"))?
        .success_or_stderr()?;

        zfs_output_os(vec![
            OsString::from("clone"),
            OsString::from("-o"),
            OsString::from(format!("mountpoint={}", child_branch_root.display())),
            OsString::from(snapshot_full.clone()),
            OsString::from(child_dataset.clone()),
        ])
        .await
        .with_context(|| format!("failed to create ZFS clone '{child_dataset}'"))?
        .success_or_stderr()?;

        tokio::fs::create_dir_all(child_data_dir)
            .await
            .with_context(|| {
                format!(
                    "failed to ensure cloned data dir '{}'",
                    child_data_dir.display()
                )
            })?;

        let metadata = ZfsBranchMetadata {
            dataset: child_dataset,
            origin_snapshot: Some(snapshot_full),
        };

        Ok(Some(
            serde_json::to_string(&metadata).context("failed to serialize ZFS branch metadata")?,
        ))
    }

    pub async fn delete_branch(
        &self,
        _project: &Project,
        _config: &ZfsProjectConfig,
        branch: &Branch,
    ) -> anyhow::Result<()> {
        let metadata = parse_zfs_branch_metadata(branch)?;

        let _ = zfs_output_os(vec![
            OsString::from("destroy"),
            OsString::from("-r"),
            OsString::from(metadata.dataset.clone()),
        ])
        .await;

        if let Some(snapshot) = metadata.origin_snapshot {
            let _ = zfs_output_os(vec![OsString::from("destroy"), OsString::from(snapshot)]).await;
        }

        let branch_root = branch_root_from_data_dir(Path::new(&branch.data_dir))?;
        if tokio::fs::metadata(branch_root).await.is_ok() {
            tokio::fs::remove_dir_all(branch_root)
                .await
                .with_context(|| format!("failed to remove '{}'", branch_root.display()))?;
        }

        Ok(())
    }
}

fn detect_dataset_from_mountpoints(projects_root: &Path, zfs_list_output: &str) -> Option<String> {
    let projects_root =
        std::fs::canonicalize(projects_root).unwrap_or_else(|_| projects_root.to_path_buf());
    let projects_root = projects_root.to_string_lossy().to_string();
    let mut winner: Option<(String, usize)> = None;

    for line in zfs_list_output.lines() {
        let mut parts = line.split('\t');
        let Some(dataset) = parts.next() else {
            continue;
        };
        let Some(mountpoint) = parts.next() else {
            continue;
        };

        if mountpoint == "-" || mountpoint == "legacy" {
            continue;
        }

        if projects_root == mountpoint || projects_root.starts_with(&format!("{mountpoint}/")) {
            let score = mountpoint.len();
            match winner {
                Some((_, best_score)) if best_score >= score => {}
                _ => winner = Some((dataset.to_string(), score)),
            }
        }
    }

    winner.map(|(dataset, _)| dataset)
}

fn parse_zfs_branch_metadata(branch: &Branch) -> anyhow::Result<ZfsBranchMetadata> {
    let raw = branch
        .storage_metadata
        .as_ref()
        .ok_or_else(|| anyhow!("branch '{}' is missing ZFS storage metadata", branch.id))?;

    serde_json::from_str(raw).with_context(|| {
        format!(
            "branch '{}' has invalid ZFS storage metadata: {}",
            branch.id, raw
        )
    })
}

fn branch_root_from_data_dir(data_dir: &Path) -> anyhow::Result<&Path> {
    data_dir.parent().ok_or_else(|| {
        anyhow!(
            "invalid branch data dir '{}': no parent",
            data_dir.display()
        )
    })
}

fn project_dataset_name(config: &ZfsProjectConfig, project_id: &str) -> String {
    format!("{}/projects/{}", config.root_dataset, project_id)
}

fn branch_dataset_name(config: &ZfsProjectConfig, project_id: &str, branch_id: &str) -> String {
    format!(
        "{}/projects/{}/branches/{}",
        config.root_dataset, project_id, branch_id
    )
}

async fn ensure_dataset_exists(dataset: &str) -> anyhow::Result<()> {
    if dataset_exists(dataset).await? {
        return Ok(());
    }

    zfs_output_os(vec![
        OsString::from("create"),
        OsString::from("-p"),
        OsString::from("-o"),
        OsString::from("mountpoint=none"),
        OsString::from(dataset.to_string()),
    ])
    .await
    .with_context(|| format!("failed to create ZFS dataset '{dataset}'"))?
    .success_or_stderr()?;

    Ok(())
}

async fn create_dataset_with_mountpoint(dataset: &str, mountpoint: &Path) -> anyhow::Result<()> {
    zfs_output_os(vec![
        OsString::from("create"),
        OsString::from("-p"),
        OsString::from("-o"),
        OsString::from(format!("mountpoint={}", mountpoint.display())),
        OsString::from(dataset.to_string()),
    ])
    .await
    .with_context(|| format!("failed to create ZFS dataset '{dataset}'"))?
    .success_or_stderr()?;

    Ok(())
}

async fn ensure_dataset_absent(dataset: &str) -> anyhow::Result<()> {
    if !dataset_exists(dataset).await? {
        return Ok(());
    }

    zfs_output_os(vec![
        OsString::from("destroy"),
        OsString::from("-r"),
        OsString::from(dataset.to_string()),
    ])
    .await
    .with_context(|| format!("failed to destroy existing ZFS dataset '{dataset}'"))?
    .success_or_stderr()?;

    Ok(())
}

async fn dataset_exists(dataset: &str) -> anyhow::Result<bool> {
    let output = zfs_output_os(vec![
        OsString::from("list"),
        OsString::from("-H"),
        OsString::from("-o"),
        OsString::from("name"),
        OsString::from(dataset.to_string()),
    ])
    .await
    .context("failed to run zfs list")?;

    Ok(output.status.success())
}

async fn zfs_output<const N: usize>(args: [&str; N]) -> anyhow::Result<std::process::Output> {
    Command::new("zfs")
        .args(args)
        .output()
        .await
        .context("failed to execute zfs command")
}

async fn zfs_output_os(args: Vec<OsString>) -> anyhow::Result<std::process::Output> {
    Command::new("zfs")
        .args(args)
        .output()
        .await
        .context("failed to execute zfs command")
}

fn short_id(value: &str) -> String {
    value.chars().take(8).collect()
}

trait OutputExt {
    fn success_or_stderr(self) -> anyhow::Result<()>;
}

impl OutputExt for std::process::Output {
    fn success_or_stderr(self) -> anyhow::Result<()> {
        if self.status.success() {
            return Ok(());
        }
        Err(anyhow!(String::from_utf8_lossy(&self.stderr)
            .trim()
            .to_string()))
    }
}
