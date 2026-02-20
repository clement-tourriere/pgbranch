use anyhow::{Context, Result};
use git2::Repository;
use std::fs;
use std::path::{Path, PathBuf};

pub struct GitRepository {
    repo: Repository,
}

impl GitRepository {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let repo = Repository::open(path).context("Failed to open Git repository")?;

        Ok(GitRepository { repo })
    }

    pub fn get_current_branch(&self) -> Result<Option<String>> {
        let head = self.repo.head().context("Failed to get HEAD reference")?;

        if let Some(branch_name) = head.shorthand() {
            Ok(Some(branch_name.to_string()))
        } else {
            Ok(None)
        }
    }

    pub fn branch_exists(&self, branch_name: &str) -> Result<bool> {
        match self.repo.find_branch(branch_name, git2::BranchType::Local) {
            Ok(_) => Ok(true),
            Err(e) => {
                if e.code() == git2::ErrorCode::NotFound {
                    Ok(false)
                } else {
                    Err(anyhow::anyhow!("Error checking branch: {}", e))
                }
            }
        }
    }

    pub fn detect_main_branch(&self) -> Result<Option<String>> {
        // Strategy 1: Check for remote's default branch (most reliable)
        if let Some(main_branch) = self.get_remote_default_branch()? {
            log::debug!("Found remote default branch: {}", main_branch);
            return Ok(Some(main_branch));
        }

        // Strategy 2: Check common main branch names that exist locally
        let common_main_branches = vec!["main", "master", "develop", "development"];
        for branch_name in common_main_branches {
            if self.branch_exists(branch_name)? {
                log::debug!("Found local main branch: {}", branch_name);
                return Ok(Some(branch_name.to_string()));
            }
        }

        // Strategy 3: Find the local branch that tracks a remote main branch
        if let Some(main_branch) = self.find_local_tracking_main_branch()? {
            log::debug!("Found local branch tracking remote main: {}", main_branch);
            return Ok(Some(main_branch));
        }

        // Strategy 4: Use current branch as last resort (original behavior)
        if let Some(current_branch) = self.get_current_branch()? {
            log::debug!("Using current branch as fallback main: {}", current_branch);
            return Ok(Some(current_branch));
        }

        Ok(None)
    }

    fn get_remote_default_branch(&self) -> Result<Option<String>> {
        // Try to get the default branch from the remote
        let mut found_default = None;

        // Get all remotes
        let remotes = self.repo.remotes()?;

        // Check origin first, then others
        let remote_names: Vec<&str> = if remotes.iter().any(|r| r == Some("origin")) {
            let mut names = vec!["origin"];
            names.extend(remotes.iter().flatten().filter(|&r| r != "origin"));
            names
        } else {
            remotes.iter().flatten().collect()
        };

        for remote_name in remote_names {
            if let Ok(_remote) = self.repo.find_remote(remote_name) {
                // Look for HEAD reference in remote
                let head_ref = format!("refs/remotes/{}/HEAD", remote_name);
                if let Ok(reference) = self.repo.find_reference(&head_ref) {
                    if let Some(target) = reference.symbolic_target() {
                        // Extract branch name from refs/remotes/origin/main -> main
                        let prefix = format!("refs/remotes/{}/", remote_name);
                        if target.starts_with(&prefix) {
                            let branch_name = target.strip_prefix(&prefix).unwrap();
                            found_default = Some(branch_name.to_string());
                            break;
                        }
                    }
                }
            }
        }

        Ok(found_default)
    }

    fn find_local_tracking_main_branch(&self) -> Result<Option<String>> {
        let branches = self.repo.branches(Some(git2::BranchType::Local))?;

        for branch_result in branches {
            let (branch, _) = branch_result?;
            if let Some(branch_name) = branch.name()? {
                // Check if this branch tracks a remote main/master branch
                if let Ok(upstream) = branch.upstream() {
                    if let Some(upstream_name) = upstream.name()? {
                        // Check if upstream is a main branch (contains main, master, etc.)
                        let upstream_lower = upstream_name.to_lowercase();
                        if upstream_lower.contains("main") || upstream_lower.contains("master") {
                            return Ok(Some(branch_name.to_string()));
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    #[allow(dead_code)]
    pub fn get_all_branches(&self) -> Result<Vec<String>> {
        let branches = self
            .repo
            .branches(Some(git2::BranchType::Local))
            .context("Failed to get branches")?;

        let mut branch_names = Vec::new();
        for branch in branches {
            let (branch, _) = branch.context("Failed to get branch")?;
            if let Some(name) = branch.name()? {
                branch_names.push(name.to_string());
            }
        }

        Ok(branch_names)
    }

    pub fn install_hooks(&self) -> Result<()> {
        let hooks_dir = self.repo.path().join("hooks");
        fs::create_dir_all(&hooks_dir).context("Failed to create hooks directory")?;

        let hook_script = self.generate_hook_script();

        let post_checkout_hook = hooks_dir.join("post-checkout");
        fs::write(&post_checkout_hook, &hook_script)
            .context("Failed to write post-checkout hook")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&post_checkout_hook)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&post_checkout_hook, perms)
                .context("Failed to set hook permissions")?;
        }

        let post_merge_hook = hooks_dir.join("post-merge");
        fs::write(&post_merge_hook, &hook_script).context("Failed to write post-merge hook")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&post_merge_hook)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&post_merge_hook, perms)
                .context("Failed to set hook permissions")?;
        }

        Ok(())
    }

    pub fn uninstall_hooks(&self) -> Result<()> {
        let hooks_dir = self.repo.path().join("hooks");

        let post_checkout_hook = hooks_dir.join("post-checkout");
        if post_checkout_hook.exists() && self.is_pgbranch_hook(&post_checkout_hook)? {
            fs::remove_file(&post_checkout_hook).context("Failed to remove post-checkout hook")?;
        }

        let post_merge_hook = hooks_dir.join("post-merge");
        if post_merge_hook.exists() && self.is_pgbranch_hook(&post_merge_hook)? {
            fs::remove_file(&post_merge_hook).context("Failed to remove post-merge hook")?;
        }

        Ok(())
    }

    fn generate_hook_script(&self) -> String {
        r#"#!/bin/sh
# pgbranch auto-generated hook
# This hook automatically creates database branches when switching Git branches

# For post-checkout hook, check if this is a branch checkout (not file checkout)
# Parameters: $1=previous HEAD, $2=new HEAD, $3=checkout type (1=branch, 0=file)
if [ "$3" = "0" ]; then
    # This is a file checkout, not a branch checkout - skip pgbranch execution
    exit 0
fi

# Detect if we're in a worktree (git-dir differs from common-dir)
GIT_DIR=$(git rev-parse --git-dir 2>/dev/null)
GIT_COMMON_DIR=$(git rev-parse --git-common-dir 2>/dev/null)

if [ "$GIT_DIR" != "$GIT_COMMON_DIR" ]; then
    # Worktree: resolve main worktree root from common dir
    MAIN_WORKTREE=$(cd "$GIT_COMMON_DIR/.." && pwd)
    if command -v pgbranch >/dev/null 2>&1; then
        pgbranch git-hook --worktree --main-worktree-dir "$MAIN_WORKTREE"
    fi
    exit 0
fi

# Regular checkout: skip if same branch
PREV_BRANCH=$(git reflog | awk 'NR==1{ print $6; exit }')
NEW_BRANCH=$(git reflog | awk 'NR==1{ print $8; exit }')

if [ "$PREV_BRANCH" = "$NEW_BRANCH" ]; then
    # This is the same branch checkout - skip pgbranch execution
    exit 0
fi

# Check if pgbranch is available
if command -v pgbranch >/dev/null 2>&1; then
    # Run pgbranch git-hook command to handle branch creation
    pgbranch git-hook
else
    echo "pgbranch not found in PATH, skipping database branch creation"
fi
"#
        .to_string()
    }

    pub fn is_pgbranch_hook(&self, hook_path: &Path) -> Result<bool> {
        if !hook_path.exists() {
            return Ok(false);
        }

        let content = fs::read_to_string(hook_path).context("Failed to read hook file")?;

        Ok(content.contains("pgbranch auto-generated hook"))
    }

    #[allow(dead_code)]
    pub fn get_repo_root(&self) -> &Path {
        self.repo.workdir().unwrap_or_else(|| self.repo.path())
    }

    pub fn is_worktree(&self) -> bool {
        self.repo.is_worktree()
    }

    pub fn get_main_worktree_dir(&self) -> Option<PathBuf> {
        if !self.repo.is_worktree() {
            return None;
        }
        self.repo.commondir().parent().map(|p| p.to_path_buf())
    }
}
