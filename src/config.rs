use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "DatabaseConfig::is_default")]
    pub database: DatabaseConfig,
    pub git: GitConfig,
    pub behavior: BehaviorConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub post_commands: Vec<PostCommand>,
    #[serde(skip)]
    pub current_branch: Option<String>, // Deprecated - kept for backward compatibility, not serialized
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<BackendConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backends: Option<Vec<NamedBackendConfig>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedBackendConfig {
    pub name: String,
    #[serde(rename = "type", default = "default_backend_type")]
    pub backend_type: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub default: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalBackendConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub neon: Option<NeonConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dblab: Option<DBLabConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "xata_lite")]
    pub xata: Option<XataConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendConfig {
    #[serde(rename = "type", default = "default_backend_type")]
    pub backend_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalBackendConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub neon: Option<NeonConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dblab: Option<DBLabConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "xata_lite")]
    pub xata: Option<XataConfig>,
}

fn default_backend_type() -> String {
    "local".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalBackendConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port_range_start: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postgres_user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postgres_password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postgres_db: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeonConfig {
    pub api_key: String,
    pub project_id: String,
    #[serde(default = "default_neon_base_url")]
    pub base_url: String,
}

fn default_neon_base_url() -> String {
    "https://console.neon.tech/api/v2".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DBLabConfig {
    pub api_url: String,
    pub auth_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XataConfig {
    pub api_key: String,
    pub organization_id: String,
    pub project_id: String,
    #[serde(default = "default_xata_base_url")]
    pub base_url: String,
}

fn default_xata_base_url() -> String {
    "https://api.xata.tech".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorktreeConfig {
    #[serde(default)]
    pub copy_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    pub template_database: String,
    pub database_prefix: String,
    pub auth: AuthConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub methods: Vec<AuthMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pgpass_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continue_on_error: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplaceConfig {
    pub action: String, // Must be "replace"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub file: String,
    pub pattern: String,
    pub replacement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_if_missing: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continue_on_error: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConfig {
    pub auto_create_on_branch: bool,
    #[serde(default = "default_true")]
    pub auto_switch_on_branch: bool,
    #[serde(default = "default_main_branch")]
    pub main_branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_create_branch_filter: Option<String>,
    // Keep the old field name for backward compatibility
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "branch_filter_regex"
    )]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

// Local configuration that can override the main config
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LocalConfig {
    pub database: Option<LocalDatabaseConfig>,
    pub git: Option<LocalGitConfig>,
    pub behavior: Option<LocalBehaviorConfig>,
    pub post_commands: Option<Vec<PostCommand>>,
    pub disabled: Option<bool>,
    pub disabled_branches: Option<Vec<String>>,
    pub worktree: Option<WorktreeConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LocalDatabaseConfig {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub template_database: Option<String>,
    pub database_prefix: Option<String>,
    pub auth: Option<LocalAuthConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LocalAuthConfig {
    pub methods: Option<Vec<AuthMethod>>,
    pub pgpass_file: Option<String>,
    pub service_name: Option<String>,
    pub prompt_for_password: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LocalGitConfig {
    pub auto_create_on_branch: Option<bool>,
    pub auto_switch_on_branch: Option<bool>,
    pub main_branch: Option<String>,
    pub auto_create_branch_filter: Option<String>,
    pub branch_filter_regex: Option<String>,
    pub exclude_branches: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LocalBehaviorConfig {
    pub auto_cleanup: Option<bool>,
    pub max_branches: Option<usize>,
    pub naming_strategy: Option<NamingStrategy>,
}

// Environment variable configuration
#[derive(Debug, Clone, Default)]
pub struct EnvConfig {
    pub disabled: Option<bool>,
    pub skip_hooks: Option<bool>,
    pub auto_create: Option<bool>,
    pub auto_switch: Option<bool>,
    pub branch_filter_regex: Option<String>,
    pub disabled_branches: Option<Vec<String>>,
    pub current_branch_disabled: Option<bool>,
    pub database_host: Option<String>,
    pub database_port: Option<u16>,
    pub database_user: Option<String>,
    pub database_password: Option<String>,
    pub database_prefix: Option<String>,
}

// The effective configuration after merging all sources
#[derive(Debug, Clone)]
pub struct EffectiveConfig {
    pub config: Config,
    pub local_config: Option<LocalConfig>,
    pub env_config: EnvConfig,
    pub disabled: bool,
    pub skip_hooks: bool,
    pub current_branch_disabled: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        DatabaseConfig {
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
        }
    }
}

impl DatabaseConfig {
    pub fn is_default(&self) -> bool {
        let default = DatabaseConfig::default();
        self.host == default.host
            && self.port == default.port
            && self.user == default.user
            && self.password.is_none()
            && self.template_database == default.template_database
            && self.database_prefix == default.database_prefix
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            database: DatabaseConfig::default(),
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
            current_branch: None, // Deprecated field, always None for new configs
            backend: None,
            backends: None,
            worktree: None,
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

        let mut config: Config = serde_yaml_ng::from_str(&content)
            .with_context(|| format!("Failed to parse YAML config file: {}", path.display()))?;

        // Handle backward compatibility: if current_branch was loaded, ignore it
        // The local state manager will handle current branch tracking
        config.current_branch = None;

        Ok(config)
    }

    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let content =
            serde_yaml_ng::to_string(self).context("Failed to serialize config to YAML")?;

        fs::write(path, content)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;

        Ok(())
    }

    pub fn find_config_file() -> Result<Option<PathBuf>> {
        let mut current_dir = std::env::current_dir().context("Failed to get current directory")?;

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
            NamingStrategy::Prefix => {
                format!("{}_{}", self.database.database_prefix, sanitized_branch)
            }
            NamingStrategy::Suffix => {
                format!("{}_{}", sanitized_branch, self.database.database_prefix)
            }
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

    pub fn substitute_template_variables(
        &self,
        template: &str,
        context: &TemplateContext,
    ) -> String {
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

    // Deprecated methods - current branch is now managed by LocalStateManager
    #[deprecated(since = "0.1.0", note = "Use LocalStateManager instead")]
    #[allow(dead_code)]
    pub fn get_current_branch(&self) -> Option<&String> {
        self.current_branch.as_ref()
    }

    #[deprecated(since = "0.1.0", note = "Use LocalStateManager instead")]
    #[allow(dead_code)]
    pub fn set_current_branch(&mut self, branch_name: Option<String>) {
        self.current_branch = branch_name;
    }

    pub fn get_normalized_branch_name(&self, branch_name: &str) -> String {
        Self::sanitize_branch_name(branch_name)
    }

    /// Resolve the list of named backends from either `backends` (new) or `backend` (legacy).
    pub fn resolve_backends(&self) -> Vec<NamedBackendConfig> {
        if let Some(ref backends) = self.backends {
            backends.clone()
        } else if let Some(ref backend) = self.backend {
            vec![NamedBackendConfig {
                name: "default".to_string(),
                backend_type: backend.backend_type.clone(),
                default: true,
                local: backend.local.clone(),
                neon: backend.neon.clone(),
                dblab: backend.dblab.clone(),
                xata: backend.xata.clone(),
            }]
        } else {
            vec![]
        }
    }

    /// Return the name of the default backend (the one with `default: true`, or the first).
    #[allow(dead_code)]
    pub fn default_backend_name(&self) -> Option<String> {
        let backends = self.resolve_backends();
        if backends.is_empty() {
            return None;
        }
        backends
            .iter()
            .find(|b| b.default)
            .or(backends.first())
            .map(|b| b.name.clone())
    }

    /// Look up a named backend config by name.
    #[allow(dead_code)]
    pub fn get_backend_config(&self, name: &str) -> Option<NamedBackendConfig> {
        self.resolve_backends().into_iter().find(|b| b.name == name)
    }

    /// Validate the backends configuration (no duplicates, not both `backend` and `backends`).
    pub fn validate_backends(&self) -> Result<()> {
        if self.backend.is_some() && self.backends.is_some() {
            anyhow::bail!(
                "Configuration error: cannot specify both 'backend' and 'backends'. \
                 Use 'backends' for multiple backends or 'backend' for a single one."
            );
        }
        if let Some(ref backends) = self.backends {
            // Check for unique names
            let mut seen = std::collections::HashSet::new();
            let mut default_count = 0;
            for b in backends {
                if !seen.insert(&b.name) {
                    anyhow::bail!("Duplicate backend name: '{}'", b.name);
                }
                if b.default {
                    default_count += 1;
                }
            }
            if default_count > 1 {
                anyhow::bail!(
                    "At most one backend can be marked as default, found {}",
                    default_count
                );
            }
        }
        Ok(())
    }

    /// Migrate legacy single `backend` to `backends` array. Returns true if migrated.
    #[allow(dead_code)]
    pub fn migrate_to_backends_array(&mut self) -> bool {
        if self.backend.is_some() && self.backends.is_none() {
            let backend = self.backend.take().unwrap();
            self.backends = Some(vec![NamedBackendConfig {
                name: "default".to_string(),
                backend_type: backend.backend_type,
                default: true,
                local: backend.local,
                neon: backend.neon,
                dblab: backend.dblab,
                xata: backend.xata,
            }]);
            true
        } else {
            false
        }
    }

    /// Add a named backend. Errors if name exists unless force=true.
    #[allow(dead_code)]
    pub fn add_backend(&mut self, named: NamedBackendConfig, force: bool) -> Result<()> {
        let backends = self.backends.get_or_insert_with(Vec::new);

        if let Some(pos) = backends.iter().position(|b| b.name == named.name) {
            if force {
                backends[pos] = named;
            } else {
                anyhow::bail!(
                    "Backend '{}' already exists. Use --force to overwrite.",
                    backends[pos].name
                );
            }
        } else {
            // Set default if it's the first entry
            let mut named = named;
            if backends.is_empty() {
                named.default = true;
            }
            backends.push(named);
        }

        Ok(())
    }

    pub fn remove_backend(&mut self, name: &str) {
        if let Some(ref mut backends) = self.backends {
            backends.retain(|b| b.name != name);
        }
    }

    pub fn load_effective_config_with_path_info(
    ) -> Result<(EffectiveConfig, Option<std::path::PathBuf>)> {
        // Load main config
        let (config, config_path) = Self::load_with_path_info()?;

        // Load local config if it exists - check in current directory if no main config path
        let local_config = if let Some(ref path) = config_path {
            let mut lc = LocalConfig::load_from_project_dir(path.parent().unwrap())?;
            // If no local config found and we're in a worktree, try the main worktree
            if lc.is_none() {
                if let Ok(git_repo) = crate::git::GitRepository::new(".") {
                    if git_repo.is_worktree() {
                        if let Some(main_dir) = git_repo.get_main_worktree_dir() {
                            lc = LocalConfig::load_from_project_dir(&main_dir)?;
                        }
                    }
                }
            }
            lc
        } else {
            // No main config found, but check current directory for local config
            LocalConfig::load_from_project_dir(&std::env::current_dir()?)?
        };

        // Load environment config
        let env_config = EnvConfig::load_from_env()?;

        // Create effective config
        let effective_config = EffectiveConfig::new(config, local_config, env_config)?;

        Ok((effective_config, config_path))
    }
}

impl LocalConfig {
    pub fn load_from_project_dir(project_dir: &Path) -> Result<Option<Self>> {
        let local_config_path = project_dir.join(".pgbranch.local.yml");

        if !local_config_path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&local_config_path).with_context(|| {
            format!(
                "Failed to read local config file: {}",
                local_config_path.display()
            )
        })?;

        let local_config: LocalConfig = serde_yaml_ng::from_str(&content).with_context(|| {
            format!(
                "Failed to parse local config file: {}",
                local_config_path.display()
            )
        })?;

        log::debug!("Loaded local config from: {}", local_config_path.display());
        Ok(Some(local_config))
    }
}

impl EnvConfig {
    pub fn load_from_env() -> Result<Self> {
        let env_config = EnvConfig {
            disabled: Self::parse_bool_env("PGBRANCH_DISABLED")?,
            skip_hooks: Self::parse_bool_env("PGBRANCH_SKIP_HOOKS")?,
            auto_create: Self::parse_bool_env("PGBRANCH_AUTO_CREATE")?,
            auto_switch: Self::parse_bool_env("PGBRANCH_AUTO_SWITCH")?,
            current_branch_disabled: Self::parse_bool_env("PGBRANCH_CURRENT_BRANCH_DISABLED")?,
            branch_filter_regex: env::var("PGBRANCH_BRANCH_FILTER_REGEX").ok(),
            database_host: env::var("PGBRANCH_DATABASE_HOST").ok(),
            database_user: env::var("PGBRANCH_DATABASE_USER").ok(),
            database_password: env::var("PGBRANCH_DATABASE_PASSWORD").ok(),
            database_prefix: env::var("PGBRANCH_DATABASE_PREFIX").ok(),
            database_port: env::var("PGBRANCH_DATABASE_PORT")
                .ok()
                .and_then(|s| s.parse().ok()),
            disabled_branches: env::var("PGBRANCH_DISABLED_BRANCHES")
                .ok()
                .map(|s| s.split(',').map(|s| s.trim().to_string()).collect()),
        };

        Ok(env_config)
    }

    fn parse_bool_env(key: &str) -> Result<Option<bool>> {
        match env::var(key) {
            Ok(value) => match value.to_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => Ok(Some(true)),
                "false" | "0" | "no" | "off" => Ok(Some(false)),
                _ => Err(anyhow::anyhow!(
                    "Invalid boolean value for {}: '{}'. Use true/false, 1/0, yes/no, or on/off",
                    key,
                    value
                )),
            },
            Err(_) => Ok(None),
        }
    }
}

impl EffectiveConfig {
    pub fn new(
        config: Config,
        local_config: Option<LocalConfig>,
        env_config: EnvConfig,
    ) -> Result<Self> {
        // Determine global disabled state
        let disabled = env_config.disabled.unwrap_or(
            local_config
                .as_ref()
                .and_then(|c| c.disabled)
                .unwrap_or(false),
        );

        // Determine skip hooks state
        let skip_hooks = env_config.skip_hooks.unwrap_or(false);

        // Determine current branch disabled state
        let current_branch_disabled = env_config.current_branch_disabled.unwrap_or(false);

        Ok(EffectiveConfig {
            config,
            local_config,
            env_config,
            disabled,
            skip_hooks,
            current_branch_disabled,
        })
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    pub fn should_skip_hooks(&self) -> bool {
        self.skip_hooks
    }

    pub fn is_current_branch_disabled(&self) -> bool {
        self.current_branch_disabled
    }

    pub fn is_branch_disabled(&self, branch_name: &str) -> bool {
        // Check environment disabled branches
        if let Some(ref disabled_branches) = self.env_config.disabled_branches {
            if Self::branch_matches_patterns(branch_name, disabled_branches) {
                return true;
            }
        }

        // Check local config disabled branches
        if let Some(ref local_config) = self.local_config {
            if let Some(ref disabled_branches) = local_config.disabled_branches {
                if Self::branch_matches_patterns(branch_name, disabled_branches) {
                    return true;
                }
            }
        }

        false
    }

    fn branch_matches_patterns(branch_name: &str, patterns: &[String]) -> bool {
        patterns.iter().any(|pattern| {
            if pattern.contains('*') {
                // Simple glob pattern matching
                let regex_pattern = pattern.replace('*', ".*");
                match regex::Regex::new(&regex_pattern) {
                    Ok(re) => re.is_match(branch_name),
                    Err(_) => false,
                }
            } else {
                // Exact match
                branch_name == pattern
            }
        })
    }

    pub fn check_current_git_branch_disabled(&self) -> Result<bool> {
        if self.is_current_branch_disabled() {
            return Ok(true);
        }

        // Get current Git branch and check if it's disabled
        match crate::git::GitRepository::new(".") {
            Ok(git_repo) => {
                if let Ok(Some(current_branch)) = git_repo.get_current_branch() {
                    Ok(self.is_branch_disabled(&current_branch))
                } else {
                    Ok(false)
                }
            }
            Err(_) => Ok(false),
        }
    }

    pub fn should_exit_early(&self) -> Result<bool> {
        if self.is_disabled() {
            return Ok(true);
        }

        self.check_current_git_branch_disabled()
    }

    pub fn get_merged_config(&self) -> Config {
        let mut merged = self.config.clone();

        // Apply local config overrides
        if let Some(ref local_config) = self.local_config {
            if let Some(ref local_db) = local_config.database {
                if let Some(ref host) = local_db.host {
                    merged.database.host = host.clone();
                }
                if let Some(port) = local_db.port {
                    merged.database.port = port;
                }
                if let Some(ref user) = local_db.user {
                    merged.database.user = user.clone();
                }
                if let Some(ref password) = local_db.password {
                    merged.database.password = Some(password.clone());
                }
                if let Some(ref template_db) = local_db.template_database {
                    merged.database.template_database = template_db.clone();
                }
                if let Some(ref prefix) = local_db.database_prefix {
                    merged.database.database_prefix = prefix.clone();
                }
                if let Some(ref auth) = local_db.auth {
                    if let Some(ref methods) = auth.methods {
                        merged.database.auth.methods = methods.clone();
                    }
                    if let Some(ref pgpass_file) = auth.pgpass_file {
                        merged.database.auth.pgpass_file = Some(pgpass_file.clone());
                    }
                    if let Some(ref service_name) = auth.service_name {
                        merged.database.auth.service_name = Some(service_name.clone());
                    }
                    if let Some(prompt_for_password) = auth.prompt_for_password {
                        merged.database.auth.prompt_for_password = prompt_for_password;
                    }
                }
            }

            if let Some(ref local_git) = local_config.git {
                if let Some(auto_create) = local_git.auto_create_on_branch {
                    merged.git.auto_create_on_branch = auto_create;
                }
                if let Some(auto_switch) = local_git.auto_switch_on_branch {
                    merged.git.auto_switch_on_branch = auto_switch;
                }
                if let Some(ref main_branch) = local_git.main_branch {
                    merged.git.main_branch = main_branch.clone();
                }
                if let Some(ref filter) = local_git.auto_create_branch_filter {
                    merged.git.auto_create_branch_filter = Some(filter.clone());
                }
                if let Some(ref regex) = local_git.branch_filter_regex {
                    merged.git.branch_filter_regex = Some(regex.clone());
                }
                if let Some(ref exclude_branches) = local_git.exclude_branches {
                    merged.git.exclude_branches = exclude_branches.clone();
                }
            }

            if let Some(ref local_behavior) = local_config.behavior {
                if let Some(auto_cleanup) = local_behavior.auto_cleanup {
                    merged.behavior.auto_cleanup = auto_cleanup;
                }
                if let Some(max_branches) = local_behavior.max_branches {
                    merged.behavior.max_branches = Some(max_branches);
                }
                if let Some(ref naming_strategy) = local_behavior.naming_strategy {
                    merged.behavior.naming_strategy = naming_strategy.clone();
                }
            }

            if let Some(ref post_commands) = local_config.post_commands {
                merged.post_commands = post_commands.clone();
            }

            if let Some(ref worktree) = local_config.worktree {
                merged.worktree = Some(worktree.clone());
            }
        }

        // Apply environment config overrides
        if let Some(ref host) = self.env_config.database_host {
            merged.database.host = host.clone();
        }
        if let Some(port) = self.env_config.database_port {
            merged.database.port = port;
        }
        if let Some(ref user) = self.env_config.database_user {
            merged.database.user = user.clone();
        }
        if let Some(ref password) = self.env_config.database_password {
            merged.database.password = Some(password.clone());
        }
        if let Some(ref prefix) = self.env_config.database_prefix {
            merged.database.database_prefix = prefix.clone();
        }
        if let Some(auto_create) = self.env_config.auto_create {
            merged.git.auto_create_on_branch = auto_create;
        }
        if let Some(auto_switch) = self.env_config.auto_switch {
            merged.git.auto_switch_on_branch = auto_switch;
        }
        if let Some(ref regex) = self.env_config.branch_filter_regex {
            merged.git.branch_filter_regex = Some(regex.clone());
        }

        merged
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
