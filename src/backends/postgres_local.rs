use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use super::{DatabaseBranchingBackend, BranchInfo, ConnectionInfo};
use crate::config::Config;
use crate::database::DatabaseManager;

pub struct PostgresLocalBackend {
    config: Config,
    db_manager: DatabaseManager,
}

impl PostgresLocalBackend {
    pub async fn new(config: &Config) -> Result<Self> {
        let db_manager = DatabaseManager::new(config.clone());
        
        Ok(Self {
            config: config.clone(),
            db_manager,
        })
    }
    
    fn get_branch_database_name(&self, branch_name: &str) -> String {
        self.db_manager.get_branch_database_name(branch_name)
    }
}

#[async_trait]
impl DatabaseBranchingBackend for PostgresLocalBackend {
    async fn create_branch(&self, branch_name: &str, from_branch: Option<&str>) -> Result<BranchInfo> {
        let template_db = if let Some(from) = from_branch {
            self.get_branch_database_name(from)
        } else {
            self.config.database.template_database.clone()
        };
        
        self.db_manager.create_database_branch(branch_name, Some(&template_db)).await?;
        
        let database_name = self.get_branch_database_name(branch_name);
        
        Ok(BranchInfo {
            name: branch_name.to_string(),
            created_at: Some(Utc::now()),
            parent_branch: from_branch.map(|s| s.to_string()),
            database_name,
        })
    }
    
    async fn delete_branch(&self, branch_name: &str) -> Result<()> {
        self.db_manager.delete_database_branch(branch_name).await
    }
    
    async fn list_branches(&self) -> Result<Vec<BranchInfo>> {
        let db_names = self.db_manager.list_database_branches().await?;
        
        let branches: Vec<BranchInfo> = db_names
            .into_iter()
            .map(|db_name| {
                let branch_name = if !self.config.database.database_prefix.is_empty() {
                    db_name.strip_prefix(&format!("{}_", self.config.database.database_prefix))
                        .unwrap_or(&db_name)
                        .to_string()
                } else {
                    db_name.clone()
                };
                
                BranchInfo {
                    name: branch_name,
                    created_at: None, // PostgreSQL doesn't store creation time by default
                    parent_branch: None,
                    database_name: db_name,
                }
            })
            .collect();
        
        Ok(branches)
    }
    
    async fn branch_exists(&self, branch_name: &str) -> Result<bool> {
        self.db_manager.database_exists(branch_name).await
    }
    
    async fn switch_to_branch(&self, branch_name: &str) -> Result<BranchInfo> {
        // For local PostgreSQL, switching is handled by post_commands
        // We just verify the branch exists
        if !self.branch_exists(branch_name).await? {
            anyhow::bail!("Branch '{}' does not exist", branch_name);
        }
        
        let database_name = self.get_branch_database_name(branch_name);
        
        Ok(BranchInfo {
            name: branch_name.to_string(),
            created_at: None,
            parent_branch: None,
            database_name,
        })
    }
    
    async fn get_connection_info(&self, branch_name: &str) -> Result<ConnectionInfo> {
        let database_name = self.get_branch_database_name(branch_name);
        
        let password = self.db_manager.get_password().await
            .context("Failed to get database password")?;
        
        let connection_string = if let Some(ref password) = password {
            format!(
                "postgresql://{}:{}@{}:{}/{}",
                self.config.database.user,
                password,
                self.config.database.host,
                self.config.database.port,
                database_name
            )
        } else {
            format!(
                "postgresql://{}@{}:{}/{}",
                self.config.database.user,
                self.config.database.host,
                self.config.database.port,
                database_name
            )
        };
        
        Ok(ConnectionInfo {
            host: self.config.database.host.clone(),
            port: self.config.database.port,
            database: database_name,
            user: self.config.database.user.clone(),
            password,
            connection_string: Some(connection_string),
        })
    }
    
    async fn cleanup_old_branches(&self, max_count: usize) -> Result<Vec<String>> {
        self.db_manager.cleanup_old_branches(max_count).await
    }
    
    async fn test_connection(&self) -> Result<()> {
        self.db_manager.test_connection().await
    }
    
    fn backend_name(&self) -> &'static str {
        "PostgreSQL (Local)"
    }
    
    fn supports_cleanup(&self) -> bool {
        true
    }
    
    fn supports_template_from_time(&self) -> bool {
        false
    }
    
    fn max_branch_name_length(&self) -> usize {
        63 // PostgreSQL identifier limit
    }
}