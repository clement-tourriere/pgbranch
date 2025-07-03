use anyhow::{Context, Result};
use tokio_postgres::{Client, NoTls};
use crate::config::{Config, AuthMethod};
use std::fs;
use std::path::Path;

pub struct DatabaseManager {
    config: Config,
}

impl DatabaseManager {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub async fn connect(&self) -> Result<Client> {
        let connection_string = self.build_connection_string().await?;
        
        let (client, connection) = tokio_postgres::connect(&connection_string, NoTls).await
            .context("Failed to connect to PostgreSQL database")?;
        
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                log::error!("Database connection error: {}", e);
            }
        });
        
        Ok(client)
    }

    pub async fn create_database_branch(&self, branch_name: &str) -> Result<()> {
        let client = self.connect().await?;
        let db_name = self.config.get_database_name(branch_name);
        
        if self.database_exists(&client, &db_name).await? {
            log::info!("Database {} already exists, skipping creation", db_name);
            return Ok(());
        }
        
        let query = format!(
            "CREATE DATABASE {} WITH TEMPLATE {}",
            escape_identifier(&db_name),
            escape_identifier(&self.config.database.template_database)
        );
        
        client.execute(&query, &[]).await
            .with_context(|| format!("Failed to create database branch: {}", db_name))?;
        
        log::info!("Created database branch: {}", db_name);
        Ok(())
    }

    pub async fn drop_database_branch(&self, branch_name: &str) -> Result<()> {
        let client = self.connect().await?;
        let db_name = self.config.get_database_name(branch_name);
        
        if !self.database_exists(&client, &db_name).await? {
            log::info!("Database {} does not exist, skipping deletion", db_name);
            return Ok(());
        }
        
        let query = format!(
            "DROP DATABASE {}",
            escape_identifier(&db_name)
        );
        
        client.execute(&query, &[]).await
            .with_context(|| format!("Failed to drop database branch: {}", db_name))?;
        
        log::info!("Dropped database branch: {}", db_name);
        Ok(())
    }

    pub async fn list_database_branches(&self) -> Result<Vec<String>> {
        let client = self.connect().await?;
        let prefix = &self.config.database.database_prefix;
        
        let query = "SELECT datname FROM pg_database WHERE datname LIKE $1";
        let pattern = format!("{}_%", prefix);
        
        let rows = client.query(query, &[&pattern]).await
            .context("Failed to list database branches")?;
        
        let mut branches = Vec::new();
        for row in rows {
            let db_name: String = row.get(0);
            if let Some(branch_name) = self.extract_branch_name(&db_name) {
                branches.push(branch_name);
            }
        }
        
        Ok(branches)
    }

    pub async fn database_exists(&self, client: &Client, db_name: &str) -> Result<bool> {
        let query = "SELECT 1 FROM pg_database WHERE datname = $1";
        let rows = client.query(query, &[&db_name]).await
            .context("Failed to check if database exists")?;
        
        Ok(!rows.is_empty())
    }

    pub async fn cleanup_old_branches(&self, max_count: usize) -> Result<()> {
        let client = self.connect().await?;
        let prefix = &self.config.database.database_prefix;
        
        let query = r#"
            SELECT datname 
            FROM pg_database 
            WHERE datname LIKE $1 
            ORDER BY oid DESC 
            OFFSET $2
        "#;
        
        let pattern = format!("{}_%", prefix);
        let rows = client.query(query, &[&pattern, &(max_count as i64)]).await
            .context("Failed to query old branches for cleanup")?;
        
        for row in rows {
            let db_name: String = row.get(0);
            if let Some(branch_name) = self.extract_branch_name(&db_name) {
                self.drop_database_branch(&branch_name).await?;
            }
        }
        
        Ok(())
    }

    async fn get_password(&self) -> Result<Option<String>> {
        for method in &self.config.database.auth.methods {
            match method {
                AuthMethod::Password => {
                    if let Some(password) = &self.config.database.password {
                        log::debug!("Using password from config");
                        return Ok(Some(password.clone()));
                    }
                }
                AuthMethod::Environment => {
                    if let Some(password) = self.get_password_from_env() {
                        log::debug!("Using password from environment");
                        return Ok(Some(password));
                    }
                }
                AuthMethod::Pgpass => {
                    if let Some(password) = self.get_password_from_pgpass()? {
                        log::debug!("Using password from pgpass file");
                        return Ok(Some(password));
                    }
                }
                AuthMethod::Service => {
                    if let Some(password) = self.get_password_from_service()? {
                        log::debug!("Using password from service file");
                        return Ok(Some(password));
                    }
                }
                AuthMethod::Prompt => {
                    if let Some(password) = self.get_password_from_prompt()? {
                        log::debug!("Using password from interactive prompt");
                        return Ok(Some(password));
                    }
                }
                AuthMethod::System => {
                    // System auth (peer, trust, etc.) - no password needed
                    log::debug!("Using system authentication");
                    return Ok(None);
                }
            }
        }
        
        log::debug!("No password found from any authentication method");
        Ok(None)
    }

    async fn build_connection_string(&self) -> Result<String> {
        let mut conn_str = format!(
            "host={} port={} user={}",
            self.config.database.host,
            self.config.database.port,
            self.config.database.user
        );
        
        // Try authentication methods in order
        if let Some(password) = self.get_password().await? {
            conn_str.push_str(&format!(" password={}", password));
        }
        
        conn_str.push_str(" dbname=postgres");
        Ok(conn_str)
    }

    fn extract_branch_name(&self, db_name: &str) -> Option<String> {
        let prefix = format!("{}_", self.config.database.database_prefix);
        if db_name.starts_with(&prefix) {
            Some(db_name[prefix.len()..].to_string())
        } else {
            None
        }
    }

    fn get_password_from_env(&self) -> Option<String> {
        // Check standard PostgreSQL environment variables
        if let Ok(password) = std::env::var("PGPASSWORD") {
            return Some(password);
        }
        
        // Check for host-specific password
        let host_var = format!("PGPASSWORD_{}", self.config.database.host.to_uppercase());
        if let Ok(password) = std::env::var(&host_var) {
            return Some(password);
        }
        
        None
    }

    fn get_password_from_pgpass(&self) -> Result<Option<String>> {
        let pgpass_file = self.config.database.auth.pgpass_file.as_ref()
            .map(|f| Path::new(f).to_path_buf())
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .map(|home| home.join(".pgpass"))
                    .unwrap_or_else(|| Path::new(".pgpass").to_path_buf())
            });

        if !pgpass_file.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&pgpass_file)
            .with_context(|| format!("Failed to read pgpass file: {}", pgpass_file.display()))?;

        for line in content.lines() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }

            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() != 5 {
                continue;
            }

            let (pg_host, pg_port, pg_database, pg_user, pg_password) = 
                (parts[0], parts[1], parts[2], parts[3], parts[4]);

            // Check if this entry matches our connection parameters
            if self.matches_pgpass_entry(pg_host, pg_port, pg_database, pg_user) {
                return Ok(Some(pg_password.to_string()));
            }
        }

        Ok(None)
    }

    fn matches_pgpass_entry(&self, pg_host: &str, pg_port: &str, pg_database: &str, pg_user: &str) -> bool {
        let host_matches = pg_host == "*" || pg_host == self.config.database.host;
        let port_matches = pg_port == "*" || pg_port == self.config.database.port.to_string();
        let database_matches = pg_database == "*" || pg_database == "postgres";
        let user_matches = pg_user == "*" || pg_user == self.config.database.user;

        host_matches && port_matches && database_matches && user_matches
    }

    fn get_password_from_service(&self) -> Result<Option<String>> {
        let service_name = self.config.database.auth.service_name.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No service name configured"))?;

        let service_file = dirs::home_dir()
            .map(|home| home.join(".pg_service.conf"))
            .unwrap_or_else(|| Path::new(".pg_service.conf").to_path_buf());

        if !service_file.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&service_file)
            .with_context(|| format!("Failed to read service file: {}", service_file.display()))?;

        let mut current_service = None;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if line.starts_with('[') && line.ends_with(']') {
                current_service = Some(&line[1..line.len()-1]);
                continue;
            }

            if current_service == Some(service_name) {
                if let Some((key, value)) = line.split_once('=') {
                    if key.trim() == "password" {
                        return Ok(Some(value.trim().to_string()));
                    }
                }
            }
        }

        Ok(None)
    }

    fn get_password_from_prompt(&self) -> Result<Option<String>> {
        if !self.config.database.auth.prompt_for_password {
            return Ok(None);
        }

        let prompt = format!("Password for PostgreSQL user '{}': ", self.config.database.user);
        match rpassword::prompt_password(&prompt) {
            Ok(password) => Ok(Some(password)),
            Err(e) => {
                log::warn!("Failed to read password from prompt: {}", e);
                Ok(None)
            }
        }
    }
}

fn escape_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}