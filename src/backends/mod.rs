#[cfg(feature = "backend-dblab")]
pub mod dblab;
pub mod factory;
#[cfg(feature = "backend-local")]
pub mod local;
#[cfg(feature = "backend-neon")]
pub mod neon;
#[cfg(feature = "backend-postgres-template")]
pub mod postgres_template;
#[cfg(feature = "backend-xata")]
pub mod xata;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchInfo {
    pub name: String,
    pub created_at: Option<DateTime<Utc>>,
    pub parent_branch: Option<String>,
    pub database_name: String,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub password: Option<String>,
    pub connection_string: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    pub storage_backend: Option<String>,
    pub image: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub name: String,
    pub available: bool,
    pub detail: String,
}

#[async_trait]
#[allow(dead_code)]
pub trait DatabaseBranchingBackend: Send + Sync {
    // Core branching operations
    async fn create_branch(
        &self,
        branch_name: &str,
        from_branch: Option<&str>,
    ) -> Result<BranchInfo>;
    async fn delete_branch(&self, branch_name: &str) -> Result<()>;
    async fn list_branches(&self) -> Result<Vec<BranchInfo>>;
    async fn branch_exists(&self, branch_name: &str) -> Result<bool>;
    async fn switch_to_branch(&self, branch_name: &str) -> Result<BranchInfo>;

    // Connection information
    async fn get_connection_info(&self, branch_name: &str) -> Result<ConnectionInfo>;

    // Backend-specific capabilities
    fn supports_cleanup(&self) -> bool {
        true
    }
    fn supports_template_from_time(&self) -> bool {
        false
    }
    fn max_branch_name_length(&self) -> usize {
        63
    }

    // Lifecycle management (for local backend with Docker containers)
    async fn start_branch(&self, _branch_name: &str) -> Result<()> {
        Ok(())
    }
    async fn stop_branch(&self, _branch_name: &str) -> Result<()> {
        Ok(())
    }
    async fn reset_branch(&self, _branch_name: &str) -> Result<()> {
        Ok(())
    }
    fn supports_lifecycle(&self) -> bool {
        false
    }

    // Cleanup
    async fn cleanup_old_branches(&self, max_count: usize) -> Result<Vec<String>> {
        if !self.supports_cleanup() {
            return Ok(vec![]);
        }

        let branches = self.list_branches().await?;
        let mut sorted_branches: Vec<_> = branches
            .into_iter()
            .filter(|b| b.name != "main" && b.name != "master")
            .collect();

        sorted_branches.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        let mut deleted = Vec::new();
        if sorted_branches.len() > max_count {
            for branch in sorted_branches.into_iter().skip(max_count) {
                match self.delete_branch(&branch.name).await {
                    Ok(_) => deleted.push(branch.name),
                    Err(e) => log::warn!("Failed to delete branch {}: {}", branch.name, e),
                }
            }
        }

        Ok(deleted)
    }

    // Project destruction (local backend)
    fn supports_destroy(&self) -> bool {
        false
    }
    async fn destroy_preview(&self) -> Result<Option<(String, Vec<String>)>> {
        Ok(None)
    }
    async fn destroy_project(&self) -> Result<Vec<String>> {
        anyhow::bail!("This backend does not support project destruction")
    }

    // Data seeding
    async fn seed_from_source(&self, _branch_name: &str, _source: &str) -> Result<()> {
        anyhow::bail!("This backend does not support seeding from external sources")
    }

    // Diagnostics
    async fn doctor(&self) -> Result<DoctorReport>;

    // Test connection
    async fn test_connection(&self) -> Result<()>;

    // Init project (for local backend)
    async fn init_project(&self, _project_name: &str) -> Result<()> {
        Ok(())
    }

    // Project metadata (optional, implemented by local backend)
    fn project_info(&self) -> Option<ProjectInfo> {
        None
    }

    // Get backend display name
    fn backend_name(&self) -> &'static str;
}
