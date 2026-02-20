use anyhow::{anyhow, Context, Result};
use bollard::exec::StartExecOptions;
use bollard::models::{ContainerCreateBody, ExecConfig, HostConfig};
use bollard::query_parameters::{
    CreateContainerOptions, UploadToContainerOptions, WaitContainerOptions,
};
use bollard::Docker;
use futures_util::TryStreamExt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum SeedSource {
    PostgresUrl(url::Url),
    LocalFile(PathBuf),
    S3Object { bucket: String, key: String },
}

pub fn parse_source(from: &str) -> Result<SeedSource> {
    if from.starts_with("postgresql://") || from.starts_with("postgres://") {
        let url =
            url::Url::parse(from).with_context(|| format!("Invalid PostgreSQL URL: {}", from))?;
        Ok(SeedSource::PostgresUrl(url))
    } else if let Some(without_scheme) = from.strip_prefix("s3://") {
        let (bucket, key) = without_scheme
            .split_once('/')
            .ok_or_else(|| anyhow!("Invalid S3 URL: expected s3://bucket/key"))?;
        Ok(SeedSource::S3Object {
            bucket: bucket.to_string(),
            key: key.to_string(),
        })
    } else {
        let path = PathBuf::from(from);
        if !path.exists() {
            anyhow::bail!("File not found: {}", from);
        }
        Ok(SeedSource::LocalFile(path))
    }
}

pub async fn seed_branch(
    docker: &Docker,
    source: &SeedSource,
    container_name: &str,
    pg_user: &str,
    pg_db: &str,
    image: &str,
) -> Result<()> {
    match source {
        SeedSource::PostgresUrl(url) => {
            seed_from_postgres(docker, url, container_name, pg_user, pg_db, image).await
        }
        SeedSource::LocalFile(path) => {
            seed_from_file(docker, path, container_name, pg_user, pg_db).await
        }
        SeedSource::S3Object { bucket, key } => {
            seed_from_s3(docker, bucket, key, container_name, pg_user, pg_db, image).await
        }
    }
}

/// Detect dump format from file extension.
/// Returns true if this is a plain SQL file (use psql), false for custom/tar format (use pg_restore).
fn is_plain_sql(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext == "sql")
        .unwrap_or(false)
}

/// Create a tar archive in memory containing a single file.
fn create_tar_with_file(filename: &str, data: &[u8]) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append_data(&mut header, filename, data)?;
    builder
        .into_inner()
        .context("failed to finalize tar archive")
}

/// Execute a command inside a container. Returns (exit_code, stderr_text).
async fn docker_exec(docker: &Docker, container_name: &str, cmd: &[&str]) -> Result<(i64, String)> {
    let config = ExecConfig {
        cmd: Some(cmd.iter().map(|s| s.to_string()).collect()),
        attach_stdout: Some(true),
        attach_stderr: Some(true),
        ..Default::default()
    };

    let exec = docker
        .create_exec(container_name, config)
        .await
        .context("failed to create exec instance")?;

    let start_opts = Some(StartExecOptions {
        detach: false,
        ..Default::default()
    });

    // Consume the output stream
    let mut stderr_buf = Vec::new();
    match docker.start_exec(&exec.id, start_opts).await? {
        bollard::exec::StartExecResults::Attached { mut output, .. } => {
            while let Some(msg) = output.try_next().await? {
                if let bollard::container::LogOutput::StdErr { message } = msg {
                    stderr_buf.extend_from_slice(&message);
                }
            }
        }
        bollard::exec::StartExecResults::Detached => {}
    }

    let inspect = docker.inspect_exec(&exec.id).await?;
    let exit_code = inspect.exit_code.unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&stderr_buf).to_string();

    Ok((exit_code, stderr))
}

/// Upload a file into a container at the given path using the Docker API.
async fn upload_file_to_container(
    docker: &Docker,
    container_name: &str,
    container_dir: &str,
    filename: &str,
    data: &[u8],
) -> Result<()> {
    let tar_bytes = create_tar_with_file(filename, data)?;

    let options = UploadToContainerOptions {
        path: container_dir.to_string(),
        ..Default::default()
    };

    docker
        .upload_to_container(
            container_name,
            Some(options),
            bollard::body_full(bytes::Bytes::from(tar_bytes)),
        )
        .await
        .with_context(|| format!("failed to upload file to container '{container_name}'"))?;

    Ok(())
}

async fn seed_from_postgres(
    docker: &Docker,
    url: &url::Url,
    container_name: &str,
    pg_user: &str,
    pg_db: &str,
    image: &str,
) -> Result<()> {
    // Rewrite localhost/127.0.0.1 to host.docker.internal for Docker access
    let mut dump_url = url.clone();
    if let Some(host) = dump_url.host_str() {
        if host == "localhost" || host == "127.0.0.1" {
            dump_url
                .set_host(Some("host.docker.internal"))
                .map_err(|_| anyhow!("Failed to rewrite host to host.docker.internal"))?;
        }
    }

    let dump_url_str = dump_url.to_string();
    let dump_path = "/tmp/pgbranch_dump.Fc";

    // Create an ephemeral container to run pg_dump, writing to a file
    let dump_container_name = format!("pgbranch-dump-{}", uuid::Uuid::new_v4());
    let config = ContainerCreateBody {
        image: Some(image.to_string()),
        cmd: Some(vec![
            "pg_dump".to_string(),
            "-Fc".to_string(),
            dump_url_str,
            "-f".to_string(),
            dump_path.to_string(),
        ]),
        host_config: Some(HostConfig {
            extra_hosts: Some(vec!["host.docker.internal:host-gateway".to_string()]),
            ..Default::default()
        }),
        ..Default::default()
    };

    let options = CreateContainerOptions {
        name: Some(dump_container_name.clone()),
        ..Default::default()
    };

    docker
        .create_container(Some(options), config)
        .await
        .context("Failed to create pg_dump container")?;

    docker
        .start_container(
            &dump_container_name,
            None::<bollard::query_parameters::StartContainerOptions>,
        )
        .await
        .context("Failed to start pg_dump container")?;

    // Wait for the container to finish
    let wait_options = WaitContainerOptions {
        condition: "not-running".to_string(),
    };

    let wait_results: Vec<_> = docker
        .wait_container(&dump_container_name, Some(wait_options))
        .try_collect()
        .await
        .context("Failed to wait for pg_dump container")?;

    let exit_code = wait_results.first().map(|r| r.status_code).unwrap_or(-1);
    if exit_code != 0 {
        // Clean up container before failing
        let _ = docker
            .remove_container(
                &dump_container_name,
                Some(bollard::query_parameters::RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;
        anyhow::bail!("pg_dump failed with exit code: {}", exit_code);
    }

    // Download the dump file from the ephemeral container
    let download_options = bollard::query_parameters::DownloadFromContainerOptions {
        path: dump_path.to_string(),
    };

    let chunks: Vec<bytes::Bytes> = docker
        .download_from_container(&dump_container_name, Some(download_options))
        .try_collect()
        .await
        .context("Failed to download dump from container")?;
    let tar_bytes: Vec<u8> = chunks.into_iter().flat_map(|b| b.to_vec()).collect();

    // Clean up the ephemeral container
    let _ = docker
        .remove_container(
            &dump_container_name,
            Some(bollard::query_parameters::RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;

    // Extract the dump file from the tar archive
    let mut archive = tar::Archive::new(tar_bytes.as_slice());
    let mut dump_data = Vec::new();
    if let Some(entry) = archive
        .entries()
        .context("Failed to read tar archive")?
        .next()
    {
        let mut entry = entry?;
        std::io::Read::read_to_end(&mut entry, &mut dump_data)?;
    }

    if dump_data.is_empty() {
        anyhow::bail!("pg_dump produced empty output");
    }

    // Upload the dump file to the target container
    upload_file_to_container(
        docker,
        container_name,
        "/tmp",
        "pgbranch_seed_dump",
        &dump_data,
    )
    .await?;

    // Restore using pg_restore
    let restore_path = "/tmp/pgbranch_seed_dump";
    let (exit_code, stderr) = docker_exec(
        docker,
        container_name,
        &[
            "pg_restore",
            "-U",
            pg_user,
            "-d",
            pg_db,
            "--no-owner",
            restore_path,
        ],
    )
    .await
    .context("Failed to run pg_restore")?;

    // Clean up temp file
    let _ = docker_exec(docker, container_name, &["rm", "-f", restore_path]).await;

    if exit_code != 0 {
        if stderr.contains("FATAL") || stderr.contains("could not connect") {
            anyhow::bail!("pg_restore failed: {}", stderr.trim());
        }
        log::warn!("pg_restore exited with warnings: {}", stderr.trim());
    }

    Ok(())
}

async fn seed_from_file(
    docker: &Docker,
    path: &std::path::Path,
    container_name: &str,
    pg_user: &str,
    pg_db: &str,
) -> Result<()> {
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    if !abs_path.exists() {
        anyhow::bail!("Seed file not found: {}", abs_path.display());
    }

    let container_path = "/tmp/pgbranch_seed_dump";

    // Read the file and upload it to the container via bollard
    let file_data = tokio::fs::read(&abs_path)
        .await
        .with_context(|| format!("Failed to read seed file: {}", abs_path.display()))?;

    upload_file_to_container(
        docker,
        container_name,
        "/tmp",
        "pgbranch_seed_dump",
        &file_data,
    )
    .await?;

    // Restore
    let (exit_code, stderr) = if is_plain_sql(&abs_path) {
        docker_exec(
            docker,
            container_name,
            &["psql", "-U", pg_user, "-d", pg_db, "-f", container_path],
        )
        .await
        .context("Failed to run psql")?
    } else {
        docker_exec(
            docker,
            container_name,
            &[
                "pg_restore",
                "-U",
                pg_user,
                "-d",
                pg_db,
                "--no-owner",
                container_path,
            ],
        )
        .await
        .context("Failed to run pg_restore")?
    };

    // Clean up temp file in container
    let _ = docker_exec(docker, container_name, &["rm", "-f", container_path]).await;

    if exit_code != 0 {
        if stderr.contains("FATAL") || stderr.contains("could not connect") {
            anyhow::bail!("Restore failed: {}", stderr.trim());
        }
        log::warn!("Restore exited with warnings: {}", stderr.trim());
    }

    Ok(())
}

async fn seed_from_s3(
    docker: &Docker,
    bucket: &str,
    key: &str,
    container_name: &str,
    pg_user: &str,
    pg_db: &str,
    _image: &str,
) -> Result<()> {
    let region = std::env::var("AWS_DEFAULT_REGION")
        .or_else(|_| std::env::var("AWS_REGION"))
        .unwrap_or_else(|_| "us-east-1".to_string());

    let s3_bucket = s3::Bucket::new(
        bucket,
        s3::Region::Custom {
            region: region.clone(),
            endpoint: format!("https://s3.{}.amazonaws.com", region),
        },
        s3::creds::Credentials::from_env()?,
    )?;

    let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;

    // Derive filename from key
    let filename = key.rsplit('/').next().unwrap_or("dump");
    let temp_path = temp_dir.path().join(filename);

    println!("Downloading s3://{}/{} ...", bucket, key);
    let response = s3_bucket
        .get_object(key)
        .await
        .with_context(|| format!("Failed to download from S3: s3://{}/{}", bucket, key))?;

    if response.status_code() != 200 {
        anyhow::bail!("S3 download failed with status {}", response.status_code());
    }

    tokio::fs::write(&temp_path, response.bytes())
        .await
        .context("Failed to write S3 object to temp file")?;

    // Delegate to file-based seeding
    seed_from_file(docker, &temp_path, container_name, pg_user, pg_db).await
}
