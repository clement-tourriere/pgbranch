use anyhow::{Context, Result};
use git2::Repository;
use std::fs;
use std::path::Path;

pub struct GitRepository {
    repo: Repository,
}

impl GitRepository {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let repo = Repository::open(path)
            .context("Failed to open Git repository")?;
        
        Ok(GitRepository { repo })
    }

    pub fn get_current_branch(&self) -> Result<Option<String>> {
        let head = self.repo.head()
            .context("Failed to get HEAD reference")?;
        
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


    #[allow(dead_code)]
    pub fn get_all_branches(&self) -> Result<Vec<String>> {
        let branches = self.repo.branches(Some(git2::BranchType::Local))
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
        fs::create_dir_all(&hooks_dir)
            .context("Failed to create hooks directory")?;
        
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
        fs::write(&post_merge_hook, &hook_script)
            .context("Failed to write post-merge hook")?;
        
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
            fs::remove_file(&post_checkout_hook)
                .context("Failed to remove post-checkout hook")?;
        }
        
        let post_merge_hook = hooks_dir.join("post-merge");
        if post_merge_hook.exists() && self.is_pgbranch_hook(&post_merge_hook)? {
            fs::remove_file(&post_merge_hook)
                .context("Failed to remove post-merge hook")?;
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

PREV_BRANCH=`git reflog | awk 'NR==1{ print $6; exit }'`
NEW_BRANCH=`git reflog | awk 'NR==1{ print $8; exit }'`

if [ "$PREV_BRANCH" == "$NEW_BRANCH" ]; then
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
"#.to_string()
    }

    fn is_pgbranch_hook(&self, hook_path: &Path) -> Result<bool> {
        if !hook_path.exists() {
            return Ok(false);
        }
        
        let content = fs::read_to_string(hook_path)
            .context("Failed to read hook file")?;
        
        Ok(content.contains("pgbranch auto-generated hook"))
    }

    #[allow(dead_code)]
    pub fn get_repo_root(&self) -> &Path {
        self.repo.workdir().unwrap_or_else(|| self.repo.path())
    }
}