use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub database: DatabaseConfig,
    pub git: GitConfig,
    pub behavior: BehaviorConfig,
    pub post_commands: Vec<PostCommand>,
    pub current_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub template_database: String,
    pub database_prefix: String,
    pub auth: AuthConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub methods: Vec<AuthMethod>,
    pub pgpass_file: Option<String>,
    pub service_name: Option<String>,
    pub prompt_for_password: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthMethod {
    #[serde(rename = "password")]
    Password,
    #[serde(rename = "pgpass")]
    Pgpass,
    #[serde(rename = "environment")]
    Environment,
    #[serde(rename = "service")]
    Service,
    #[serde(rename = "prompt")]
    Prompt,
    #[serde(rename = "system")]
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PostCommand {
    Simple(String),
    Complex(PostCommandConfig),
    Replace(ReplaceConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostCommandConfig {
    pub name: Option<String>,
    pub command: String,
    pub working_dir: Option<String>,
    pub continue_on_error: Option<bool>,
    pub condition: Option<String>,
    pub environment: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplaceConfig {
    pub action: String, // Must be "replace"
    pub name: Option<String>,
    pub file: String,
    pub pattern: String,
    pub replacement: String,
    pub create_if_missing: Option<bool>,
    pub continue_on_error: Option<bool>,
    pub condition: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConfig {
    pub auto_create_on_branch: bool,
    #[serde(default = "default_true")]
    pub auto_switch_on_branch: bool,
    #[serde(default = "default_main_branch")]
    pub main_branch: String,
    pub auto_create_branch_filter: Option<String>,
    // Keep the old field name for backward compatibility
    #[serde(alias = "branch_filter_regex")]
    pub branch_filter_regex: Option<String>,
    pub exclude_branches: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn default_main_branch() -> String {
    "main".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorConfig {
    pub auto_cleanup: bool,
    pub max_branches: Option<usize>,
    pub naming_strategy: NamingStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NamingStrategy {
    #[serde(rename = "prefix")]
    Prefix,
    #[serde(rename = "suffix")]
    Suffix,
    #[serde(rename = "replace")]
    Replace,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            database: DatabaseConfig {
                host: "localhost".to_string(),
                port: 5432,
                user: "postgres".to_string(),
                password: None,
                template_database: "template0".to_string(),
                database_prefix: "pgbranch".to_string(),
                auth: AuthConfig {
                    methods: vec![
                        AuthMethod::Environment,
                        AuthMethod::Pgpass,
                        AuthMethod::Password,
                        AuthMethod::Prompt,
                    ],
                    pgpass_file: None,
                    service_name: None,
                    prompt_for_password: false,
                },
            },
            git: GitConfig {
                auto_create_on_branch: true,
                auto_switch_on_branch: true,
                main_branch: "main".to_string(),
                auto_create_branch_filter: None,
                branch_filter_regex: None,
                exclude_branches: vec!["main".to_string(), "master".to_string()],
            },
            behavior: BehaviorConfig {
                auto_cleanup: false,
                max_branches: Some(10),
                naming_strategy: NamingStrategy::Prefix,
            },
            post_commands: vec![],
            current_branch: None,
        }
    }
}

impl Config {
    pub fn load_with_path_info() -> Result<(Self, Option<std::path::PathBuf>)> {
        if let Some(config_path) = Self::find_config_file()? {
            let config = Self::from_file(&config_path)?;
            Ok((config, Some(config_path)))
        } else {
            log::info!("No .pgbranch file found, using default configuration");
            Ok((Config::default(), None))
        }
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        
        let config: Config = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse YAML config file: {}", path.display()))?;
        
        Ok(config)
    }

    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let content = serde_yaml::to_string(self)
            .context("Failed to serialize config to YAML")?;
        
        fs::write(path, content)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;
        
        Ok(())
    }

    pub fn find_config_file() -> Result<Option<PathBuf>> {
        let mut current_dir = std::env::current_dir()
            .context("Failed to get current directory")?;
        
        loop {
            // Check for YAML format only
            for filename in [".pgbranch.yml", ".pgbranch.yaml"] {
                let config_path = current_dir.join(filename);
                if config_path.exists() {
                    return Ok(Some(config_path));
                }
            }
            
            if let Some(parent) = current_dir.parent() {
                current_dir = parent.to_path_buf();
            } else {
                break;
            }
        }
        
        Ok(None)
    }

    pub fn get_database_name(&self, branch_name: &str) -> String {
        // For main branch marker, use the template database name directly
        if branch_name == "_main" {
            return self.database.template_database.clone();
        }
        
        // For excluded branches (main/master), use the template database name directly
        if self.git.exclude_branches.contains(&branch_name.to_string()) {
            return self.database.template_database.clone();
        }
        
        let sanitized_branch = Self::sanitize_branch_name(branch_name);
        
        let full_name = match self.behavior.naming_strategy {
            NamingStrategy::Prefix => format!("{}_{}", self.database.database_prefix, sanitized_branch),
            NamingStrategy::Suffix => format!("{}_{}", sanitized_branch, self.database.database_prefix),
            NamingStrategy::Replace => sanitized_branch,
        };
        
        Self::ensure_valid_postgres_name(&full_name)
    }
    
    fn sanitize_branch_name(branch_name: &str) -> String {
        // Convert to lowercase and replace invalid characters with underscores
        let mut sanitized = String::new();
        
        for ch in branch_name.to_lowercase().chars() {
            match ch {
                // Valid PostgreSQL identifier characters
                'a'..='z' | '0'..='9' | '_' | '$' => sanitized.push(ch),
                // Replace everything else with underscore
                _ => sanitized.push('_'),
            }
        }
        
        // Ensure it starts with letter or underscore (not digit)
        if sanitized.starts_with(|c: char| c.is_ascii_digit()) {
            sanitized = format!("_{}", sanitized);
        }
        
        // Remove consecutive underscores for cleaner names
        while sanitized.contains("__") {
            sanitized = sanitized.replace("__", "_");
        }
        
        // Remove trailing underscore
        sanitized = sanitized.trim_end_matches('_').to_string();
        
        // Ensure we have something if everything got removed
        if sanitized.is_empty() {
            sanitized = "branch".to_string();
        }
        
        sanitized
    }
    
    fn ensure_valid_postgres_name(name: &str) -> String {
        const MAX_POSTGRES_NAME_LENGTH: usize = 63;
        
        if name.len() <= MAX_POSTGRES_NAME_LENGTH {
            return name.to_string();
        }
        
        // If name is too long, truncate and add hash to avoid collisions
        let hash = Self::calculate_name_hash(name);
        let hash_suffix = format!("_{:x}", hash);
        let max_prefix_len = MAX_POSTGRES_NAME_LENGTH - hash_suffix.len();
        
        format!("{}{}", &name[..max_prefix_len], hash_suffix)
    }
    
    fn calculate_name_hash(name: &str) -> u32 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        name.hash(&mut hasher);
        (hasher.finish() as u32) & 0xFFFF // Use 16 bits for shorter hash
    }

    pub fn should_create_branch(&self, branch_name: &str) -> bool {
        if !self.git.auto_create_on_branch {
            return false;
        }

        if self.git.exclude_branches.contains(&branch_name.to_string()) {
            return false;
        }

        if let Some(filter) = &self.git.branch_filter_regex {
            match regex::Regex::new(filter) {
                Ok(re) => re.is_match(branch_name),
                Err(_) => {
                    log::warn!("Invalid regex filter: {}", filter);
                    false
                }
            }
        } else {
            true
        }
    }
    
    pub fn should_switch_on_branch(&self, branch_name: &str) -> bool {
        if !self.git.auto_switch_on_branch {
            return false;
        }

        // Always switch to main branch
        if branch_name == self.git.main_branch {
            return true;
        }

        if self.git.exclude_branches.contains(&branch_name.to_string()) {
            return false;
        }

        if let Some(filter) = &self.git.branch_filter_regex {
            match regex::Regex::new(filter) {
                Ok(re) => re.is_match(branch_name),
                Err(_) => {
                    log::warn!("Invalid regex filter: {}", filter);
                    false
                }
            }
        } else {
            true
        }
    }

    pub fn substitute_template_variables(&self, template: &str, context: &TemplateContext) -> String {
        let mut result = template.to_string();
        
        result = result.replace("{branch_name}", &context.branch_name);
        result = result.replace("{db_name}", &context.db_name);
        result = result.replace("{db_host}", &context.db_host);
        result = result.replace("{db_port}", &context.db_port.to_string());
        result = result.replace("{db_user}", &context.db_user);
        result = result.replace("{template_db}", &context.template_db);
        result = result.replace("{prefix}", &context.prefix);
        
        if let Some(ref password) = context.db_password {
            result = result.replace("{db_password}", password);
        }
        
        result
    }

    pub fn get_current_branch(&self) -> Option<&String> {
        self.current_branch.as_ref()
    }

    pub fn set_current_branch(&mut self, branch_name: Option<String>) {
        self.current_branch = branch_name;
    }

    pub fn get_normalized_branch_name(&self, branch_name: &str) -> String {
        Self::sanitize_branch_name(branch_name)
    }
}

#[derive(Debug, Clone)]
pub struct TemplateContext {
    pub branch_name: String,
    pub db_name: String,
    pub db_host: String,
    pub db_port: u16,
    pub db_user: String,
    pub db_password: Option<String>,
    pub template_db: String,
    pub prefix: String,
}

impl TemplateContext {
    pub fn new(config: &Config, branch_name: &str) -> Self {
        Self {
            branch_name: branch_name.to_string(),
            db_name: config.get_database_name(branch_name),
            db_host: config.database.host.clone(),
            db_port: config.database.port,
            db_user: config.database.user.clone(),
            db_password: config.database.password.clone(),
            template_db: config.database.template_database.clone(),
            prefix: config.database.database_prefix.clone(),
        }
    }
}