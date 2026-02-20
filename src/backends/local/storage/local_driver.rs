use std::{ffi::OsString, path::Path};

use anyhow::{anyhow, Context};
use tokio::process::Command;
use uuid::Uuid;

use super::zfs_driver::BackendDetection;

#[derive(Debug, Clone, Copy)]
pub enum LocalMode {
    ApfsClone,
    Reflink,
    Copy,
}

#[derive(Debug, Default, Clone)]
pub struct LocalDriver;

impl LocalDriver {
    pub fn new() -> Self {
        Self
    }

    pub async fn detect_apfs(&self, projects_root: &Path) -> BackendDetection {
        if !cfg!(target_os = "macos") {
            return BackendDetection {
                available: false,
                detail: "APFS clone backend only applies on macOS".to_string(),
                root_dataset: None,
            };
        }

        let probe_dir = projects_root.join(format!(".pgbranch-apfs-probe-{}", Uuid::new_v4()));
        let src = probe_dir.join("src.bin");
        let dst = probe_dir.join("dst.bin");

        let result = async {
            tokio::fs::create_dir_all(&probe_dir)
                .await
                .with_context(|| format!("failed to create probe dir '{}'", probe_dir.display()))?;
            tokio::fs::write(&src, b"pgbranch")
                .await
                .with_context(|| format!("failed to write probe file '{}'", src.display()))?;

            run_cp(vec![
                OsString::from("-c"),
                src.as_os_str().to_owned(),
                dst.as_os_str().to_owned(),
            ])
            .await
        }
        .await;

        let _ = tokio::fs::remove_dir_all(&probe_dir).await;

        match result {
            Ok(()) => BackendDetection {
                available: true,
                detail: "APFS clone probe succeeded".to_string(),
                root_dataset: None,
            },
            Err(err) => BackendDetection {
                available: false,
                detail: format!("APFS clone probe failed: {err}"),
                root_dataset: None,
            },
        }
    }

    pub async fn detect_reflink(&self, projects_root: &Path) -> BackendDetection {
        if !cfg!(target_os = "linux") {
            return BackendDetection {
                available: false,
                detail: "reflink backend probe only runs on Linux".to_string(),
                root_dataset: None,
            };
        }

        let probe_dir = projects_root.join(format!(".pgbranch-reflink-probe-{}", Uuid::new_v4()));
        let src = probe_dir.join("src.bin");
        let dst = probe_dir.join("dst.bin");

        let result = async {
            tokio::fs::create_dir_all(&probe_dir)
                .await
                .with_context(|| format!("failed to create probe dir '{}'", probe_dir.display()))?;
            tokio::fs::write(&src, b"pgbranch")
                .await
                .with_context(|| format!("failed to write probe file '{}'", src.display()))?;

            run_cp(vec![
                OsString::from("-a"),
                OsString::from("--reflink=always"),
                src.as_os_str().to_owned(),
                dst.as_os_str().to_owned(),
            ])
            .await
        }
        .await;

        let _ = tokio::fs::remove_dir_all(&probe_dir).await;

        match result {
            Ok(()) => BackendDetection {
                available: true,
                detail: "reflink clone probe succeeded".to_string(),
                root_dataset: None,
            },
            Err(err) => BackendDetection {
                available: false,
                detail: format!("reflink clone probe failed: {err}"),
                root_dataset: None,
            },
        }
    }

    pub async fn prepare_empty(&self, data_dir: &Path, _mode: LocalMode) -> anyhow::Result<()> {
        recreate_dir(data_dir).await
    }

    pub async fn clone_dir(
        &self,
        source: &Path,
        target: &Path,
        mode: LocalMode,
    ) -> anyhow::Result<()> {
        tokio::fs::metadata(source)
            .await
            .with_context(|| format!("source directory '{}' not found", source.display()))?;

        recreate_dir(target).await?;

        let source_dot = source.join(".");
        match mode {
            LocalMode::ApfsClone => {
                let clone_attempt = run_cp(vec![
                    OsString::from("-cR"),
                    source_dot.as_os_str().to_owned(),
                    target.as_os_str().to_owned(),
                ])
                .await;

                if clone_attempt.is_ok() {
                    return Ok(());
                }

                run_cp(vec![
                    OsString::from("-R"),
                    source_dot.as_os_str().to_owned(),
                    target.as_os_str().to_owned(),
                ])
                .await
                .context("failed to clone directory with APFS fallback copy")?;
            }
            LocalMode::Reflink => {
                let reflink_attempt = run_cp(vec![
                    OsString::from("-a"),
                    OsString::from("--reflink=auto"),
                    source_dot.as_os_str().to_owned(),
                    target.as_os_str().to_owned(),
                ])
                .await;

                if reflink_attempt.is_ok() {
                    return Ok(());
                }

                run_cp(vec![
                    OsString::from("-a"),
                    source_dot.as_os_str().to_owned(),
                    target.as_os_str().to_owned(),
                ])
                .await
                .context("failed to clone directory with reflink fallback copy")?;
            }
            LocalMode::Copy => {
                run_cp(vec![
                    OsString::from("-a"),
                    source_dot.as_os_str().to_owned(),
                    target.as_os_str().to_owned(),
                ])
                .await
                .context("failed to copy directory")?;
            }
        }

        Ok(())
    }

    pub async fn remove_dir(&self, data_dir: &Path) -> anyhow::Result<()> {
        let branch_root = branch_root_from_data_dir(data_dir)?;
        if tokio::fs::metadata(branch_root).await.is_ok() {
            tokio::fs::remove_dir_all(branch_root)
                .await
                .with_context(|| {
                    format!("failed to delete directory '{}'", branch_root.display())
                })?;
        }
        Ok(())
    }
}

fn branch_root_from_data_dir(data_dir: &Path) -> anyhow::Result<&Path> {
    data_dir
        .parent()
        .ok_or_else(|| anyhow!("invalid data dir '{}'", data_dir.display()))
}

async fn recreate_dir(path: &Path) -> anyhow::Result<()> {
    let branch_root = branch_root_from_data_dir(path)?;
    if tokio::fs::metadata(branch_root).await.is_ok() {
        tokio::fs::remove_dir_all(branch_root)
            .await
            .with_context(|| format!("failed to delete directory '{}'", branch_root.display()))?;
    }

    tokio::fs::create_dir_all(path)
        .await
        .with_context(|| format!("failed to create directory '{}'", path.display()))?;
    Ok(())
}

async fn run_cp(args: Vec<OsString>) -> anyhow::Result<()> {
    let output = Command::new("cp")
        .args(args)
        .output()
        .await
        .context("failed to execute cp command")?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(anyhow!("cp command failed: {stderr}"))
}
