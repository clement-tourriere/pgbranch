use super::{BranchInfo, ConnectionInfo, DatabaseBranchingBackend, DoctorCheck, DoctorReport};
use crate::config::Config;
use crate::database::DatabaseManager;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;

pub struct PostgresTemplateBackend {
    config: Config,
    db_manager: DatabaseManager,
}

impl PostgresTemplateBackend {
    pub async fn new(config: &Config) -> Result<Self> {
        let db_manager = DatabaseManager::new(config.clone());

        Ok(Self {
            config: config.clone(),
            db_manager,
        })
    }

    fn get_branch_database_name(&self, branch_name: &str) -> String {
        self.config.get_database_name(branch_name)
    }
}

#[async_trait]
impl DatabaseBranchingBackend for PostgresTemplateBackend {
    async fn create_branch(
        &self,
        branch_name: &str,
        _from_branch: Option<&str>,
    ) -> Result<BranchInfo> {
        self.db_manager.create_database_branch(branch_name).await?;

        let database_name = self.get_branch_database_name(branch_name);

        Ok(BranchInfo {
            name: branch_name.to_string(),
            created_at: Some(Utc::now()),
            parent_branch: _from_branch.map(|s| s.to_string()),
            database_name,
            state: Some("running".to_string()),
        })
    }

    async fn delete_branch(&self, branch_name: &str) -> Result<()> {
        self.db_manager.drop_database_branch(branch_name).await
    }

    async fn list_branches(&self) -> Result<Vec<BranchInfo>> {
        let db_names = self.db_manager.list_database_branches().await?;

        let branches: Vec<BranchInfo> = db_names
            .into_iter()
            .map(|name| BranchInfo {
                name: name.clone(),
                created_at: None,
                parent_branch: None,
                database_name: self.get_branch_database_name(&name),
                state: Some("running".to_string()),
            })
            .collect();

        Ok(branches)
    }

    async fn branch_exists(&self, branch_name: &str) -> Result<bool> {
        let client = self.db_manager.connect().await?;
        let db_name = self.get_branch_database_name(branch_name);
        self.db_manager.database_exists(&client, &db_name).await
    }

    async fn switch_to_branch(&self, branch_name: &str) -> Result<BranchInfo> {
        let database_name = self.get_branch_database_name(branch_name);

        Ok(BranchInfo {
            name: branch_name.to_string(),
            created_at: None,
            parent_branch: None,
            database_name,
            state: Some("running".to_string()),
        })
    }

    async fn get_connection_info(&self, branch_name: &str) -> Result<ConnectionInfo> {
        let database_name = self.get_branch_database_name(branch_name);

        let connection_string = if let Some(ref password) = self.config.database.password {
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
            password: self.config.database.password.clone(),
            connection_string: Some(connection_string),
        })
    }

    async fn cleanup_old_branches(&self, max_count: usize) -> Result<Vec<String>> {
        self.db_manager.cleanup_old_branches(max_count).await?;
        Ok(vec![])
    }

    async fn test_connection(&self) -> Result<()> {
        let _client = self.db_manager.connect().await?;
        Ok(())
    }

    async fn doctor(&self) -> Result<DoctorReport> {
        let mut checks = vec![];

        // Check PostgreSQL connection
        let pg_check = match self.db_manager.connect().await {
            Ok(_client) => DoctorCheck {
                name: "PostgreSQL connection".to_string(),
                available: true,
                detail: format!(
                    "Connected to {}:{}",
                    self.config.database.host, self.config.database.port
                ),
            },
            Err(e) => DoctorCheck {
                name: "PostgreSQL connection".to_string(),
                available: false,
                detail: format!("Failed: {}", e),
            },
        };
        checks.push(pg_check);

        // Check template database
        let template_check = match self.db_manager.connect().await {
            Ok(client) => {
                match self
                    .db_manager
                    .database_exists(&client, &self.config.database.template_database)
                    .await
                {
                    Ok(true) => DoctorCheck {
                        name: "Template database".to_string(),
                        available: true,
                        detail: format!("'{}' exists", self.config.database.template_database),
                    },
                    Ok(false) => DoctorCheck {
                        name: "Template database".to_string(),
                        available: false,
                        detail: format!("'{}' not found", self.config.database.template_database),
                    },
                    Err(e) => DoctorCheck {
                        name: "Template database".to_string(),
                        available: false,
                        detail: format!("Error: {}", e),
                    },
                }
            }
            Err(e) => DoctorCheck {
                name: "Template database".to_string(),
                available: false,
                detail: format!("Cannot check (no connection): {}", e),
            },
        };
        checks.push(template_check);

        Ok(DoctorReport { checks })
    }

    fn backend_name(&self) -> &'static str {
        "PostgreSQL Template"
    }

    fn supports_cleanup(&self) -> bool {
        true
    }

    fn max_branch_name_length(&self) -> usize {
        63
    }
}
