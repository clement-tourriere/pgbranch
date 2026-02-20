use crate::config::NamedBackendConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LocalState {
    pub projects: HashMap<String, ProjectState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectState {
    pub current_branch: Option<String>,
    pub last_updated: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backends: Option<Vec<NamedBackendConfig>>,
}

pub struct LocalStateManager {
    state_file_path: PathBuf,
    state: LocalState,
}

impl LocalStateManager {
    pub fn new() -> Result<Self> {
        let state_file_path = Self::get_state_file_path()?;
        let state = Self::load_state(&state_file_path)?;

        Ok(Self {
            state_file_path,
            state,
        })
    }

    pub fn get_current_branch(&self, project_path: &Path) -> Option<String> {
        let project_key = self.get_project_key(project_path)?;
        self.state
            .projects
            .get(&project_key)
            .and_then(|project| project.current_branch.clone())
    }

    pub fn set_current_branch(
        &mut self,
        project_path: &Path,
        branch: Option<String>,
    ) -> Result<()> {
        let project_key = self.get_project_key(project_path).ok_or_else(|| {
            anyhow::anyhow!(
                "Failed to get project key for path: {}",
                project_path.display()
            )
        })?;

        // Preserve existing backends when updating current branch
        let existing_backends = self
            .state
            .projects
            .get(&project_key)
            .and_then(|p| p.backends.clone());

        let project_state = ProjectState {
            current_branch: branch,
            last_updated: chrono::Utc::now(),
            backends: existing_backends,
        };

        self.state.projects.insert(project_key, project_state);
        self.save_state()?;

        Ok(())
    }

    pub fn get_backends(&self, project_path: &Path) -> Option<Vec<NamedBackendConfig>> {
        let project_key = self.get_project_key(project_path)?;
        self.state
            .projects
            .get(&project_key)
            .and_then(|project| project.backends.clone())
    }

    pub fn set_backends(
        &mut self,
        project_path: &Path,
        backends: Vec<NamedBackendConfig>,
    ) -> Result<()> {
        let project_key = self.get_project_key(project_path).ok_or_else(|| {
            anyhow::anyhow!(
                "Failed to get project key for path: {}",
                project_path.display()
            )
        })?;

        let existing = self.state.projects.get(&project_key);
        let current_branch = existing.and_then(|p| p.current_branch.clone());

        let project_state = ProjectState {
            current_branch,
            last_updated: chrono::Utc::now(),
            backends: Some(backends),
        };

        self.state.projects.insert(project_key, project_state);
        self.save_state()?;
        Ok(())
    }

    pub fn add_backend(
        &mut self,
        project_path: &Path,
        backend: NamedBackendConfig,
        force: bool,
    ) -> Result<()> {
        let project_key = self.get_project_key(project_path).ok_or_else(|| {
            anyhow::anyhow!(
                "Failed to get project key for path: {}",
                project_path.display()
            )
        })?;

        let existing = self.state.projects.get(&project_key);
        let current_branch = existing.and_then(|p| p.current_branch.clone());
        let mut backends = existing
            .and_then(|p| p.backends.clone())
            .unwrap_or_default();

        if let Some(pos) = backends.iter().position(|b| b.name == backend.name) {
            if force {
                backends[pos] = backend;
            } else {
                anyhow::bail!(
                    "Backend '{}' already exists. Use --force to overwrite.",
                    backends[pos].name
                );
            }
        } else {
            let mut backend = backend;
            if backends.is_empty() {
                backend.default = true;
            }
            backends.push(backend);
        }

        let project_state = ProjectState {
            current_branch,
            last_updated: chrono::Utc::now(),
            backends: Some(backends),
        };

        self.state.projects.insert(project_key, project_state);
        self.save_state()?;
        Ok(())
    }

    pub fn remove_backend(&mut self, project_path: &Path, name: &str) -> Result<()> {
        let project_key = self.get_project_key(project_path).ok_or_else(|| {
            anyhow::anyhow!(
                "Failed to get project key for path: {}",
                project_path.display()
            )
        })?;

        if let Some(project) = self.state.projects.get_mut(&project_key) {
            if let Some(ref mut backends) = project.backends {
                backends.retain(|b| b.name != name);
            }
            project.last_updated = chrono::Utc::now();
            self.save_state()?;
        }

        Ok(())
    }

    #[allow(dead_code)]
    pub fn cleanup_old_projects(&mut self, max_age_days: u32) -> Result<()> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(max_age_days as i64);

        let old_projects: Vec<String> = self
            .state
            .projects
            .iter()
            .filter(|(_, project)| project.last_updated < cutoff)
            .map(|(key, _)| key.clone())
            .collect();

        let mut projects_removed = false;
        for project_key in &old_projects {
            // Check if project still exists before removing
            if let Ok(path) = PathBuf::from(&project_key).canonicalize() {
                if !path.exists() {
                    log::debug!("Removing state for non-existent project: {}", project_key);
                    self.state.projects.remove(project_key);
                    projects_removed = true;
                }
            } else {
                log::debug!("Removing state for inaccessible project: {}", project_key);
                self.state.projects.remove(project_key);
                projects_removed = true;
            }
        }

        if projects_removed {
            self.save_state()?;
        }

        Ok(())
    }

    fn get_project_key(&self, project_path: &Path) -> Option<String> {
        // Use the canonical path of the directory containing .pgbranch.yml as the project key
        project_path
            .parent()
            .and_then(|dir| dir.canonicalize().ok())
            .map(|canonical_path| canonical_path.to_string_lossy().to_string())
    }

    fn get_state_file_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Failed to get user config directory")?
            .join("pgbranch");

        // Ensure the config directory exists
        fs::create_dir_all(&config_dir).with_context(|| {
            format!(
                "Failed to create config directory: {}",
                config_dir.display()
            )
        })?;

        Ok(config_dir.join("local_state.yml"))
    }

    fn load_state(state_file_path: &Path) -> Result<LocalState> {
        if !state_file_path.exists() {
            log::debug!("Local state file does not exist, creating new state");
            return Ok(LocalState::default());
        }

        let content = fs::read_to_string(state_file_path).with_context(|| {
            format!(
                "Failed to read local state file: {}",
                state_file_path.display()
            )
        })?;

        let state: LocalState = serde_yaml_ng::from_str(&content).with_context(|| {
            format!(
                "Failed to parse local state file: {}",
                state_file_path.display()
            )
        })?;

        log::debug!("Loaded local state with {} projects", state.projects.len());
        Ok(state)
    }

    fn save_state(&self) -> Result<()> {
        let content = serde_yaml_ng::to_string(&self.state)
            .context("Failed to serialize local state to YAML")?;

        fs::write(&self.state_file_path, content).with_context(|| {
            format!(
                "Failed to write local state file: {}",
                self.state_file_path.display()
            )
        })?;

        log::debug!("Saved local state to: {}", self.state_file_path.display());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_project_key_generation() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join(".pgbranch.yml");

        let manager = LocalStateManager::new().unwrap();
        let project_key = manager.get_project_key(&config_path);

        assert!(project_key.is_some());
        assert!(project_key
            .unwrap()
            .contains(temp_dir.path().to_str().unwrap()));
    }

    #[test]
    fn test_current_branch_operations() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join(".pgbranch.yml");

        let mut manager = LocalStateManager::new().unwrap();

        // Initially no current branch
        assert_eq!(manager.get_current_branch(&config_path), None);

        // Set a branch
        manager
            .set_current_branch(&config_path, Some("feature_test".to_string()))
            .unwrap();
        assert_eq!(
            manager.get_current_branch(&config_path),
            Some("feature_test".to_string())
        );

        // Update branch
        manager
            .set_current_branch(&config_path, Some("main".to_string()))
            .unwrap();
        assert_eq!(
            manager.get_current_branch(&config_path),
            Some("main".to_string())
        );

        // Clear branch
        manager.set_current_branch(&config_path, None).unwrap();
        assert_eq!(manager.get_current_branch(&config_path), None);
    }
}
