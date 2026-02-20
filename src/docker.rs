use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct PostgresConfig {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DockerComposeService {
    environment: Option<DockerComposeEnvironment>,
    ports: Option<DockerComposePorts>,
    env_file: Option<DockerComposeEnvFile>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DockerComposePorts {
    Simple(Vec<String>),
    Complex(Vec<DockerComposePortMapping>),
}

#[derive(Debug, Deserialize)]
struct DockerComposePortMapping {
    #[allow(dead_code)]
    mode: Option<String>,
    target: Option<u16>,
    published: Option<DockerComposePortValue>,
    #[allow(dead_code)]
    protocol: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DockerComposePortValue {
    String(String),
    Number(u16),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DockerComposeEnvironment {
    List(Vec<String>),
    Map(HashMap<String, String>),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DockerComposeEnvFile {
    Single(String),
    List(Vec<String>),
}

#[derive(Debug, Deserialize)]
struct DockerComposeFile {
    services: Option<HashMap<String, DockerComposeService>>,
}

pub fn find_docker_compose_files() -> Vec<String> {
    let compose_filenames = vec![
        "docker-compose.yml",
        "docker-compose.yaml",
        "compose.yml",
        "compose.yaml",
        "docker-compose.override.yml",
        "docker-compose.override.yaml",
    ];

    compose_filenames
        .into_iter()
        .filter(|filename| Path::new(filename).exists())
        .map(|s| s.to_string())
        .collect()
}

pub fn parse_postgres_config_from_files(filenames: &[String]) -> Result<Option<PostgresConfig>> {
    let mut combined_config = PostgresConfig {
        host: None,
        port: None,
        user: None,
        password: None,
        database: None,
    };

    let mut found_any = false;

    for filename in filenames {
        if let Some(config) = parse_postgres_config_from_file(filename)? {
            found_any = true;

            // Override with values from this file if they exist
            if config.host.is_some() {
                combined_config.host = config.host;
            }
            if config.port.is_some() {
                combined_config.port = config.port;
            }
            if config.user.is_some() {
                combined_config.user = config.user;
            }
            if config.password.is_some() {
                combined_config.password = config.password;
            }
            if config.database.is_some() {
                combined_config.database = config.database;
            }
        }
    }

    if found_any {
        Ok(Some(combined_config))
    } else {
        Ok(None)
    }
}

fn parse_postgres_config_from_file(filename: &str) -> Result<Option<PostgresConfig>> {
    let content =
        fs::read_to_string(filename).with_context(|| format!("Failed to read {}", filename))?;

    let compose: DockerComposeFile = serde_yaml_ng::from_str(&content)
        .with_context(|| format!("Failed to parse {} as YAML", filename))?;

    let services = match compose.services {
        Some(services) => services,
        None => return Ok(None),
    };

    // Look for PostgreSQL-related services (postgres, postgresql, db, database, etc.)
    let postgres_service_names = ["postgres", "postgresql", "db", "database", "pg"];

    for (service_name, service) in &services {
        let service_name_lower = service_name.to_lowercase();

        // Check if this looks like a postgres service
        let is_postgres_service = postgres_service_names
            .iter()
            .any(|&pg_name| service_name_lower.contains(pg_name));

        if !is_postgres_service {
            log::debug!(
                "Skipping service '{}' - not a PostgreSQL service",
                service_name
            );
            continue;
        }

        log::debug!("Found PostgreSQL service: {}", service_name);

        // Collect environment variables from both environment and env_file
        let mut all_env_vars = HashMap::new();

        // First, add variables from environment section
        if let Some(ref environment) = service.environment {
            let env_vars = extract_environment_variables(environment);
            all_env_vars.extend(env_vars);
        }

        // Then, add variables from env_file(s) - these can override environment section
        if let Some(ref env_file) = service.env_file {
            let env_file_vars = extract_environment_from_files(env_file);
            all_env_vars.extend(env_file_vars);
        }

        if !all_env_vars.is_empty() {
            log::debug!(
                "Found {} environment variables for service '{}'",
                all_env_vars.len(),
                service_name
            );
            for (key, value) in &all_env_vars {
                if key.to_uppercase().contains("POSTGRES") {
                    log::debug!("  PostgreSQL env var: {}={}", key, value);
                }
            }

            let mut postgres_config = extract_postgres_config_from_env(&all_env_vars);

            // Check port mappings to find the actual exposed port
            if let Some(exposed_port) = extract_exposed_postgres_port(&service.ports) {
                postgres_config.port = Some(exposed_port);
            }

            if postgres_config.host.is_some()
                || postgres_config.port.is_some()
                || postgres_config.user.is_some()
                || postgres_config.password.is_some()
                || postgres_config.database.is_some()
            {
                log::debug!(
                    "Successfully extracted PostgreSQL config from service '{}'",
                    service_name
                );
                return Ok(Some(postgres_config));
            } else {
                log::debug!(
                    "No PostgreSQL configuration found in service '{}'",
                    service_name
                );
            }
        } else {
            log::debug!(
                "No environment variables found for service '{}'",
                service_name
            );
        }
    }

    Ok(None)
}

fn extract_environment_variables(
    environment: &DockerComposeEnvironment,
) -> HashMap<String, String> {
    let mut env_vars = HashMap::new();

    match environment {
        DockerComposeEnvironment::List(list) => {
            for item in list {
                if let Some((key, value)) = item.split_once('=') {
                    // Format: KEY=VALUE
                    env_vars.insert(key.to_string(), value.to_string());
                }
                // Note: For format "KEY" (without value), we skip it here and let env_file provide the value
            }
        }
        DockerComposeEnvironment::Map(map) => {
            for (key, value) in map {
                env_vars.insert(key.clone(), value.clone());
            }
        }
    }

    env_vars
}

fn extract_environment_from_files(env_file: &DockerComposeEnvFile) -> HashMap<String, String> {
    let mut env_vars = HashMap::new();

    let files = match env_file {
        DockerComposeEnvFile::Single(file) => vec![file.clone()],
        DockerComposeEnvFile::List(files) => files.clone(),
    };

    for file_path in files {
        match fs::read_to_string(&file_path) {
            Ok(content) => {
                log::debug!("Reading env file: {}", file_path);
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
                        log::debug!("Found env var from file {}: {}={}", file_path, key, value);
                        env_vars.insert(key.to_string(), value.to_string());
                    }
                }
            }
            Err(e) => {
                log::debug!("Could not read env file {}: {}", file_path, e);
            }
        }
    }

    env_vars
}

fn extract_postgres_config_from_env(env_vars: &HashMap<String, String>) -> PostgresConfig {
    let mut config = PostgresConfig {
        host: None,
        port: None,
        user: None,
        password: None,
        database: None,
    };

    // Host mappings
    for host_key in ["POSTGRES_HOST", "POSTGRESQL_HOST", "DB_HOST"] {
        if let Some(value) = env_vars.get(host_key) {
            config.host = Some(value.clone());
            break;
        }
    }

    // Port mappings
    for port_key in [
        "POSTGRES_PORT",
        "POSTGRES_PORT_HOST",
        "POSTGRESQL_PORT",
        "DB_PORT",
    ] {
        if let Some(value) = env_vars.get(port_key) {
            if let Ok(port) = value.parse::<u16>() {
                config.port = Some(port);
                break;
            }
        }
    }

    // User mappings
    for user_key in ["POSTGRES_USER", "POSTGRESQL_USER", "DB_USER"] {
        if let Some(value) = env_vars.get(user_key) {
            config.user = Some(value.clone());
            break;
        }
    }

    // Password mappings
    for password_key in ["POSTGRES_PASSWORD", "POSTGRESQL_PASSWORD", "DB_PASSWORD"] {
        if let Some(value) = env_vars.get(password_key) {
            config.password = Some(value.clone());
            break;
        }
    }

    // Database mappings
    for db_key in [
        "POSTGRES_DB",
        "POSTGRESQL_DB",
        "POSTGRES_DATABASE",
        "DB_NAME",
    ] {
        if let Some(value) = env_vars.get(db_key) {
            config.database = Some(value.clone());
            break;
        }
    }

    config
}

fn extract_exposed_postgres_port(ports: &Option<DockerComposePorts>) -> Option<u16> {
    let ports = match ports {
        Some(ports) => ports,
        None => return None,
    };

    match ports {
        DockerComposePorts::Simple(port_strings) => {
            // Handle simple port mappings like "5433:5432" or "5432"
            for port_string in port_strings {
                if let Some(exposed_port) = parse_simple_port_mapping(port_string) {
                    // Check if this maps to PostgreSQL port (5432)
                    if port_string.contains(":5432") || port_string == "5432" {
                        return Some(exposed_port);
                    }
                }
            }
        }
        DockerComposePorts::Complex(port_mappings) => {
            // Handle complex port mappings with target/published
            for mapping in port_mappings {
                // Check if this is mapping PostgreSQL port (5432)
                if mapping.target == Some(5432) {
                    if let Some(ref published) = mapping.published {
                        return match published {
                            DockerComposePortValue::String(s) => s.parse().ok(),
                            DockerComposePortValue::Number(n) => Some(*n),
                        };
                    }
                }
            }
        }
    }

    None
}

fn parse_simple_port_mapping(port_string: &str) -> Option<u16> {
    // Handle formats like:
    // "5433:5432" -> 5433
    // "5432" -> 5432
    // "127.0.0.1:5433:5432" -> 5433

    if port_string.contains(':') {
        let parts: Vec<&str> = port_string.split(':').collect();
        if parts.len() >= 2 {
            // For "host:published:target" or "published:target"
            let published_port = if parts.len() == 3 { parts[1] } else { parts[0] };
            return published_port.parse().ok();
        }
    } else {
        // Single port means same port for host and container
        return port_string.parse().ok();
    }

    None
}

pub fn prompt_user_for_config_usage(postgres_config: &PostgresConfig) -> Result<bool> {
    println!("🐳 Found PostgreSQL configuration in Docker Compose:");

    if let Some(ref host) = postgres_config.host {
        println!("  Host: {}", host);
    }
    if let Some(port) = postgres_config.port {
        println!("  Port: {}", port);
    }
    if let Some(ref user) = postgres_config.user {
        println!("  User: {}", user);
    }
    if postgres_config.password.is_some() {
        println!("  Password: [configured]");
    }
    if let Some(ref database) = postgres_config.database {
        println!("  Database: {}", database);
    }

    print!("\nWould you like to use these settings? (y/N): ");
    std::io::Write::flush(&mut std::io::stdout()).unwrap();

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("Failed to read user input")?;

    let response = input.trim().to_lowercase();
    Ok(response == "y" || response == "yes")
}
