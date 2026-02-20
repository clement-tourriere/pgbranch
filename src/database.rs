use crate::config::{AuthMethod, Config};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use tokio_postgres::{Client, NoTls};

pub struct DatabaseManager {
    config: Config,
}

impl DatabaseManager {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub async fn connect(&self) -> Result<Client> {
        let connection_string = self.build_connection_string().await?;

        let (client, connection) = tokio_postgres::connect(&connection_string, NoTls)
            .await
            .context("Failed to connect to PostgreSQL database")?;

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                log::error!("Database connection error: {}", e);
            }
        });

        Ok(client)
    }

    fn get_env_var_with_fallback(&self, var_name: &str) -> Option<String> {
        // First try .env file, then actual environment variables
        if let Some(value) = self.get_var_from_env_file(var_name) {
            return Some(value);
        }

        std::env::var(var_name).ok()
    }

    pub async fn create_database_branch(&self, branch_name: &str) -> Result<()> {
        let client = self.connect().await?;
        let db_name = self.config.get_database_name(branch_name);

        if self.database_exists(&client, &db_name).await? {
            log::info!("Database {} already exists, skipping creation", db_name);
            return Ok(());
        }

        // Terminate existing connections to the template database before creating
        self.terminate_connections_to_database(&client, &self.config.database.template_database)
            .await?;

        let query = format!(
            "CREATE DATABASE {} WITH TEMPLATE {}",
            escape_identifier(&db_name),
            escape_identifier(&self.config.database.template_database)
        );

        client
            .execute(&query, &[])
            .await
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

        let query = format!("DROP DATABASE {}", escape_identifier(&db_name));

        client
            .execute(&query, &[])
            .await
            .with_context(|| format!("Failed to drop database branch: {}", db_name))?;

        log::info!("Dropped database branch: {}", db_name);
        Ok(())
    }

    pub async fn list_database_branches(&self) -> Result<Vec<String>> {
        let client = self.connect().await?;
        let prefix = &self.config.database.database_prefix;

        let query = "SELECT datname FROM pg_database WHERE datname LIKE $1";
        let pattern = format!("{}_%", prefix);

        let rows = client
            .query(query, &[&pattern])
            .await
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
        let rows = client
            .query(query, &[&db_name])
            .await
            .context("Failed to check if database exists")?;

        Ok(!rows.is_empty())
    }

    async fn terminate_connections_to_database(
        &self,
        client: &Client,
        db_name: &str,
    ) -> Result<()> {
        log::debug!("Terminating connections to database: {}", db_name);

        // Query to find ALL connections to the database (excluding our own)
        let query = r#"
            SELECT pid, usename, application_name, client_addr, state
            FROM pg_stat_activity 
            WHERE datname = $1 
            AND pid != pg_backend_pid()
        "#;

        let rows = client
            .query(query, &[&db_name])
            .await
            .context("Failed to query active connections")?;

        if rows.is_empty() {
            log::debug!("No connections found to database: {}", db_name);
            return Ok(());
        }

        println!(
            "⚠️  Found {} connection(s) to database '{}', terminating them...",
            rows.len(),
            db_name
        );

        // Terminate each connection
        for row in rows {
            let pid: i32 = row.get(0);
            let username: String = row.get(1);
            let app_name: Option<String> = row.get(2);
            let _client_addr: Option<std::net::IpAddr> = row.get(3);
            let state: String = row.get(4);

            println!(
                "💀 Terminating connection: PID={}, User={}, State={}, App={:?}",
                pid, username, state, app_name
            );

            let terminate_query = "SELECT pg_terminate_backend($1)";
            match client.query(terminate_query, &[&pid]).await {
                Ok(_) => println!("✅ Successfully terminated connection PID: {}", pid),
                Err(e) => println!("❌ Failed to terminate connection PID {}: {}", pid, e),
            }
        }

        // Give more time for connections to close
        println!("⏱️  Waiting for connections to close...");
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        println!(
            "🔄 Finished terminating connections to database: {}",
            db_name
        );
        Ok(())
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
        let rows = client
            .query(query, &[&pattern, &(max_count as i64)])
            .await
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
                    // First try .env file, then actual environment variables
                    if let Some(password) = self.get_password_from_env_file() {
                        log::debug!("Using password from .env file");
                        return Ok(Some(password));
                    }
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
        // Use config values but allow .env file override for host, port, user
        let host = self
            .get_env_var_with_fallback("PGHOST")
            .or_else(|| self.get_var_from_env_file("POSTGRES_HOST"))
            .unwrap_or_else(|| self.config.database.host.clone());

        let port = self
            .get_env_var_with_fallback("PGPORT")
            .or_else(|| self.get_var_from_env_file("POSTGRES_PORT"))
            .and_then(|p| p.parse().ok())
            .unwrap_or(self.config.database.port);

        let user = self
            .get_env_var_with_fallback("PGUSER")
            .or_else(|| self.get_var_from_env_file("POSTGRES_USER"))
            .unwrap_or_else(|| self.config.database.user.clone());

        let mut conn_str = format!("host={} port={} user={}", host, port, user);

        // Try authentication methods in order
        if let Some(password) = self.get_password().await? {
            conn_str.push_str(&format!(" password={}", password));
        }

        conn_str.push_str(" dbname=postgres");
        log::debug!(
            "Connection string: {}",
            conn_str.replace("password=", "password=***")
        );
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

    fn get_password_from_env_file(&self) -> Option<String> {
        // Check for PGPASSWORD first
        if let Some(password) = self.get_var_from_env_file("PGPASSWORD") {
            log::debug!("Found PGPASSWORD in .env file");
            return Some(password);
        }

        // Also check for other common PostgreSQL password variables
        for var_name in ["POSTGRES_PASSWORD", "POSTGRESQL_PASSWORD", "DB_PASSWORD"] {
            if let Some(password) = self.get_var_from_env_file(var_name) {
                log::debug!("Found {} in .env file", var_name);
                return Some(password);
            }
        }

        None
    }

    fn get_var_from_env_file(&self, var_name: &str) -> Option<String> {
        // Check common .env file locations in order
        let env_files = vec![".env", ".env.local", ".env.development"];

        for env_file in env_files {
            if let Some(value) = self.read_env_var_from_file(env_file, var_name) {
                log::debug!("Found {} in {}", var_name, env_file);
                return Some(value);
            }
        }

        None
    }

    fn read_env_var_from_file(&self, file_path: &str, var_name: &str) -> Option<String> {
        if let Ok(content) = fs::read_to_string(file_path) {
            for line in content.lines() {
                let line = line.trim();

                // Skip empty lines and comments
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }

                // Parse KEY=VALUE format
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    let value = value.trim();

                    // Remove quotes if present
                    let value = value.trim_matches('"').trim_matches('\'');

                    if key == var_name {
                        return Some(value.to_string());
                    }
                }
            }
        }

        None
    }

    fn get_password_from_pgpass(&self) -> Result<Option<String>> {
        let pgpass_file = self
            .config
            .database
            .auth
            .pgpass_file
            .as_ref()
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

    fn matches_pgpass_entry(
        &self,
        pg_host: &str,
        pg_port: &str,
        pg_database: &str,
        pg_user: &str,
    ) -> bool {
        let host_matches = pg_host == "*" || pg_host == self.config.database.host;
        let port_matches = pg_port == "*" || pg_port == self.config.database.port.to_string();
        let database_matches = pg_database == "*" || pg_database == "postgres";
        let user_matches = pg_user == "*" || pg_user == self.config.database.user;

        host_matches && port_matches && database_matches && user_matches
    }

    fn get_password_from_service(&self) -> Result<Option<String>> {
        let service_name = self
            .config
            .database
            .auth
            .service_name
            .as_ref()
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
                current_service = Some(&line[1..line.len() - 1]);
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

        let prompt = format!(
            "Password for PostgreSQL user '{}': ",
            self.config.database.user
        );
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
