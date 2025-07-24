use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub mod postgres_local;
pub mod factory;
pub mod neon;
pub mod dblab;
pub mod xata;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchInfo {
    pub name: String,
    pub created_at: Option<DateTime<Utc>>,
    pub parent_branch: Option<String>,
    pub database_name: String,
}

#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub password: Option<String>,
    pub connection_string: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchMetadata {
    pub backend_type: String,
    pub branch_id: Option<String>,
    pub extra: Option<serde_json::Value>,
}

#[async_trait]
pub trait DatabaseBranchingBackend: Send + Sync {
    // Core branching operations
    async fn create_branch(&self, branch_name: &str, from_branch: Option<&str>) -> Result<BranchInfo>;
    async fn delete_branch(&self, branch_name: &str) -> Result<()>;
    async fn list_branches(&self) -> Result<Vec<BranchInfo>>;
    async fn branch_exists(&self, branch_name: &str) -> Result<bool>;
    async fn switch_to_branch(&self, branch_name: &str) -> Result<BranchInfo>;
    
    // Connection information for post_commands
    async fn get_connection_info(&self, branch_name: &str) -> Result<ConnectionInfo>;
    
    // Backend-specific capabilities
    fn supports_cleanup(&self) -> bool {
        true
    }
    
    fn supports_template_from_time(&self) -> bool {
        false
    }
    
    fn max_branch_name_length(&self) -> usize {
        63 // PostgreSQL default
    }
    
    // Optional: cleanup old branches
    async fn cleanup_old_branches(&self, max_count: usize) -> Result<Vec<String>> {
        if !self.supports_cleanup() {
            return Ok(vec![]);
        }
        
        let branches = self.list_branches().await?;
        let mut sorted_branches: Vec<_> = branches
            .into_iter()
            .filter(|b| b.name != "main" && b.name != "master")
            .collect();
        
        sorted_branches.sort_by(|a, b| {
            b.created_at.cmp(&a.created_at)
        });
        
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
    
    // Test connection
    async fn test_connection(&self) -> Result<()>;
    
    // Get backend display name
    fn backend_name(&self) -> &'static str;
}