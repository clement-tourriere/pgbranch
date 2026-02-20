use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{anyhow, Context};

use super::zfs_driver::ZfsDriver;

/// Configuration for creating a file-backed ZFS pool.
pub struct ZfsPoolSetupConfig {
    pub pool_name: String,
    pub image_path: PathBuf,
    pub image_size: String,
    pub mountpoint: PathBuf,
}

impl Default for ZfsPoolSetupConfig {
    fn default() -> Self {
        Self {
            pool_name: "pgbranch".to_string(),
            image_path: PathBuf::from("/var/lib/pgbranch/pgdata.img"),
            image_size: "10G".to_string(),
            mountpoint: PathBuf::from("/var/lib/pgbranch/data"),
        }
    }
}

/// Result of checking whether ZFS is available for auto-setup.
pub enum ZfsSetupStatus {
    /// A usable ZFS dataset already covers the projects_root.
    AlreadyAvailable { root_dataset: String },
    /// The "pgbranch" pool exists but wasn't detected as covering projects_root
    /// (e.g. data_root mismatch). Returns the pool's mountpoint.
    PgbranchPoolExists { mountpoint: String },
    /// ZFS tools are installed but no suitable pool exists.
    ToolsAvailableNoPool,
    /// The `zfs` command is not found.
    ToolsNotInstalled,
    /// Not running on Linux.
    NotSupported,
}

/// Check ZFS availability with setup-specific granularity.
pub async fn check_zfs_setup_status(projects_root: &Path) -> ZfsSetupStatus {
    if !cfg!(target_os = "linux") {
        return ZfsSetupStatus::NotSupported;
    }

    // Check if zfs command is available
    let which_result = tokio::process::Command::new("which")
        .arg("zfs")
        .output()
        .await;

    match which_result {
        Ok(output) if output.status.success() => {}
        _ => return ZfsSetupStatus::ToolsNotInstalled,
    }

    // Check if ZfsDriver already detects a usable dataset for our projects_root
    let driver = ZfsDriver::new();
    let detection = driver.detect(projects_root).await;
    if detection.available {
        if let Some(root_dataset) = detection.root_dataset {
            return ZfsSetupStatus::AlreadyAvailable { root_dataset };
        }
    }

    // Check if a "pgbranch" pool already exists
    let zpool_result = tokio::process::Command::new("zpool")
        .args(["list", "-H", "-o", "name,health", "pgbranch"])
        .output()
        .await;

    if let Ok(output) = zpool_result {
        if output.status.success() {
            // Pool exists — figure out its mountpoint
            let mountpoint = get_pool_mountpoint("pgbranch")
                .await
                .unwrap_or_else(|| "/var/lib/pgbranch/data".to_string());
            return ZfsSetupStatus::PgbranchPoolExists { mountpoint };
        }
    }

    ZfsSetupStatus::ToolsAvailableNoPool
}

/// Create a file-backed ZFS pool. On failure after pool creation, rolls back.
pub async fn create_file_backed_pool(config: &ZfsPoolSetupConfig) -> anyhow::Result<String> {
    let parent_dir = config
        .image_path
        .parent()
        .ok_or_else(|| anyhow!("invalid image path: no parent directory"))?;

    // Step 1: Create parent directory
    sudo_command("mkdir", &["-p", &parent_dir.to_string_lossy()])
        .await
        .context("failed to create parent directory")?;

    // Step 2: Create sparse image file
    sudo_command(
        "truncate",
        &[
            "-s",
            &config.image_size,
            &config.image_path.to_string_lossy(),
        ],
    )
    .await
    .context("failed to create sparse image file")?;

    // Step 3: Create zpool — from here on we need rollback on failure
    let pool_created = sudo_command(
        "zpool",
        &[
            "create",
            &config.pool_name,
            &config.image_path.to_string_lossy(),
        ],
    )
    .await;

    if let Err(e) = pool_created {
        // Rollback: remove image file
        let _ = sudo_command("rm", &["-f", &config.image_path.to_string_lossy()]).await;
        return Err(e).context("failed to create ZFS pool");
    }

    // Steps 4-7: configure pool properties and mountpoint
    let configure_result = configure_pool(config).await;
    if let Err(e) = configure_result {
        // Rollback: destroy pool and remove image
        let _ = sudo_command("zpool", &["destroy", &config.pool_name]).await;
        let _ = sudo_command("rm", &["-f", &config.image_path.to_string_lossy()]).await;
        return Err(e).context("failed to configure ZFS pool");
    }

    Ok(config.mountpoint.to_string_lossy().to_string())
}

async fn configure_pool(config: &ZfsPoolSetupConfig) -> anyhow::Result<()> {
    // Set compression=lz4 (PostgreSQL optimization)
    sudo_command("zfs", &["set", "compression=lz4", &config.pool_name])
        .await
        .context("failed to set compression=lz4")?;

    // Set recordsize=8k (aligned to PG page size)
    sudo_command("zfs", &["set", "recordsize=8k", &config.pool_name])
        .await
        .context("failed to set recordsize=8k")?;

    // Set mountpoint
    let mountpoint_arg = format!("mountpoint={}", config.mountpoint.display());
    sudo_command("zfs", &["set", &mountpoint_arg, &config.pool_name])
        .await
        .context("failed to set mountpoint")?;

    // Set ownership to current user
    let user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
    let ownership = format!("{}:{}", user, user);
    sudo_command("chown", &[&ownership, &config.mountpoint.to_string_lossy()])
        .await
        .context("failed to set ownership on mountpoint")?;

    // Delegate ZFS permissions to current user (so zfs create/destroy work without sudo)
    sudo_command(
        "zfs",
        &[
            "allow",
            &user,
            "create,destroy,snapshot,clone,mount,mountpoint,promote,rename,rollback",
            &config.pool_name,
        ],
    )
    .await
    .context("failed to delegate ZFS permissions to current user")?;

    Ok(())
}

async fn get_pool_mountpoint(pool_name: &str) -> Option<String> {
    let output = tokio::process::Command::new("zfs")
        .args(["get", "-H", "-o", "value", "mountpoint", pool_name])
        .output()
        .await
        .ok()?;

    if output.status.success() {
        let mountpoint = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !mountpoint.is_empty() && mountpoint != "-" && mountpoint != "none" {
            return Some(mountpoint);
        }
    }
    None
}

/// Run a command via sudo, printing what's being run and inheriting stdin for
/// the password prompt.
async fn sudo_command(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let display_args: Vec<&str> = args.to_vec();
    println!("  Running: sudo {} {}", program, display_args.join(" "));

    let output = tokio::process::Command::new("sudo")
        .arg(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "failed to spawn sudo {} {}",
                program,
                display_args.join(" ")
            )
        })?
        .wait_with_output()
        .await
        .with_context(|| {
            format!(
                "failed to wait for sudo {} {}",
                program,
                display_args.join(" ")
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "sudo {} {} failed: {}",
            program,
            display_args.join(" "),
            stderr.trim()
        ));
    }

    Ok(())
}
