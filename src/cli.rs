use anyhow::Result;
use clap::Subcommand;
use crate::config::Config;
use crate::database::DatabaseManager;
use crate::git::GitRepository;
use crate::docker;
use crate::post_commands::PostCommandExecutor;
use crate::local_state::LocalStateManager;

#[derive(Subcommand)]
pub enum Commands {
    #[command(about = "Create a new database branch")]
    Create {
        #[arg(help = "Name of the branch to create")]
        branch_name: String,
    },
    #[command(about = "Delete a database branch")]
    Delete {
        #[arg(help = "Name of the branch to delete")]
        branch_name: String,
    },
    #[command(about = "List all database branches")]
    List,
    #[command(about = "Initialize pgbranch configuration")]
    Init {
        #[arg(long, help = "Force overwrite existing configuration")]
        force: bool,
    },
    #[command(about = "Clean up old database branches")]
    Cleanup {
        #[arg(long, help = "Maximum number of branches to keep")]
        max_count: Option<usize>,
    },
    #[command(about = "Show current configuration")]
    Config,
    #[command(about = "Install Git hooks")]
    InstallHooks,
    #[command(about = "Uninstall Git hooks")]
    UninstallHooks,
    #[command(about = "Check configuration and database connectivity")]
    Check,
    #[command(about = "Handle Git hook execution (internal use)")]
    GitHook,
    #[command(about = "Show available template variables for post-commands")]
    Templates {
        #[arg(help = "Branch name to use for template variable examples")]
        branch_name: Option<String>,
    },
    #[command(about = "Test post-commands without database connection")]
    TestPostCommands {
        #[arg(help = "Branch name to test with")]
        branch_name: String,
    },
    #[command(about = "Switch to a PostgreSQL branch (creates if doesn't exist). If no branch name provided, shows interactive selection with arrow keys and fuzzy filtering.")]
    Switch {
        #[arg(help = "PostgreSQL branch name to switch to (optional - if omitted, shows interactive selection)")]
        branch_name: Option<String>,
        #[arg(long, help = "Switch to main database (template/development database)")]
        template: bool,
    },
    #[command(about = "Test switch functionality without database operations")]
    TestSwitch {
        #[arg(help = "PostgreSQL branch name to test switch to")]
        branch_name: String,
    },
}

pub async fn handle_command(cmd: Commands) -> Result<()> {
    // Check if command requires configuration file
    let requires_config = matches!(cmd, 
        Commands::Create { .. } | 
        Commands::Delete { .. } | 
        Commands::List | 
        Commands::Cleanup { .. } |
        Commands::GitHook |
        Commands::Templates { .. } |
        Commands::TestPostCommands { .. } |
        Commands::Switch { .. } |
        Commands::TestSwitch { .. }
    );
    
    let (mut config, config_path) = if requires_config {
        let (cfg, path) = Config::load_with_path_info()?;
        if path.is_none() {
            anyhow::bail!(
                "No configuration file found. Please run 'pgbranch init' to create a .pgbranch.yml file first."
            );
        }
        (cfg, path)
    } else {
        let (cfg, path) = Config::load_with_path_info()?;
        (cfg, path)
    };
    
    // Initialize local state manager for commands that need it
    let mut local_state = if requires_config {
        Some(LocalStateManager::new()?)
    } else {
        None
    };
    
    let db_manager = DatabaseManager::new(config.clone());
    
    match cmd {
        Commands::Create { branch_name } => {
            log::info!("Creating database branch: {}", branch_name);
            db_manager.create_database_branch(&branch_name).await?;
            println!("✅ Created database branch: {}", branch_name);
            
            // Execute post-commands
            if !config.post_commands.is_empty() {
                let executor = PostCommandExecutor::new(&config, &branch_name)?;
                executor.execute_all_post_commands().await?;
            }
        }
        Commands::Delete { branch_name } => {
            log::info!("Deleting database branch: {}", branch_name);
            db_manager.drop_database_branch(&branch_name).await?;
            println!("✅ Deleted database branch: {}", branch_name);
        }
        Commands::List => {
            match db_manager.list_database_branches().await {
                Ok(mut branches) => {
                    // Always add main branch at the beginning
                    branches.insert(0, "main".to_string());
                    
                    println!("📋 PostgreSQL branches:");
                    for branch in branches {
                        let current_branch = get_current_branch_with_default(&local_state, &config_path, &config);
                        let is_current = match current_branch {
                            Some(current) => {
                                if current == "_main" && branch == "main" {
                                    true
                                } else {
                                    current == branch
                                }
                            }
                            None => false
                        };
                        
                        let marker = if is_current { "* " } else { "  " };
                        
                        // Special display for main - inverse format: "* postgres (main)"
                        if branch == "main" {
                            println!("{}{} (main)", marker, config.database.template_database);
                        } else {
                            println!("{}{}", marker, branch);
                        }
                    }
                }
                Err(e) => {
                    // Even when database connection fails, show main and current branch from local state
                    println!("⚠️  Could not list database branches: {}", e);
                    println!("📋 PostgreSQL branches:");
                    
                    let current_branch = get_current_branch_with_default(&local_state, &config_path, &config);
                    
                    // Always show main branch
                    let main_marker = if current_branch == Some("_main".to_string()) {
                        "* "
                    } else {
                        "  "
                    };
                    println!("{}{} (main)", main_marker, config.database.template_database);
                    
                    // Show current branch from local state if it's not main
                    if let Some(current) = current_branch {
                        if current != "_main" {
                            println!("* {}", current);
                        }
                    }
                }
            }
        }
        Commands::Init { force } => {
            let config_path = std::env::current_dir()?.join(".pgbranch.yml");
            
            if config_path.exists() && !force {
                println!("❌ Configuration file already exists. Use --force to overwrite.");
                return Ok(());
            }
            
            let mut config = Config::default();
            
            // Auto-detect main Git branch using improved detection
            if let Ok(git_repo) = GitRepository::new(".") {
                if let Ok(Some(detected_main)) = git_repo.detect_main_branch() {
                    config.git.main_branch = detected_main.clone();
                    println!("🔍 Auto-detected main Git branch: {}", detected_main);
                } else {
                    println!("⚠️  Could not auto-detect main Git branch, using default: main");
                }
            }
            
            // Look for Docker Compose files and PostgreSQL configuration
            let compose_files = docker::find_docker_compose_files();
            if !compose_files.is_empty() {
                println!("🔍 Found Docker Compose files: {}", compose_files.join(", "));
                
                if let Some(postgres_config) = docker::parse_postgres_config_from_files(&compose_files)? {
                    if docker::prompt_user_for_config_usage(&postgres_config)? {
                        // Update config with Docker Compose values
                        if let Some(host) = postgres_config.host {
                            config.database.host = host;
                        }
                        if let Some(port) = postgres_config.port {
                            config.database.port = port;
                        }
                        if let Some(user) = postgres_config.user {
                            config.database.user = user;
                        }
                        if let Some(password) = postgres_config.password {
                            config.database.password = Some(password);
                        }
                        // Use template_database from Docker Compose database name if available
                        if let Some(database) = postgres_config.database {
                            config.database.template_database = database;
                        }
                        
                        println!("✅ Using PostgreSQL configuration from Docker Compose");
                    }
                } else {
                    println!("ℹ️  No PostgreSQL configuration found in Docker Compose files");
                }
            }
            
            config.save_to_file(&config_path)?;
            println!("✅ Initialized pgbranch configuration at: {}", config_path.display());
        }
        Commands::Cleanup { max_count } => {
            let max = max_count.unwrap_or(config.behavior.max_branches.unwrap_or(10));
            log::info!("Cleaning up old branches, keeping {} most recent", max);
            db_manager.cleanup_old_branches(max).await?;
            println!("✅ Cleaned up old database branches");
        }
        Commands::Config => {
            println!("Current configuration:");
            println!("{}", serde_yaml::to_string(&config)?);
        }
        Commands::InstallHooks => {
            let git_repo = GitRepository::new(".")?;
            git_repo.install_hooks()?;
            println!("✅ Installed Git hooks");
        }
        Commands::UninstallHooks => {
            let git_repo = GitRepository::new(".")?;
            git_repo.uninstall_hooks()?;
            println!("✅ Uninstalled Git hooks");
        }
        Commands::Check => {
            perform_system_check(&config, &db_manager, config_path).await?;
        }
        Commands::GitHook => {
            handle_git_hook(&mut config, &db_manager, &mut local_state, &config_path).await?;
        }
        Commands::Templates { branch_name } => {
            let example_branch = branch_name.unwrap_or_else(|| "feature/example-branch".to_string());
            let executor = PostCommandExecutor::new(&config, &example_branch)?;
            executor.print_template_variables();
        }
        Commands::TestPostCommands { branch_name } => {
            println!("🧪 Testing post-commands for branch: {}", branch_name);
            println!("💡 This simulates database creation without actually connecting to PostgreSQL\n");
            
            let executor = PostCommandExecutor::new(&config, &branch_name)?;
            executor.execute_all_post_commands().await?;
        }
        Commands::Switch { branch_name, template } => {
            if template {
                handle_switch_to_main(&mut config, &db_manager, &mut local_state, &config_path).await?;
            } else if let Some(branch) = branch_name {
                handle_switch_command(&mut config, &db_manager, &branch, &mut local_state, &config_path).await?;
            } else {
                handle_interactive_switch(&mut config, &db_manager, &mut local_state, &config_path).await?;
            }
        }
        Commands::TestSwitch { branch_name } => {
            handle_test_switch_command(&mut config, &branch_name).await?;
        }
    }
    
    Ok(())
}


async fn perform_system_check(config: &Config, db_manager: &DatabaseManager, config_path: Option<std::path::PathBuf>) -> Result<()> {
    println!("🔍 Performing system check...\n");
    
    let mut all_checks_passed = true;
    
    // Check 1: Configuration file validation
    print!("📋 Configuration file... ");
    match config_path {
        Some(path) => {
            match validate_config(config) {
                Ok(_) => println!("✅ Found and valid: {}", path.display()),
                Err(e) => {
                    println!("❌ Invalid: {}", e);
                    all_checks_passed = false;
                }
            }
        }
        None => {
            println!("⚠️  No configuration file found, using defaults (run 'pgbranch init' to create one)");
        }
    }
    
    // Check 2: PostgreSQL connection
    print!("🔌 PostgreSQL connection... ");
    match db_manager.connect().await {
        Ok(_) => println!("✅ Connected"),
        Err(e) => {
            println!("❌ Failed: {}", e);
            all_checks_passed = false;
        }
    }
    
    // Check 3: Template database existence
    print!("🗃️  Template database '{}'... ", config.database.template_database);
    match check_template_database(db_manager, &config.database.template_database).await {
        Ok(exists) => {
            if exists {
                println!("✅ Found");
            } else {
                println!("❌ Not found");
                all_checks_passed = false;
            }
        }
        Err(e) => {
            println!("❌ Error checking: {}", e);
            all_checks_passed = false;
        }
    }
    
    // Check 4: Database permissions
    print!("🔐 Database permissions... ");
    match check_database_permissions(db_manager).await {
        Ok(can_create) => {
            if can_create {
                println!("✅ Can create databases");
            } else {
                println!("❌ Cannot create databases");
                all_checks_passed = false;
            }
        }
        Err(e) => {
            println!("❌ Error checking permissions: {}", e);
            all_checks_passed = false;
        }
    }
    
    // Check 5: Git repository
    print!("📁 Git repository... ");
    match GitRepository::new(".") {
        Ok(_) => println!("✅ Valid Git repository"),
        Err(e) => {
            println!("❌ Not a Git repository: {}", e);
            all_checks_passed = false;
        }
    }
    
    // Check 6: Git hooks (if installed)
    print!("🪝 Git hooks... ");
    match check_git_hooks() {
        Ok(installed) => {
            if installed {
                println!("✅ Installed");
            } else {
                println!("⚠️  Not installed (run 'pgbranch install-hooks' to install)");
            }
        }
        Err(e) => {
            println!("❌ Error checking hooks: {}", e);
            all_checks_passed = false;
        }
    }
    
    // Check 7: Branch filtering regex (if configured)
    if let Some(regex_pattern) = &config.git.branch_filter_regex {
        print!("🔍 Branch filter regex... ");
        match regex::Regex::new(regex_pattern) {
            Ok(_) => println!("✅ Valid regex pattern"),
            Err(e) => {
                println!("❌ Invalid regex: {}", e);
                all_checks_passed = false;
            }
        }
    }
    
    println!();
    if all_checks_passed {
        println!("🎉 All checks passed! pgbranch is ready to use.");
    } else {
        println!("❌ Some checks failed. Please address the issues above.");
    }
    
    Ok(())
}

fn validate_config(config: &Config) -> Result<()> {
    if config.database.host.is_empty() {
        anyhow::bail!("Database host cannot be empty");
    }
    
    if config.database.port == 0 {
        anyhow::bail!("Database port must be greater than 0");
    }
    
    if config.database.user.is_empty() {
        anyhow::bail!("Database user cannot be empty");
    }
    
    if config.database.template_database.is_empty() {
        anyhow::bail!("Template database cannot be empty");
    }
    
    if config.database.database_prefix.is_empty() {
        anyhow::bail!("Database prefix cannot be empty");
    }
    
    Ok(())
}

async fn check_template_database(db_manager: &DatabaseManager, template_name: &str) -> Result<bool> {
    let client = db_manager.connect().await?;
    db_manager.database_exists(&client, template_name).await
}

async fn check_database_permissions(db_manager: &DatabaseManager) -> Result<bool> {
    let client = db_manager.connect().await?;
    
    // Try to check if user has CREATEDB privilege
    let query = r#"
        SELECT 1 FROM pg_user 
        WHERE usename = current_user 
        AND usecreatedb = true
    "#;
    
    let rows = client.query(query, &[]).await?;
    Ok(!rows.is_empty())
}

fn check_git_hooks() -> Result<bool> {
    // Use GitRepository to properly check for pgbranch-specific hooks
    match GitRepository::new(".") {
        Ok(git_repo) => {
            let hooks_dir = std::path::Path::new(".git/hooks");
            if !hooks_dir.exists() {
                return Ok(false);
            }
            
            let post_checkout_hook = hooks_dir.join("post-checkout");
            let post_merge_hook = hooks_dir.join("post-merge");
            
            // Check if the hooks exist AND are pgbranch hooks
            let has_post_checkout = post_checkout_hook.exists() && 
                git_repo.is_pgbranch_hook(&post_checkout_hook).unwrap_or(false);
            let has_post_merge = post_merge_hook.exists() && 
                git_repo.is_pgbranch_hook(&post_merge_hook).unwrap_or(false);
            
            Ok(has_post_checkout || has_post_merge)
        }
        Err(_) => {
            // If we can't access git repo, fall back to simple file existence check
            let hooks_dir = std::path::Path::new(".git/hooks");
            if !hooks_dir.exists() {
                return Ok(false);
            }
            
            let post_checkout_hook = hooks_dir.join("post-checkout");
            let post_merge_hook = hooks_dir.join("post-merge");
            
            Ok(post_checkout_hook.exists() || post_merge_hook.exists())
        }
    }
}

async fn handle_git_hook(config: &mut Config, db_manager: &DatabaseManager, local_state: &mut Option<LocalStateManager>, config_path: &Option<std::path::PathBuf>) -> Result<()> {
    let git_repo = GitRepository::new(".")?;
    
    if let Some(current_git_branch) = git_repo.get_current_branch()? {
        log::info!("Git hook triggered for branch: {}", current_git_branch);
        
        // Check if this branch should trigger a switch
        if config.should_switch_on_branch(&current_git_branch) {
            // If switching to main git branch, use main database
            if current_git_branch == config.git.main_branch {
                handle_switch_to_main(config, db_manager, local_state, config_path).await?;
            } else {
                // For other branches, check if we should create them and switch
                if config.should_create_branch(&current_git_branch) {
                    handle_switch_command(config, db_manager, &current_git_branch, local_state, config_path).await?;
                } else {
                    log::info!("Git branch {} configured not to create PostgreSQL branch", current_git_branch);
                }
            }
        } else {
            log::info!("Git branch {} filtered out by auto_switch configuration", current_git_branch);
        }
    }
    
    Ok(())
}

async fn handle_interactive_switch(config: &mut Config, db_manager: &DatabaseManager, local_state: &mut Option<LocalStateManager>, config_path: &Option<std::path::PathBuf>) -> Result<()> {
    // Get available branches
    let mut branches = match db_manager.list_database_branches().await {
        Ok(branches) => branches,
        Err(_) => {
            // If database connection fails, show current branch from local state or smart default (if not main)
            let mut fallback_branches = Vec::new();
            if let Some(current) = get_current_branch_with_default(local_state, config_path, config) {
                if current != "_main" {
                    fallback_branches.push(current);
                }
            }
            fallback_branches
        }
    };
    
    // Always add main at the beginning
    branches.insert(0, "main".to_string());
    
    // Create branch items with display info
    let branch_items: Vec<BranchItem> = branches.iter().map(|branch| {
        let current_branch = get_current_branch_with_default(local_state, config_path, config);
        let is_current = match current_branch {
            Some(current) => {
                if current == "_main" && branch == "main" {
                    true
                } else {
                    current == *branch
                }
            }
            None => false
        };
        
        let display_name = if branch == "main" {
            // Inverse format: "postgres (main)" instead of "main (postgres)"
            format!("{} (main)", config.database.template_database)
        } else {
            branch.clone()
        };
        
        BranchItem {
            name: branch.clone(),
            display_name,
            is_current,
        }
    }).collect();
    
    // Run interactive selector
    match run_interactive_selector(branch_items) {
        Ok(selected_branch) => {
            if selected_branch == "main" {
                handle_switch_to_main(config, db_manager, local_state, config_path).await?;
            } else {
                handle_switch_command(config, db_manager, &selected_branch, local_state, config_path).await?;
            }
        }
        Err(e) => {
            match e {
                inquire::InquireError::OperationCanceled => {
                    println!("Cancelled.");
                }
                inquire::InquireError::OperationInterrupted => {
                    println!("Interrupted.");
                }
                _ => {
                    println!("⚠️  Interactive mode failed: {}", e);
                    println!("💡 Try using: pgbranch switch <branch-name> or pgbranch switch --template");
                }
            }
        }
    }
    
    Ok(())
}

#[derive(Clone)]
struct BranchItem {
    name: String,
    display_name: String,
    is_current: bool,
}

fn run_interactive_selector(items: Vec<BranchItem>) -> Result<String, inquire::InquireError> {
    use inquire::Select;
    
    if items.is_empty() {
        return Err(inquire::InquireError::InvalidConfiguration("No branches available".to_string()));
    }
    
    // Create display options with current branch marker
    let options: Vec<String> = items.iter().map(|item| {
        if item.is_current {
            format!("{} ★", item.display_name)
        } else {
            item.display_name.clone()
        }
    }).collect();
    
    // Find the default selection (current branch if available)
    let default = items.iter().position(|item| item.is_current);
    
    let mut select = Select::new("Select a PostgreSQL branch to switch to:", options.clone())
        .with_help_message("Use ↑/↓ to navigate, type to filter, Enter to select, Esc to cancel");
    
    if let Some(default_index) = default {
        select = select.with_starting_cursor(default_index);
    }
    
    // Run the selector
    let selected_display = select.prompt()?;
    
    // Find the corresponding branch name (remove the ★ marker if present)
    let selected_index = options.iter().position(|opt| opt == &selected_display)
        .ok_or_else(|| inquire::InquireError::InvalidConfiguration("Selected option not found".to_string()))?;
    
    Ok(items[selected_index].name.clone())
}

async fn handle_switch_command(config: &mut Config, db_manager: &DatabaseManager, branch_name: &str, local_state: &mut Option<LocalStateManager>, config_path: &Option<std::path::PathBuf>) -> Result<()> {
    // Normalize the branch name (feature/auth → feature_auth)
    let normalized_branch = config.get_normalized_branch_name(branch_name);
    
    println!("🔄 Switching to PostgreSQL branch: {}", normalized_branch);
    
    // Update current branch in local state first (so it persists even if DB operations fail)
    set_current_branch(local_state, config_path, Some(normalized_branch.clone()))?;
    
    // Try database operations (non-fatal if they fail)
    match db_manager.list_database_branches().await {
        Ok(db_branches) => {
            if !db_branches.contains(&normalized_branch) {
                println!("📦 Creating database branch: {}", normalized_branch);
                match db_manager.create_database_branch(&normalized_branch).await {
                    Ok(_) => println!("✅ Created database branch: {}", normalized_branch),
                    Err(e) => {
                        println!("⚠️  Failed to create database branch: {}", e);
                        println!("💡 Branch state updated in config, but database operation failed");
                    }
                }
            }
        }
        Err(e) => {
            println!("⚠️  Failed to connect to database: {}", e);
            println!("💡 Branch state updated in config, but couldn't verify database");
        }
    }
    
    println!("✅ Switched to PostgreSQL branch: {}", normalized_branch);
    
    // Execute post-commands
    if !config.post_commands.is_empty() {
        println!("🔧 Executing post-commands for branch switch...");
        let executor = PostCommandExecutor::new(config, &normalized_branch)?;
        executor.execute_all_post_commands().await?;
    }
    
    Ok(())
}

async fn handle_switch_to_main(config: &mut Config, _db_manager: &DatabaseManager, local_state: &mut Option<LocalStateManager>, config_path: &Option<std::path::PathBuf>) -> Result<()> {
    let main_name = "_main";
    
    println!("🔄 Switching to main database");
    
    // Update current branch in local state to a special main marker
    set_current_branch(local_state, config_path, Some(main_name.to_string()))?;
    
    println!("✅ Switched to main database: {}", config.database.template_database);
    
    // Execute post-commands with main branch
    if !config.post_commands.is_empty() {
        println!("🔧 Executing post-commands for main switch...");
        let executor = PostCommandExecutor::new(config, main_name)?;
        executor.execute_all_post_commands().await?;
    }
    
    Ok(())
}

async fn handle_test_switch_command(config: &mut Config, branch_name: &str) -> Result<()> {
    // Normalize the branch name (feature/auth → feature_auth)
    let normalized_branch = config.get_normalized_branch_name(branch_name);
    
    println!("🧪 Testing switch to PostgreSQL branch: {}", normalized_branch);
    println!("💡 This simulates branch switching without database operations\n");
    
    // Note: For test mode, we don't update local state
    // The normalized branch is only shown for demonstration
    
    println!("✅ Updated current branch to: {}", normalized_branch);
    
    // Execute post-commands
    if !config.post_commands.is_empty() {
        println!("🔧 Executing post-commands for branch switch...");
        let executor = PostCommandExecutor::new(config, &normalized_branch)?;
        executor.execute_all_post_commands().await?;
    }
    
    Ok(())
}

// Helper functions for current branch management with local state
fn get_current_branch(local_state: &Option<LocalStateManager>, config_path: &Option<std::path::PathBuf>) -> Option<String> {
    if let (Some(state_manager), Some(path)) = (local_state, config_path) {
        state_manager.get_current_branch(path)
    } else {
        None
    }
}

fn get_current_branch_with_default(
    local_state: &Option<LocalStateManager>, 
    config_path: &Option<std::path::PathBuf>,
    config: &Config
) -> Option<String> {
    // First check if we have local state
    if let Some(current) = get_current_branch(local_state, config_path) {
        return Some(current);
    }
    
    // No local state found, try to detect smart default
    detect_default_current_branch(config)
}


fn detect_default_current_branch(config: &Config) -> Option<String> {
    // Try to get current Git branch to make intelligent default
    match GitRepository::new(".") {
        Ok(git_repo) => {
            if let Ok(Some(current_git_branch)) = git_repo.get_current_branch() {
                log::debug!("Detecting default current branch from Git branch: {}", current_git_branch);
                
                // If on main Git branch, default to main database
                if current_git_branch == config.git.main_branch {
                    log::debug!("On main Git branch, defaulting to main database");
                    return Some("_main".to_string());
                }
                
                // If current Git branch would create a database branch, default to that
                if config.should_create_branch(&current_git_branch) {
                    let normalized_branch = config.get_normalized_branch_name(&current_git_branch);
                    log::debug!("Git branch matches create filter, defaulting to: {}", normalized_branch);
                    return Some(normalized_branch);
                }
                
                // Git branch exists but doesn't match filters, default to main
                log::debug!("Git branch doesn't match filters, defaulting to main database");
                return Some("_main".to_string());
            }
        }
        Err(e) => {
            log::debug!("Could not access Git repository: {}", e);
        }
    }
    
    // Fallback to main database if Git detection fails
    log::debug!("Git detection failed, defaulting to main database");
    Some("_main".to_string())
}

fn set_current_branch(local_state: &mut Option<LocalStateManager>, config_path: &Option<std::path::PathBuf>, branch: Option<String>) -> Result<()> {
    if let (Some(state_manager), Some(path)) = (local_state, config_path) {
        state_manager.set_current_branch(path, branch)?;
    }
    Ok(())
}
