use anyhow::{Context, Result};
use super::{DatabaseBranchingBackend, postgres_local::PostgresLocalBackend, neon::NeonBackend, dblab::DBLabBackend, xata::XataBackend};
use crate::config::Config;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackendType {
    PostgresLocal,
    Neon,
    DBLab,
    Xata,
}

impl BackendType {
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "postgres_local" | "local" | "postgres" => Ok(BackendType::PostgresLocal),
            "neon" => Ok(BackendType::Neon),
            "dblab" | "database_lab" => Ok(BackendType::DBLab),
            "xata" => Ok(BackendType::Xata),
            _ => anyhow::bail!("Unknown backend type: {}", s),
        }
    }
}

pub async fn create_backend(config: &Config) -> Result<Box<dyn DatabaseBranchingBackend>> {
    if let Some(ref backend_config) = config.backend {
        let backend_type = BackendType::from_str(&backend_config.backend_type)?;
        
        match backend_type {
            BackendType::PostgresLocal => {
                if let Some(ref postgres_config) = backend_config.postgres_local {
                    // Create a temporary config with the postgres local settings
                    let mut temp_config = config.clone();
                    temp_config.database.host = postgres_config.host.clone();
                    temp_config.database.port = postgres_config.port;
                    temp_config.database.user = postgres_config.user.clone();
                    temp_config.database.password = postgres_config.password.clone();
                    temp_config.database.template_database = postgres_config.template_database.clone();
                    temp_config.database.database_prefix = postgres_config.database_prefix.clone();
                    temp_config.database.auth = postgres_config.auth.clone();
                    
                    let backend = PostgresLocalBackend::new(&temp_config).await
                        .context("Failed to create PostgreSQL local backend")?;
                    return Ok(Box::new(backend));
                } else {
                    anyhow::bail!("PostgresLocal backend selected but no postgres_local configuration provided");
                }
            }
            BackendType::Neon => {
                if let Some(ref neon_config) = backend_config.neon {
                    let backend = NeonBackend::new(
                        resolve_env_var(&neon_config.api_key)?,
                        resolve_env_var(&neon_config.project_id)?,
                        Some(neon_config.base_url.clone()),
                    )?;
                    return Ok(Box::new(backend));
                } else {
                    anyhow::bail!("Neon backend selected but no neon configuration provided");
                }
            }
            BackendType::DBLab => {
                if let Some(ref dblab_config) = backend_config.dblab {
                    let backend = DBLabBackend::new(
                        resolve_env_var(&dblab_config.api_url)?,
                        resolve_env_var(&dblab_config.auth_token)?,
                    )?;
                    return Ok(Box::new(backend));
                } else {
                    anyhow::bail!("DBLab backend selected but no dblab configuration provided");
                }
            }
            BackendType::Xata => {
                if let Some(ref xata_config) = backend_config.xata {
                    let backend = XataBackend::new(
                        resolve_env_var(&xata_config.organization_id)?,
                        resolve_env_var(&xata_config.project_id)?,
                        resolve_env_var(&xata_config.api_key)?,
                    )?;
                    return Ok(Box::new(backend));
                } else {
                    anyhow::bail!("Xata backend selected but no xata configuration provided");
                }
            }
        }
    }
    
    // Backward compatibility: default to PostgresLocal using the old database config
    let backend = PostgresLocalBackend::new(config).await
        .context("Failed to create PostgreSQL local backend")?;
    
    Ok(Box::new(backend))
}

// Helper function to resolve environment variables in config values
fn resolve_env_var(value: &str) -> Result<String> {
    if value.starts_with("${") && value.ends_with("}") {
        let env_var = &value[2..value.len()-1];
        std::env::var(env_var)
            .with_context(|| format!("Environment variable {} not found", env_var))
    } else {
        Ok(value.to_string())
    }
}