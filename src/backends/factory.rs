#[cfg(feature = "backend-dblab")]
use super::dblab::DBLabBackend;
#[cfg(feature = "backend-local")]
use super::local::LocalBackend;
#[cfg(feature = "backend-neon")]
use super::neon::NeonBackend;
#[cfg(feature = "backend-postgres-template")]
use super::postgres_template::PostgresTemplateBackend;
#[cfg(feature = "backend-xata")]
use super::xata::XataBackend;
use super::DatabaseBranchingBackend;
use crate::config::{Config, NamedBackendConfig};
use anyhow::{Context, Result};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackendType {
    #[cfg(feature = "backend-local")]
    Local,
    #[cfg(feature = "backend-postgres-template")]
    PostgresTemplate,
    #[cfg(feature = "backend-neon")]
    Neon,
    #[cfg(feature = "backend-dblab")]
    DBLab,
    #[cfg(feature = "backend-xata")]
    Xata,
}

impl BackendType {
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            #[cfg(feature = "backend-local")]
            "local" | "docker" => Ok(BackendType::Local),
            #[cfg(not(feature = "backend-local"))]
            "local" | "docker" => anyhow::bail!("Local backend not compiled. Rebuild with --features backend-local"),

            #[cfg(feature = "backend-postgres-template")]
            "postgres_template" | "postgres" | "postgresql" => Ok(BackendType::PostgresTemplate),
            #[cfg(not(feature = "backend-postgres-template"))]
            "postgres_template" | "postgres" | "postgresql" => anyhow::bail!("PostgreSQL template backend not compiled. Rebuild with --features backend-postgres-template"),

            #[cfg(feature = "backend-neon")]
            "neon" => Ok(BackendType::Neon),
            #[cfg(not(feature = "backend-neon"))]
            "neon" => anyhow::bail!("Neon backend not compiled. Rebuild with --features backend-neon"),

            #[cfg(feature = "backend-dblab")]
            "dblab" | "database_lab" => Ok(BackendType::DBLab),
            #[cfg(not(feature = "backend-dblab"))]
            "dblab" | "database_lab" => anyhow::bail!("DBLab backend not compiled. Rebuild with --features backend-dblab"),

            #[cfg(feature = "backend-xata")]
            "xata" | "xata_lite" => Ok(BackendType::Xata),
            #[cfg(not(feature = "backend-xata"))]
            "xata" | "xata_lite" => anyhow::bail!("Xata backend not compiled. Rebuild with --features backend-xata"),

            _ => anyhow::bail!("Unknown backend type: {}. Valid types: local, postgres_template, neon, dblab, xata", s),
        }
    }

    pub fn is_local(s: &str) -> bool {
        matches!(s.to_lowercase().as_str(), "local" | "docker")
    }
}

pub struct NamedBackend {
    pub name: String,
    pub backend: Box<dyn DatabaseBranchingBackend>,
}

/// Create a backend from a NamedBackendConfig.
pub async fn create_backend_from_named_config(
    config: &Config,
    named: &NamedBackendConfig,
) -> Result<Box<dyn DatabaseBranchingBackend>> {
    let backend_type = BackendType::from_str(&named.backend_type)?;

    match backend_type {
        #[cfg(feature = "backend-local")]
        BackendType::Local => {
            let local_config = named.local.as_ref();
            let backend = LocalBackend::new(&named.name, config, local_config)
                .await
                .context("Failed to create local backend")?;
            Ok(Box::new(backend))
        }
        #[cfg(feature = "backend-postgres-template")]
        BackendType::PostgresTemplate => {
            let backend = PostgresTemplateBackend::new(config)
                .await
                .context("Failed to create PostgreSQL template backend")?;
            Ok(Box::new(backend))
        }
        #[cfg(feature = "backend-neon")]
        BackendType::Neon => {
            if let Some(ref neon_config) = named.neon {
                let backend = NeonBackend::new(
                    resolve_env_var(&neon_config.api_key)?,
                    resolve_env_var(&neon_config.project_id)?,
                    Some(neon_config.base_url.clone()),
                )?;
                Ok(Box::new(backend))
            } else {
                anyhow::bail!("Neon backend selected but no neon configuration provided");
            }
        }
        #[cfg(feature = "backend-dblab")]
        BackendType::DBLab => {
            if let Some(ref dblab_config) = named.dblab {
                let backend = DBLabBackend::new(
                    resolve_env_var(&dblab_config.api_url)?,
                    resolve_env_var(&dblab_config.auth_token)?,
                )?;
                Ok(Box::new(backend))
            } else {
                anyhow::bail!("DBLab backend selected but no dblab configuration provided");
            }
        }
        #[cfg(feature = "backend-xata")]
        BackendType::Xata => {
            if let Some(ref xata_config) = named.xata {
                let backend = XataBackend::new(
                    resolve_env_var(&xata_config.api_key)?,
                    resolve_env_var(&xata_config.organization_id)?,
                    resolve_env_var(&xata_config.project_id)?,
                    Some(xata_config.base_url.clone()),
                )?;
                Ok(Box::new(backend))
            } else {
                anyhow::bail!("Xata backend selected but no xata configuration provided");
            }
        }
    }
}

/// Resolve a single backend by name (or the default).
pub async fn resolve_backend(config: &Config, backend_name: Option<&str>) -> Result<NamedBackend> {
    config.validate_backends()?;

    let backends = config.resolve_backends();

    // If backends list is populated, use it
    if !backends.is_empty() {
        let named = if let Some(name) = backend_name {
            backends
                .iter()
                .find(|b| b.name == name)
                .ok_or_else(|| anyhow::anyhow!("Backend '{}' not found in configuration", name))?
        } else {
            backends
                .iter()
                .find(|b| b.default)
                .or(backends.first())
                .ok_or_else(|| anyhow::anyhow!("No backends configured"))?
        };

        let backend = create_backend_from_named_config(config, named).await?;
        return Ok(NamedBackend {
            name: named.name.clone(),
            backend,
        });
    }

    // No backends or backend config — fall back to auto-detection
    if backend_name.is_some() {
        anyhow::bail!("--database specified but no backends configured");
    }

    let backend = create_backend_default(config).await?;
    Ok(NamedBackend {
        name: "default".to_string(),
        backend,
    })
}

/// Instantiate all configured backends.
pub async fn create_all_backends(config: &Config) -> Result<Vec<NamedBackend>> {
    config.validate_backends()?;

    let named_configs = config.resolve_backends();

    if named_configs.is_empty() {
        // Fall back to default auto-detection
        let backend = create_backend_default(config).await?;
        return Ok(vec![NamedBackend {
            name: "default".to_string(),
            backend,
        }]);
    }

    let mut result = Vec::with_capacity(named_configs.len());
    for named in &named_configs {
        let backend = create_backend_from_named_config(config, named).await?;
        result.push(NamedBackend {
            name: named.name.clone(),
            backend,
        });
    }

    Ok(result)
}

/// Auto-detect backend when no config section is present.
async fn create_backend_default(config: &Config) -> Result<Box<dyn DatabaseBranchingBackend>> {
    // Backward compatibility: if database config differs from defaults,
    // use postgres_template backend
    #[cfg(feature = "backend-postgres-template")]
    if config.database.host != "localhost"
        || config.database.port != 5432
        || config.database.template_database != "template0"
    {
        let backend = PostgresTemplateBackend::new(config)
            .await
            .context("Failed to create PostgreSQL template backend")?;
        return Ok(Box::new(backend));
    }

    #[cfg(not(feature = "backend-postgres-template"))]
    if config.database.host != "localhost"
        || config.database.port != 5432
        || config.database.template_database != "template0"
    {
        anyhow::bail!("PostgreSQL template backend not compiled. Rebuild with --features backend-postgres-template");
    }

    // Default to local backend — derive name from cwd for backward compatibility
    #[cfg(feature = "backend-local")]
    {
        let default_name = std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "default".to_string());
        let backend = LocalBackend::new(&default_name, config, None)
            .await
            .context("Failed to create local backend")?;
        Ok(Box::new(backend))
    }

    #[cfg(not(feature = "backend-local"))]
    {
        anyhow::bail!("Local backend not compiled. Rebuild with --features backend-local");
    }
}

fn resolve_env_var(value: &str) -> Result<String> {
    if value.starts_with("${") && value.ends_with('}') {
        let env_var = &value[2..value.len() - 1];
        std::env::var(env_var)
            .with_context(|| format!("Environment variable {} not found", env_var))
    } else {
        Ok(value.to_string())
    }
}
