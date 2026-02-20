use crate::config::{Config, PostCommand, ReplaceConfig, TemplateContext};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::process::Command;

pub struct PostCommandExecutor<'a> {
    config: &'a Config,
    context: TemplateContext,
    working_dir: std::path::PathBuf,
}

impl<'a> PostCommandExecutor<'a> {
    pub fn new(config: &'a Config, branch_name: &str) -> Result<Self> {
        let context = TemplateContext::new(config, branch_name);
        let working_dir =
            std::env::current_dir().context("Failed to get current working directory")?;

        Ok(Self {
            config,
            context,
            working_dir,
        })
    }

    pub async fn execute_all_post_commands(&self) -> Result<()> {
        if self.config.post_commands.is_empty() {
            log::debug!("No post-commands configured");
            return Ok(());
        }

        println!("🔧 Executing post-commands...");

        for (index, post_command) in self.config.post_commands.iter().enumerate() {
            match self.execute_post_command(post_command, index).await {
                Ok(_) => {}
                Err(e) => {
                    let continue_on_error = match post_command {
                        PostCommand::Simple(_) => false,
                        PostCommand::Complex(config) => config.continue_on_error.unwrap_or(false),
                        PostCommand::Replace(config) => config.continue_on_error.unwrap_or(false),
                    };

                    if continue_on_error {
                        log::warn!("Post-command {} failed but continuing: {}", index + 1, e);
                        println!("⚠️  Command {} failed but continuing: {}", index + 1, e);
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        println!("✅ All post-commands completed successfully");
        Ok(())
    }

    async fn execute_post_command(&self, post_command: &PostCommand, index: usize) -> Result<()> {
        match post_command {
            PostCommand::Simple(command_str) => {
                let substituted_command = self
                    .config
                    .substitute_template_variables(command_str, &self.context);
                self.execute_command_string(&substituted_command, None, None, index)
                    .await
            }
            PostCommand::Complex(config) => {
                let substituted_command = self
                    .config
                    .substitute_template_variables(&config.command, &self.context);

                // Check condition if specified
                if let Some(ref condition) = config.condition {
                    if !self.evaluate_condition(condition)? {
                        log::debug!(
                            "Skipping command {} due to condition: {}",
                            index + 1,
                            condition
                        );
                        if let Some(ref name) = config.name {
                            println!("⏭️  Skipped: {}", name);
                        } else {
                            println!("⏭️  Skipped command {}", index + 1);
                        }
                        return Ok(());
                    }
                }

                self.execute_command_string(
                    &substituted_command,
                    config.working_dir.as_deref(),
                    config.environment.as_ref(),
                    index,
                )
                .await
            }
            PostCommand::Replace(config) => {
                // Check condition if specified
                if let Some(ref condition) = config.condition {
                    if !self.evaluate_condition(condition)? {
                        log::debug!(
                            "Skipping replace {} due to condition: {}",
                            index + 1,
                            condition
                        );
                        if let Some(ref name) = config.name {
                            println!("⏭️  Skipped: {}", name);
                        } else {
                            println!("⏭️  Skipped replace {}", index + 1);
                        }
                        return Ok(());
                    }
                }

                self.execute_replace_action(config, index).await
            }
        }
    }

    async fn execute_command_string(
        &self,
        command: &str,
        working_dir: Option<&str>,
        environment: Option<&HashMap<String, String>>,
        index: usize,
    ) -> Result<()> {
        let cmd_working_dir = if let Some(wd) = working_dir {
            self.working_dir.join(wd)
        } else {
            self.working_dir.clone()
        };

        log::info!("Executing post-command {}: {}", index + 1, command);
        println!("▶️  Executing: {}", command);

        let mut cmd = if cfg!(target_os = "windows") {
            let mut cmd = Command::new("cmd");
            cmd.args(["/C", command]);
            cmd
        } else {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", command]);
            cmd
        };

        cmd.current_dir(&cmd_working_dir);

        // Set environment variables
        if let Some(env_vars) = environment {
            for (key, value) in env_vars {
                let substituted_value = self
                    .config
                    .substitute_template_variables(value, &self.context);
                cmd.env(key, substituted_value);
            }
        }

        let output = cmd
            .output()
            .with_context(|| format!("Failed to execute command: {}", command))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);

            return Err(anyhow::anyhow!(
                "Command failed with exit code {}: {}\nStdout: {}\nStderr: {}",
                output.status.code().unwrap_or(-1),
                command,
                stdout,
                stderr
            ));
        }

        // Print command output if it's not empty
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.trim().is_empty() {
            println!("{}", stdout.trim());
        }

        Ok(())
    }

    fn evaluate_condition(&self, condition: &str) -> Result<bool> {
        if let Some(file_path) = condition.strip_prefix("file_exists:") {
            let substituted_path = self
                .config
                .substitute_template_variables(file_path, &self.context);
            let full_path = self.working_dir.join(substituted_path);
            Ok(full_path.exists())
        } else if let Some(dir_path) = condition.strip_prefix("dir_exists:") {
            let substituted_path = self
                .config
                .substitute_template_variables(dir_path, &self.context);
            let full_path = self.working_dir.join(substituted_path);
            Ok(full_path.is_dir())
        } else if condition == "always" {
            Ok(true)
        } else if condition == "never" {
            Ok(false)
        } else {
            Err(anyhow::anyhow!("Unknown condition: {}", condition))
        }
    }

    async fn execute_replace_action(&self, config: &ReplaceConfig, _index: usize) -> Result<()> {
        let file_path = self
            .config
            .substitute_template_variables(&config.file, &self.context);
        let pattern = self
            .config
            .substitute_template_variables(&config.pattern, &self.context);
        let replacement = self
            .config
            .substitute_template_variables(&config.replacement, &self.context);

        if let Some(ref name) = config.name {
            println!("🔄 Replacing: {}", name);
        } else {
            println!("🔄 Replacing in file: {}", file_path);
        }

        let file_exists = std::path::Path::new(&file_path).exists();

        if !file_exists {
            if config.create_if_missing.unwrap_or(false) {
                // Create file with the replacement content
                std::fs::write(&file_path, &replacement)
                    .with_context(|| format!("Failed to create file: {}", file_path))?;
                println!("✅ Created file: {}", file_path);
                return Ok(());
            } else {
                return Err(anyhow::anyhow!("File does not exist: {}", file_path));
            }
        }

        // Read the file
        let content = std::fs::read_to_string(&file_path)
            .with_context(|| format!("Failed to read file: {}", file_path))?;

        // Use regex for pattern matching
        let re = regex::Regex::new(&pattern)
            .with_context(|| format!("Invalid regex pattern: {}", pattern))?;

        let new_content = re.replace_all(&content, replacement.as_str());

        // Write back only if content changed
        if new_content != content {
            std::fs::write(&file_path, new_content.as_ref())
                .with_context(|| format!("Failed to write file: {}", file_path))?;
            println!("✅ Updated file: {}", file_path);
        } else {
            println!("ℹ️  No changes needed in: {}", file_path);
        }

        Ok(())
    }
}
