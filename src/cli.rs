use std::path::PathBuf;

use crate::backends;
use crate::config::{Config, EffectiveConfig};
#[cfg(feature = "backend-postgres-template")]
use crate::database::DatabaseManager;
use crate::docker;
use crate::git::GitRepository;
use crate::local_state::LocalStateManager;
use crate::post_commands::PostCommandExecutor;
use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum Commands {
    #[command(about = "Create a new database branch")]
    Create {
        #[arg(help = "Name of the branch to create")]
        branch_name: String,
        #[arg(long, help = "Parent branch to clone from")]
        from: Option<String>,
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
        #[arg(help = "Database/backend name (defaults to project directory name)")]
        name: Option<String>,
        #[arg(long, help = "Force overwrite existing configuration")]
        force: bool,
        #[arg(
            long,
            help = "Backend type to use (local, postgres_template, neon, dblab, xata)"
        )]
        backend: Option<String>,
        #[arg(
            long,
            help = "Seed main branch from source (PostgreSQL URL, file path, or s3:// URL)"
        )]
        from: Option<String>,
    },
    #[command(about = "Clean up old database branches")]
    Cleanup {
        #[arg(long, help = "Maximum number of branches to keep")]
        max_count: Option<usize>,
    },
    #[command(about = "Show current configuration")]
    Config {
        #[arg(
            short,
            long,
            help = "Show effective configuration with precedence details"
        )]
        verbose: bool,
    },
    #[command(about = "Install Git hooks")]
    InstallHooks,
    #[command(about = "Uninstall Git hooks")]
    UninstallHooks,
    #[command(about = "Handle Git hook execution", hide = true)]
    GitHook {
        #[arg(long, hide = true)]
        worktree: bool,
        #[arg(long, hide = true)]
        main_worktree_dir: Option<String>,
    },
    #[command(about = "Switch to a database branch (creates if doesn't exist)")]
    Switch {
        #[arg(
            help = "Branch name to switch to (optional - if omitted, shows interactive selection)"
        )]
        branch_name: Option<String>,
        #[arg(long, help = "Switch to main database (template/development database)")]
        template: bool,
        #[arg(long, help = "Simulate switching without database operations")]
        dry_run: bool,
    },
    #[command(about = "Start a stopped database branch container (local backend)")]
    Start {
        #[arg(help = "Name of the branch to start")]
        branch_name: String,
    },
    #[command(about = "Stop a running database branch container (local backend)")]
    Stop {
        #[arg(help = "Name of the branch to stop")]
        branch_name: String,
    },
    #[command(about = "Reset a database branch to its parent state (local backend)")]
    Reset {
        #[arg(help = "Name of the branch to reset")]
        branch_name: String,
    },
    #[command(about = "Run diagnostics and check system health")]
    Doctor,
    #[command(about = "Show connection info for a database branch")]
    Connection {
        #[arg(help = "Name of the branch")]
        branch_name: String,
        #[arg(long, help = "Output format: uri, env, or json")]
        format: Option<String>,
    },
    #[command(about = "Show current project and backend status")]
    Status,
    #[command(about = "Destroy a database and all its branches (local backend)")]
    Destroy {
        #[arg(long, help = "Skip confirmation prompt")]
        force: bool,
    },
    #[command(
        name = "worktree-setup",
        about = "Set up pgbranch in a Git worktree (copy files, create DB branch)"
    )]
    WorktreeSetup,
    #[command(
        name = "setup-zfs",
        about = "Set up a file-backed ZFS pool for Copy-on-Write storage (Linux)"
    )]
    SetupZfs {
        #[arg(long, default_value = "pgbranch", help = "ZFS pool name")]
        pool_name: Option<String>,
        #[arg(long, default_value = "10G", help = "Pool image size (sparse file)")]
        size: Option<String>,
    },
}

pub async fn handle_command(
    cmd: Commands,
    json_output: bool,
    _non_interactive: bool,
    database_name: Option<&str>,
) -> Result<()> {
    // Commands that use the new backend system
    let uses_backend = matches!(
        cmd,
        Commands::Create { .. }
            | Commands::Delete { .. }
            | Commands::List
            | Commands::Start { .. }
            | Commands::Stop { .. }
            | Commands::Reset { .. }
            | Commands::Doctor
            | Commands::Connection { .. }
            | Commands::Status
            | Commands::Cleanup { .. }
            | Commands::Destroy { .. }
    );

    // Commands that use the legacy direct-database approach
    let uses_legacy = matches!(
        cmd,
        Commands::GitHook { .. } | Commands::Switch { .. } | Commands::WorktreeSetup
    );

    // Check if command requires configuration file
    let requires_config = uses_backend || uses_legacy;

    // Load effective configuration (includes local config and environment overrides)
    let (effective_config, config_path) = Config::load_effective_config_with_path_info()?;

    // Early exit if pgbranch is disabled
    if effective_config.should_exit_early()? {
        if effective_config.is_disabled() {
            log::debug!("pgbranch is globally disabled via configuration");
        } else {
            log::debug!("pgbranch is disabled for current branch");
        }
        return Ok(());
    }

    // Check for required config file after checking if disabled
    if requires_config && config_path.is_none() {
        // For backend commands, we allow no config (will use local backend defaults)
        if uses_legacy {
            anyhow::bail!(
                "No configuration file found. Please run 'pgbranch init' to create a .pgbranch.yml file first."
            );
        }
    }

    // Get the merged configuration for normal operations
    let mut config = effective_config.get_merged_config();

    // Inject backends from state (state backends take precedence over committed)
    let local_state_for_backends = if uses_backend || uses_legacy {
        LocalStateManager::new().ok()
    } else {
        None
    };
    if let Some(ref state_manager) = local_state_for_backends {
        if let Some(ref path) = config_path {
            if let Some(state_backends) = state_manager.get_backends(path) {
                config.backends = Some(state_backends);
                config.backend = None;
            }
        }
    }

    // Handle backend-based commands
    if uses_backend {
        // For doctor, run config/git pre-checks before backend-specific checks
        if matches!(cmd, Commands::Doctor) && !json_output {
            run_doctor_pre_checks(&config, &config_path);
        }
        return handle_backend_command(
            cmd,
            &mut config,
            json_output,
            _non_interactive,
            database_name,
            &config_path,
        )
        .await;
    }

    // Initialize local state manager for commands that need it
    let mut local_state = if requires_config {
        Some(LocalStateManager::new()?)
    } else {
        None
    };

    #[cfg(feature = "backend-postgres-template")]
    let db_manager = DatabaseManager::new(config.clone());

    match cmd {
        Commands::Init {
            name,
            force,
            backend,
            from,
        } => {
            let config_path = std::env::current_dir()?.join(".pgbranch.yml");

            // Resolve the name: if None, derive from current directory
            let resolved_name = match name {
                Some(n) => n,
                None => std::env::current_dir()?
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "default".to_string()),
            };

            let backend_type = backend.as_deref().unwrap_or("local").to_string();
            let is_local = backends::factory::BackendType::is_local(&backend_type);
            let is_postgres_template = matches!(
                backend_type.as_str(),
                "postgres_template" | "postgres" | "postgresql"
            );

            if config_path.exists() {
                // --- Subsequent init: add a new backend to state (don't modify .pgbranch.yml) ---
                let config = Config::from_file(&config_path)?;

                // Build new named backend config
                let named_cfg = crate::config::NamedBackendConfig {
                    name: resolved_name.clone(),
                    backend_type: backend_type.clone(),
                    default: false,
                    local: if is_local {
                        Some(crate::config::LocalBackendConfig {
                            image: None,
                            data_root: None,
                            storage: None,
                            port_range_start: None,
                            postgres_user: None,
                            postgres_password: None,
                            postgres_db: None,
                        })
                    } else {
                        None
                    },
                    neon: None,
                    dblab: None,
                    xata: None,
                };

                // Store backend in local state instead of committed config
                let mut state = LocalStateManager::new()?;
                state.add_backend(&config_path, named_cfg.clone(), force)?;
                println!("Added backend '{}' to local state", resolved_name);

                // Create main branch for local backends
                if is_local {
                    // Build a config with the backend injected so the factory can find it
                    let mut config_with_backend = config;
                    if let Some(state_backends) = state.get_backends(&config_path) {
                        config_with_backend.backends = Some(state_backends);
                        config_with_backend.backend = None;
                    }

                    // On Linux, offer ZFS auto-setup before creating the main branch
                    #[cfg(feature = "backend-local")]
                    if cfg!(target_os = "linux") {
                        if let Some(data_root) = attempt_zfs_auto_setup(_non_interactive).await {
                            let mut updated_cfg = named_cfg.clone();
                            if let Some(ref mut local) = updated_cfg.local {
                                local.data_root = Some(data_root);
                            }
                            let _ = state.add_backend(&config_path, updated_cfg.clone(), true);
                            if let Some(state_backends) = state.get_backends(&config_path) {
                                config_with_backend.backends = Some(state_backends);
                            }
                            init_local_backend_main(
                                &config_with_backend,
                                &updated_cfg,
                                from.as_deref(),
                            )
                            .await;
                        } else {
                            init_local_backend_main(
                                &config_with_backend,
                                &named_cfg,
                                from.as_deref(),
                            )
                            .await;
                        }
                    } else {
                        init_local_backend_main(&config_with_backend, &named_cfg, from.as_deref())
                            .await;
                    }
                    #[cfg(not(feature = "backend-local"))]
                    {
                        init_local_backend_main(&config_with_backend, &named_cfg, from.as_deref())
                            .await;
                    }
                }
            } else {
                // --- First-time init: create .pgbranch.yml ---
                let mut config = Config::default();

                // Auto-detect main Git branch
                if let Ok(git_repo) = GitRepository::new(".") {
                    if let Ok(Some(detected_main)) = git_repo.detect_main_branch() {
                        config.git.main_branch = detected_main.clone();
                        println!("Auto-detected main Git branch: {}", detected_main);
                    } else {
                        println!("Could not auto-detect main Git branch, using default: main");
                    }
                }

                // For postgres_template backend, look for Docker Compose files
                if is_postgres_template {
                    let compose_files = docker::find_docker_compose_files();
                    if !compose_files.is_empty() {
                        println!("Found Docker Compose files: {}", compose_files.join(", "));

                        if let Some(postgres_config) =
                            docker::parse_postgres_config_from_files(&compose_files)?
                        {
                            if docker::prompt_user_for_config_usage(&postgres_config)? {
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
                                if let Some(database) = postgres_config.database {
                                    config.database.template_database = database;
                                }

                                println!("Using PostgreSQL configuration from Docker Compose");
                            }
                        }
                    }
                }

                // Build named backend config
                let named_cfg = crate::config::NamedBackendConfig {
                    name: resolved_name.clone(),
                    backend_type: backend_type.clone(),
                    default: true,
                    local: if is_local {
                        Some(crate::config::LocalBackendConfig {
                            image: None,
                            data_root: None,
                            storage: None,
                            port_range_start: None,
                            postgres_user: None,
                            postgres_password: None,
                            postgres_db: None,
                        })
                    } else {
                        None
                    },
                    neon: None,
                    dblab: None,
                    xata: None,
                };

                // Don't write backends to committed config — store in state
                config.backends = None;
                config.backend = None;
                config.save_to_file(&config_path)?;
                println!(
                    "Initialized pgbranch configuration at: {}",
                    config_path.display()
                );

                // Store backend in local state
                let mut state = LocalStateManager::new()?;
                state.set_backends(&config_path, vec![named_cfg.clone()])?;
                println!("Stored backend '{}' in local state", resolved_name);

                // Inject backends into config so init_local_backend_main can use them
                config.backends = Some(vec![named_cfg.clone()]);

                // Create main branch for local backends
                if is_local {
                    // On Linux, offer ZFS auto-setup before creating the main branch
                    #[cfg(feature = "backend-local")]
                    if cfg!(target_os = "linux") {
                        if let Some(data_root) = attempt_zfs_auto_setup(_non_interactive).await {
                            // Update the named backend config with the ZFS data_root
                            let mut updated_cfg = named_cfg.clone();
                            if let Some(ref mut local) = updated_cfg.local {
                                local.data_root = Some(data_root);
                            }
                            // Update in state and injected config
                            if let Ok(mut state) = LocalStateManager::new() {
                                let _ = state.set_backends(&config_path, vec![updated_cfg.clone()]);
                            }
                            config.backends = Some(vec![updated_cfg.clone()]);
                            init_local_backend_main(&config, &updated_cfg, from.as_deref()).await;
                        } else {
                            init_local_backend_main(&config, &named_cfg, from.as_deref()).await;
                        }
                    } else {
                        init_local_backend_main(&config, &named_cfg, from.as_deref()).await;
                    }
                    #[cfg(not(feature = "backend-local"))]
                    {
                        init_local_backend_main(&config, &named_cfg, from.as_deref()).await;
                    }
                }

                // Suggest adding local config to gitignore
                let gitignore_path = std::env::current_dir()?.join(".gitignore");
                if gitignore_path.exists() {
                    println!("\nSuggestion: Add '.pgbranch.local.yml' to your .gitignore file:");
                    println!("   echo '.pgbranch.local.yml' >> .gitignore");
                }
            }
        }
        Commands::SetupZfs { pool_name, size } => {
            if !cfg!(target_os = "linux") {
                anyhow::bail!("setup-zfs is only supported on Linux");
            }

            #[cfg(not(feature = "backend-local"))]
            {
                let _ = (pool_name, size);
                anyhow::bail!("Local backend not compiled. Rebuild with --features backend-local");
            }

            #[cfg(feature = "backend-local")]
            {
                use crate::backends::local::storage::zfs_setup::*;

                let pool = pool_name.unwrap_or_else(|| "pgbranch".to_string());
                let img_size = size.unwrap_or_else(|| "10G".to_string());

                let config = ZfsPoolSetupConfig {
                    pool_name: pool.clone(),
                    image_path: PathBuf::from(format!("/var/lib/pgbranch/{}.img", pool)),
                    image_size: img_size.clone(),
                    mountpoint: PathBuf::from("/var/lib/pgbranch/data"),
                };

                println!("Creating file-backed ZFS pool:");
                println!("  Pool name:  {}", config.pool_name);
                println!(
                    "  Image:      {} (sparse, {})",
                    config.image_path.display(),
                    img_size
                );
                println!("  Mountpoint: {}", config.mountpoint.display());
                println!();

                let data_root = create_file_backed_pool(&config).await?;
                println!();
                println!("ZFS pool '{}' created successfully", pool);
                println!("Data root: {}", data_root);
                println!();
                println!("Run 'pgbranch init' to set up a project using this pool.");
            }
        }
        Commands::Config { verbose } => {
            if verbose {
                show_effective_config(&effective_config)?;
            } else {
                println!("Current configuration:");
                println!("{}", serde_yaml_ng::to_string(&config)?);
            }
        }
        Commands::InstallHooks => {
            let git_repo = GitRepository::new(".")?;
            git_repo.install_hooks()?;
            println!("Installed Git hooks");
        }
        Commands::UninstallHooks => {
            let git_repo = GitRepository::new(".")?;
            git_repo.uninstall_hooks()?;
            println!("Uninstalled Git hooks");
        }
        Commands::GitHook {
            worktree,
            main_worktree_dir,
        } => {
            if effective_config.should_skip_hooks() {
                log::debug!("Git hooks are disabled via configuration");
                return Ok(());
            }
            #[cfg(feature = "backend-postgres-template")]
            {
                handle_git_hook(
                    &mut config,
                    &db_manager,
                    &mut local_state,
                    &config_path,
                    worktree,
                    main_worktree_dir,
                )
                .await?;
            }
            #[cfg(not(feature = "backend-postgres-template"))]
            {
                let _ = (worktree, main_worktree_dir);
                anyhow::bail!("Legacy git hook support requires the postgres-template backend. Rebuild with --features backend-postgres-template");
            }
        }
        Commands::WorktreeSetup => {
            #[cfg(feature = "backend-postgres-template")]
            {
                handle_worktree_setup(&mut config, &db_manager, &mut local_state, &config_path)
                    .await?;
            }
            #[cfg(not(feature = "backend-postgres-template"))]
            {
                anyhow::bail!("Legacy worktree support requires the postgres-template backend. Rebuild with --features backend-postgres-template");
            }
        }
        Commands::Switch {
            branch_name,
            template,
            dry_run,
        } => {
            if dry_run {
                if let Some(branch) = branch_name {
                    let normalized_branch = config.get_normalized_branch_name(&branch);
                    println!(
                        "Dry run: would switch to PostgreSQL branch: {}",
                        normalized_branch
                    );
                    if !config.post_commands.is_empty() {
                        println!(
                            "Would execute {} post-command(s)",
                            config.post_commands.len()
                        );
                    }
                } else {
                    println!("Dry run requires a branch name");
                }
            } else {
                #[cfg(feature = "backend-postgres-template")]
                {
                    if template {
                        handle_switch_to_main(
                            &mut config,
                            &db_manager,
                            &mut local_state,
                            &config_path,
                        )
                        .await?;
                    } else if let Some(branch) = branch_name {
                        handle_switch_command(
                            &mut config,
                            &db_manager,
                            &branch,
                            &mut local_state,
                            &config_path,
                        )
                        .await?;
                    } else {
                        handle_interactive_switch(
                            &mut config,
                            &db_manager,
                            &mut local_state,
                            &config_path,
                        )
                        .await?;
                    }
                }
                #[cfg(not(feature = "backend-postgres-template"))]
                {
                    let _ = (template, branch_name);
                    anyhow::bail!("Legacy switch support requires the postgres-template backend. Rebuild with --features backend-postgres-template");
                }
            }
        }
        _ => unreachable!(),
    }

    Ok(())
}

/// Check if ZFS auto-setup should be offered during init (Linux only).
/// Returns `Some(data_root)` if a pool was created or already exists,
/// so the caller can set it on the `LocalBackendConfig`.
#[cfg(feature = "backend-local")]
async fn attempt_zfs_auto_setup(non_interactive: bool) -> Option<String> {
    use crate::backends::local::storage::zfs_setup::*;

    // Use a placeholder path — the actual projects_root hasn't been established yet
    let placeholder = std::path::PathBuf::from("/var/lib/pgbranch/data/projects");
    let status = check_zfs_setup_status(&placeholder).await;

    match status {
        ZfsSetupStatus::NotSupported => None,
        ZfsSetupStatus::ToolsNotInstalled => {
            println!();
            println!("Tip: Install ZFS for near-instant Copy-on-Write database branching:");
            println!("  sudo apt install zfsutils-linux");
            None
        }
        ZfsSetupStatus::AlreadyAvailable { root_dataset } => {
            println!();
            println!(
                "ZFS dataset '{}' detected - will use ZFS for Copy-on-Write storage.",
                root_dataset
            );
            None
        }
        ZfsSetupStatus::PgbranchPoolExists { mountpoint } => {
            println!();
            println!(
                "ZFS pool 'pgbranch' already exists (mountpoint: {}).",
                mountpoint
            );
            Some(mountpoint)
        }
        ZfsSetupStatus::ToolsAvailableNoPool => {
            if non_interactive {
                println!();
                println!(
                    "ZFS tools detected but no pool found. Run 'pgbranch setup-zfs' to create one."
                );
                return None;
            }

            println!();
            println!("ZFS tools detected but no ZFS pool found.");
            println!("pgbranch can create a file-backed ZFS pool for near-instant Copy-on-Write branching.");
            println!();
            println!("This will:");
            println!("  1. Create a 10G sparse image at /var/lib/pgbranch/pgdata.img");
            println!("  2. Create ZFS pool 'pgbranch' with compression=lz4, recordsize=8k");
            println!("  3. Mount at /var/lib/pgbranch/data");
            println!();
            println!("Note: This requires sudo. The 10G image is sparse (starts at ~0 disk usage, grows as needed).");
            println!();

            let confirm = inquire::Confirm::new("Create a file-backed ZFS pool?")
                .with_default(true)
                .prompt();

            match confirm {
                Ok(true) => {
                    let config = ZfsPoolSetupConfig::default();
                    match create_file_backed_pool(&config).await {
                        Ok(data_root) => {
                            println!("ZFS pool 'pgbranch' created successfully");
                            println!();
                            Some(data_root)
                        }
                        Err(e) => {
                            eprintln!("Warning: ZFS pool creation failed: {}", e);
                            eprintln!("Continuing without ZFS (will use copy/reflink fallback).");
                            None
                        }
                    }
                }
                Ok(false) => {
                    println!("Skipping ZFS setup. You can run 'pgbranch setup-zfs' later.");
                    None
                }
                Err(_) => {
                    println!("Skipping ZFS setup.");
                    None
                }
            }
        }
    }
}

async fn init_local_backend_main(
    config: &Config,
    named_cfg: &crate::config::NamedBackendConfig,
    from: Option<&str>,
) {
    match backends::factory::create_backend_from_named_config(config, named_cfg).await {
        Ok(be) => {
            match be.create_branch("main", None).await {
                Ok(info) => {
                    println!("Created main branch");
                    if let Ok(conn) = be.get_connection_info("main").await {
                        if let Some(ref uri) = conn.connection_string {
                            println!("  Connection: {}", uri);
                        }
                    }
                    if let Some(state) = &info.state {
                        println!("  State: {}", state);
                    }

                    // Seed if --from specified
                    if let Some(source) = from {
                        println!("Seeding main branch from: {}", source);
                        match be.seed_from_source("main", source).await {
                            Ok(_) => println!("Seeding completed successfully"),
                            Err(e) => eprintln!("Warning: seeding failed: {}", e),
                        }
                    }
                }
                Err(e) => {
                    eprintln!(
                        "Warning: could not create main branch for '{}': {}",
                        named_cfg.name, e
                    );
                    eprintln!("  You can create it later with: pgbranch create main");
                }
            }
        }
        Err(e) => {
            eprintln!(
                "Warning: could not initialize backend '{}': {}",
                named_cfg.name, e
            );
            eprintln!("  You can create the main branch later with: pgbranch create main");
        }
    }
}

fn print_branch_tree(branches: &[backends::BranchInfo], indent: &str) {
    use std::collections::HashMap;

    if branches.is_empty() {
        println!("{}(none)", indent);
        return;
    }

    // Collect the set of known branch names for parent lookups
    let known: std::collections::HashSet<&str> = branches.iter().map(|b| b.name.as_str()).collect();

    // Group children by parent name
    let mut children: HashMap<&str, Vec<&backends::BranchInfo>> = HashMap::new();
    let mut roots: Vec<&backends::BranchInfo> = Vec::new();

    for b in branches {
        match b.parent_branch.as_deref() {
            Some(parent) if known.contains(parent) => {
                children.entry(parent).or_default().push(b);
            }
            _ => roots.push(b),
        }
    }

    fn print_node(
        branch: &backends::BranchInfo,
        prefix: &str,
        connector: &str,
        children: &std::collections::HashMap<&str, Vec<&backends::BranchInfo>>,
    ) {
        let state_str = branch.state.as_deref().unwrap_or("unknown");
        println!("{}{} [{}]", connector, branch.name, state_str);

        if let Some(kids) = children.get(branch.name.as_str()) {
            let count = kids.len();
            for (i, child) in kids.iter().enumerate() {
                let is_last = i == count - 1;
                let child_connector = if is_last {
                    format!("{}└─ ", prefix)
                } else {
                    format!("{}├─ ", prefix)
                };
                let child_prefix = if is_last {
                    format!("{}   ", prefix)
                } else {
                    format!("{}│  ", prefix)
                };
                print_node(child, &child_prefix, &child_connector, children);
            }
        }
    }

    for root in &roots {
        print_node(root, indent, indent, &children);
    }
}

async fn handle_backend_command(
    cmd: Commands,
    config: &mut Config,
    json_output: bool,
    non_interactive: bool,
    database_name: Option<&str>,
    config_path: &Option<std::path::PathBuf>,
) -> Result<()> {
    // Aggregation commands (List, Status, Doctor) show all backends when no --database given
    let is_aggregation = matches!(cmd, Commands::List | Commands::Status | Commands::Doctor);
    let has_multiple_backends = config.resolve_backends().len() > 1;

    if is_aggregation && database_name.is_none() && has_multiple_backends {
        return handle_multi_backend_command(cmd, config, json_output).await;
    }

    let named = backends::factory::resolve_backend(config, database_name).await?;
    let backend = named.backend;
    let resolved_name = named.name;

    // For mutation commands with multiple backends and no --database, print a note
    if !is_aggregation && database_name.is_none() && has_multiple_backends {
        eprintln!(
            "note: using default database '{}'. Use --database to target a specific one.",
            resolved_name
        );
    }

    match cmd {
        Commands::Create { branch_name, from } => {
            let info = backend.create_branch(&branch_name, from.as_deref()).await?;
            if json_output {
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                println!("Created database branch: {}", info.name);
                if let Some(state) = &info.state {
                    println!("  State: {}", state);
                }
                if let Some(parent) = &info.parent_branch {
                    println!("  Parent: {}", parent);
                }
                // Show connection info
                if let Ok(conn) = backend.get_connection_info(&branch_name).await {
                    if let Some(ref uri) = conn.connection_string {
                        println!("  Connection: {}", uri);
                    }
                }
            }

            // Execute post-commands
            if !config.post_commands.is_empty() {
                let executor = PostCommandExecutor::new(config, &branch_name)?;
                executor.execute_all_post_commands().await?;
            }
        }
        Commands::Delete { branch_name } => {
            backend.delete_branch(&branch_name).await?;
            if json_output {
                println!("{{\"status\":\"ok\",\"deleted\":\"{}\"}}", branch_name);
            } else {
                println!("Deleted database branch: {}", branch_name);
            }
        }
        Commands::List => {
            let branches = backend.list_branches().await?;
            if json_output {
                println!("{}", serde_json::to_string_pretty(&branches)?);
            } else {
                println!("Database branches ({}):", backend.backend_name());
                print_branch_tree(&branches, "  ");
            }
        }
        Commands::Start { branch_name } => {
            if !backend.supports_lifecycle() {
                anyhow::bail!(
                    "Backend '{}' does not support start/stop lifecycle",
                    backend.backend_name()
                );
            }
            backend.start_branch(&branch_name).await?;
            if json_output {
                println!("{{\"status\":\"ok\",\"started\":\"{}\"}}", branch_name);
            } else {
                println!("Started branch: {}", branch_name);
            }
        }
        Commands::Stop { branch_name } => {
            if !backend.supports_lifecycle() {
                anyhow::bail!(
                    "Backend '{}' does not support start/stop lifecycle",
                    backend.backend_name()
                );
            }
            backend.stop_branch(&branch_name).await?;
            if json_output {
                println!("{{\"status\":\"ok\",\"stopped\":\"{}\"}}", branch_name);
            } else {
                println!("Stopped branch: {}", branch_name);
            }
        }
        Commands::Reset { branch_name } => {
            if !backend.supports_lifecycle() {
                anyhow::bail!(
                    "Backend '{}' does not support reset",
                    backend.backend_name()
                );
            }
            backend.reset_branch(&branch_name).await?;
            if json_output {
                println!("{{\"status\":\"ok\",\"reset\":\"{}\"}}", branch_name);
            } else {
                println!("Reset branch: {}", branch_name);
            }
        }
        Commands::Doctor => {
            let report = backend.doctor().await?;
            if json_output {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Doctor report ({}):", backend.backend_name());
                for check in &report.checks {
                    let icon = if check.available { "OK" } else { "FAIL" };
                    println!("  [{}] {}: {}", icon, check.name, check.detail);
                }
            }
        }
        Commands::Connection {
            branch_name,
            format,
        } => {
            let conn = backend.get_connection_info(&branch_name).await?;
            let fmt = format.as_deref().unwrap_or("uri");
            match fmt {
                "uri" => {
                    if let Some(ref uri) = conn.connection_string {
                        println!("{}", uri);
                    } else {
                        println!(
                            "postgresql://{}@{}:{}/{}",
                            conn.user, conn.host, conn.port, conn.database
                        );
                    }
                }
                "env" => {
                    println!("DATABASE_HOST={}", conn.host);
                    println!("DATABASE_PORT={}", conn.port);
                    println!("DATABASE_NAME={}", conn.database);
                    println!("DATABASE_USER={}", conn.user);
                    if let Some(ref password) = conn.password {
                        println!("DATABASE_PASSWORD={}", password);
                    }
                    if let Some(ref uri) = conn.connection_string {
                        println!("DATABASE_URL={}", uri);
                    }
                }
                _ => {
                    println!("{}", serde_json::to_string_pretty(&conn)?);
                }
            }
        }
        Commands::Status => {
            let branches = backend.list_branches().await.unwrap_or_default();
            let running = branches
                .iter()
                .filter(|b| b.state.as_deref() == Some("running"))
                .count();
            let stopped = branches
                .iter()
                .filter(|b| b.state.as_deref() == Some("stopped"))
                .count();
            let project_info = backend.project_info();

            if json_output {
                let mut status = serde_json::json!({
                    "backend": backend.backend_name(),
                    "total_branches": branches.len(),
                    "running": running,
                    "stopped": stopped,
                    "supports_lifecycle": backend.supports_lifecycle(),
                });
                if let Some(ref info) = project_info {
                    status["project"] = serde_json::Value::String(info.name.clone());
                    if let Some(ref storage) = info.storage_backend {
                        status["storage"] = serde_json::Value::String(storage.clone());
                    }
                    if let Some(ref image) = info.image {
                        status["image"] = serde_json::Value::String(image.clone());
                    }
                }
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!("Backend: {}", backend.backend_name());
                if let Some(ref info) = project_info {
                    println!("Project: {}", info.name);
                    if let Some(ref storage) = info.storage_backend {
                        println!("Storage: {}", storage);
                    }
                    if let Some(ref image) = info.image {
                        println!("Image: {}", image);
                    }
                }
                println!(
                    "Branches: {} total ({} running, {} stopped)",
                    branches.len(),
                    running,
                    stopped
                );
                if backend.supports_lifecycle() {
                    println!("Lifecycle: supported (start/stop/reset)");
                }
            }
        }
        Commands::Cleanup { max_count } => {
            let max = max_count.unwrap_or(config.behavior.max_branches.unwrap_or(10));
            let deleted = backend.cleanup_old_branches(max).await?;
            if json_output {
                println!("{}", serde_json::to_string_pretty(&deleted)?);
            } else if deleted.is_empty() {
                println!("No branches to clean up");
            } else {
                println!(
                    "Cleaned up {} branches: {}",
                    deleted.len(),
                    deleted.join(", ")
                );
            }
        }
        Commands::Destroy { force } => {
            if !backend.supports_destroy() {
                anyhow::bail!(
                    "Backend '{}' does not support destroy. This command is only available for the local (Docker + CoW) backend.",
                    backend.backend_name()
                );
            }

            let preview = backend.destroy_preview().await?;
            let (project_name, branch_names) = match preview {
                Some(p) => p,
                None => {
                    if json_output {
                        println!("{{\"status\":\"ok\",\"message\":\"no project found\"}}");
                    } else {
                        println!(
                            "No project found for database '{}'. Nothing to destroy.",
                            resolved_name
                        );
                    }
                    return Ok(());
                }
            };

            if !force && !non_interactive {
                println!("This will permanently destroy the following:");
                println!("  Project: {}", project_name);
                if branch_names.is_empty() {
                    println!("  Branches: (none)");
                } else {
                    println!("  Branches ({}):", branch_names.len());
                    for name in &branch_names {
                        println!("    - {}", name);
                    }
                }
                println!();
                println!("All containers, storage data, and state will be removed.");

                let confirm =
                    inquire::Confirm::new("Are you sure you want to destroy this project?")
                        .with_default(false)
                        .prompt()?;

                if !confirm {
                    println!("Aborted.");
                    return Ok(());
                }
            }

            let destroyed = backend.destroy_project().await?;

            // Remove the backend entry from local state
            if let Some(ref path) = config_path {
                if let Ok(mut state) = LocalStateManager::new() {
                    let _ = state.remove_backend(path, &resolved_name);
                }
            }

            // Also remove from committed config for backward compat (legacy configs)
            config.remove_backend(&resolved_name);
            if let Some(path) = config_path {
                config.save_to_file(path)?;
            }

            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": "ok",
                        "project": project_name,
                        "destroyed_branches": destroyed,
                    }))?
                );
            } else {
                println!(
                    "Destroyed project '{}' and {} branch(es)",
                    project_name,
                    destroyed.len()
                );
                for name in &destroyed {
                    println!("  - {}", name);
                }
            }
        }
        _ => unreachable!(),
    }

    Ok(())
}

/// Handle aggregation commands (List, Status, Doctor) across all backends.
async fn handle_multi_backend_command(
    cmd: Commands,
    config: &Config,
    json_output: bool,
) -> Result<()> {
    let all_backends = backends::factory::create_all_backends(config).await?;

    match cmd {
        Commands::List => {
            if json_output {
                let mut map = serde_json::Map::new();
                for named in &all_backends {
                    let branches = named.backend.list_branches().await.unwrap_or_default();
                    map.insert(named.name.clone(), serde_json::to_value(&branches)?);
                }
                println!("{}", serde_json::to_string_pretty(&map)?);
            } else {
                for named in &all_backends {
                    let branches = named.backend.list_branches().await.unwrap_or_default();
                    println!("[{}] ({}):", named.name, named.backend.backend_name());
                    print_branch_tree(&branches, "  ");
                    println!();
                }
            }
        }
        Commands::Status => {
            if json_output {
                let mut map = serde_json::Map::new();
                for named in &all_backends {
                    let branches = named.backend.list_branches().await.unwrap_or_default();
                    let running = branches
                        .iter()
                        .filter(|b| b.state.as_deref() == Some("running"))
                        .count();
                    let stopped = branches
                        .iter()
                        .filter(|b| b.state.as_deref() == Some("stopped"))
                        .count();
                    let project_info = named.backend.project_info();

                    let mut status = serde_json::json!({
                        "backend": named.backend.backend_name(),
                        "total_branches": branches.len(),
                        "running": running,
                        "stopped": stopped,
                        "supports_lifecycle": named.backend.supports_lifecycle(),
                    });
                    if let Some(ref info) = project_info {
                        status["project"] = serde_json::Value::String(info.name.clone());
                        if let Some(ref storage) = info.storage_backend {
                            status["storage"] = serde_json::Value::String(storage.clone());
                        }
                        if let Some(ref image) = info.image {
                            status["image"] = serde_json::Value::String(image.clone());
                        }
                    }
                    map.insert(named.name.clone(), status);
                }
                println!("{}", serde_json::to_string_pretty(&map)?);
            } else {
                for named in &all_backends {
                    let branches = named.backend.list_branches().await.unwrap_or_default();
                    let running = branches
                        .iter()
                        .filter(|b| b.state.as_deref() == Some("running"))
                        .count();
                    let stopped = branches
                        .iter()
                        .filter(|b| b.state.as_deref() == Some("stopped"))
                        .count();
                    let project_info = named.backend.project_info();

                    println!("[{}] ({}):", named.name, named.backend.backend_name());
                    if let Some(ref info) = project_info {
                        println!("  Project: {}", info.name);
                        if let Some(ref storage) = info.storage_backend {
                            println!("  Storage: {}", storage);
                        }
                        if let Some(ref image) = info.image {
                            println!("  Image: {}", image);
                        }
                    }
                    println!(
                        "  Branches: {} total ({} running, {} stopped)",
                        branches.len(),
                        running,
                        stopped
                    );
                    if named.backend.supports_lifecycle() {
                        println!("  Lifecycle: supported (start/stop/reset)");
                    }
                    println!();
                }
            }
        }
        Commands::Doctor => {
            if json_output {
                let mut map = serde_json::Map::new();
                for named in &all_backends {
                    let report = named.backend.doctor().await?;
                    map.insert(named.name.clone(), serde_json::to_value(&report)?);
                }
                println!("{}", serde_json::to_string_pretty(&map)?);
            } else {
                for named in &all_backends {
                    let report = named.backend.doctor().await?;
                    println!(
                        "[{}] Doctor report ({}):",
                        named.name,
                        named.backend.backend_name()
                    );
                    for check in &report.checks {
                        let icon = if check.available { "OK" } else { "FAIL" };
                        println!("  [{}] {}: {}", icon, check.name, check.detail);
                    }
                    println!();
                }
            }
        }
        _ => unreachable!(),
    }

    Ok(())
}

/// Run configuration and environment checks as part of `doctor`.
fn run_doctor_pre_checks(config: &Config, config_path: &Option<std::path::PathBuf>) {
    println!("General:");

    // Config file
    match config_path {
        Some(path) => println!("  [OK] Config file: {}", path.display()),
        None => println!("  [WARN] Config file: not found (using defaults)"),
    }

    // Git repository
    match GitRepository::new(".") {
        Ok(_) => println!("  [OK] Git repository: detected"),
        Err(_) => println!("  [FAIL] Git repository: not found"),
    }

    // Git hooks
    let hooks_dir = std::path::Path::new(".git/hooks");
    let has_hooks = if hooks_dir.exists() {
        let post_checkout = hooks_dir.join("post-checkout");
        let post_merge = hooks_dir.join("post-merge");
        if let Ok(git_repo) = GitRepository::new(".") {
            (post_checkout.exists() && git_repo.is_pgbranch_hook(&post_checkout).unwrap_or(false))
                || (post_merge.exists() && git_repo.is_pgbranch_hook(&post_merge).unwrap_or(false))
        } else {
            post_checkout.exists() || post_merge.exists()
        }
    } else {
        false
    };
    if has_hooks {
        println!("  [OK] Git hooks: installed");
    } else {
        println!("  [WARN] Git hooks: not installed (run 'pgbranch install-hooks')");
    }

    // Branch filter regex
    if let Some(ref regex_pattern) = config.git.branch_filter_regex {
        match regex::Regex::new(regex_pattern) {
            Ok(_) => println!("  [OK] Branch filter regex: valid"),
            Err(e) => println!("  [FAIL] Branch filter regex: {}", e),
        }
    }

    println!();
}

fn copy_worktree_files(config: &Config, main_worktree_dir: &str) -> Result<()> {
    let copy_files = match config.worktree {
        Some(ref wt) => &wt.copy_files,
        None => return Ok(()),
    };

    let main_dir = std::path::Path::new(main_worktree_dir);
    let current_dir = std::env::current_dir()?;

    for file in copy_files {
        let source = main_dir.join(file);
        let target = current_dir.join(file);

        if source.exists() && !target.exists() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&source, &target)?;
            println!("Copied {} from main worktree", file);
        }
    }
    Ok(())
}

#[cfg(feature = "backend-postgres-template")]
async fn handle_worktree_setup(
    config: &mut Config,
    db_manager: &DatabaseManager,
    local_state: &mut Option<LocalStateManager>,
    config_path: &Option<std::path::PathBuf>,
) -> Result<()> {
    let git_repo = GitRepository::new(".")?;

    if !git_repo.is_worktree() {
        anyhow::bail!(
            "Not inside a Git worktree. Use this command from within a worktree directory."
        );
    }

    let main_dir = git_repo
        .get_main_worktree_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine main worktree directory"))?;

    // Copy files from main worktree
    copy_worktree_files(config, main_dir.to_str().unwrap_or(""))?;

    // Run normal git-hook logic to create/switch DB branch
    handle_git_hook(config, db_manager, local_state, config_path, false, None).await?;

    Ok(())
}

#[cfg(feature = "backend-postgres-template")]
async fn handle_git_hook(
    config: &mut Config,
    db_manager: &DatabaseManager,
    local_state: &mut Option<LocalStateManager>,
    config_path: &Option<std::path::PathBuf>,
    worktree: bool,
    main_worktree_dir: Option<String>,
) -> Result<()> {
    // If called from a worktree, copy files first
    if worktree {
        if let Some(ref main_dir) = main_worktree_dir {
            copy_worktree_files(config, main_dir)?;
        }
    }

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
                    handle_switch_command(
                        config,
                        db_manager,
                        &current_git_branch,
                        local_state,
                        config_path,
                    )
                    .await?;
                } else {
                    log::info!(
                        "Git branch {} configured not to create PostgreSQL branch",
                        current_git_branch
                    );
                }
            }
        } else {
            log::info!(
                "Git branch {} filtered out by auto_switch configuration",
                current_git_branch
            );
        }
    }

    Ok(())
}

#[cfg(feature = "backend-postgres-template")]
async fn handle_interactive_switch(
    config: &mut Config,
    db_manager: &DatabaseManager,
    local_state: &mut Option<LocalStateManager>,
    config_path: &Option<std::path::PathBuf>,
) -> Result<()> {
    // Get available branches
    let mut branches = match db_manager.list_database_branches().await {
        Ok(branches) => branches,
        Err(_) => {
            // If database connection fails, show current branch from local state or smart default (if not main)
            let mut fallback_branches = Vec::new();
            if let Some(current) = get_current_branch_with_default(local_state, config_path, config)
            {
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
    let branch_items: Vec<BranchItem> = branches
        .iter()
        .map(|branch| {
            let current_branch = get_current_branch_with_default(local_state, config_path, config);
            let is_current = match current_branch {
                Some(current) => {
                    if current == "_main" && branch == "main" {
                        true
                    } else {
                        current == *branch
                    }
                }
                None => false,
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
        })
        .collect();

    // Run interactive selector
    match run_interactive_selector(branch_items) {
        Ok(selected_branch) => {
            if selected_branch == "main" {
                handle_switch_to_main(config, db_manager, local_state, config_path).await?;
            } else {
                handle_switch_command(
                    config,
                    db_manager,
                    &selected_branch,
                    local_state,
                    config_path,
                )
                .await?;
            }
        }
        Err(e) => match e {
            inquire::InquireError::OperationCanceled => {
                println!("Cancelled.");
            }
            inquire::InquireError::OperationInterrupted => {
                println!("Interrupted.");
            }
            _ => {
                println!("⚠️  Interactive mode failed: {}", e);
                println!(
                    "💡 Try using: pgbranch switch <branch-name> or pgbranch switch --template"
                );
            }
        },
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
        return Err(inquire::InquireError::InvalidConfiguration(
            "No branches available".to_string(),
        ));
    }

    // Create display options with current branch marker
    let options: Vec<String> = items
        .iter()
        .map(|item| {
            if item.is_current {
                format!("{} ★", item.display_name)
            } else {
                item.display_name.clone()
            }
        })
        .collect();

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
    let selected_index = options
        .iter()
        .position(|opt| opt == &selected_display)
        .ok_or_else(|| {
            inquire::InquireError::InvalidConfiguration("Selected option not found".to_string())
        })?;

    Ok(items[selected_index].name.clone())
}

#[cfg(feature = "backend-postgres-template")]
async fn handle_switch_command(
    config: &mut Config,
    db_manager: &DatabaseManager,
    branch_name: &str,
    local_state: &mut Option<LocalStateManager>,
    config_path: &Option<std::path::PathBuf>,
) -> Result<()> {
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
                        println!(
                            "💡 Branch state updated in config, but database operation failed"
                        );
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

#[cfg(feature = "backend-postgres-template")]
async fn handle_switch_to_main(
    config: &mut Config,
    _db_manager: &DatabaseManager,
    local_state: &mut Option<LocalStateManager>,
    config_path: &Option<std::path::PathBuf>,
) -> Result<()> {
    let main_name = "_main";

    println!("🔄 Switching to main database");

    // Update current branch in local state to a special main marker
    set_current_branch(local_state, config_path, Some(main_name.to_string()))?;

    println!(
        "✅ Switched to main database: {}",
        config.database.template_database
    );

    // Execute post-commands with main branch
    if !config.post_commands.is_empty() {
        println!("🔧 Executing post-commands for main switch...");
        let executor = PostCommandExecutor::new(config, main_name)?;
        executor.execute_all_post_commands().await?;
    }

    Ok(())
}

// Helper functions for current branch management with local state
fn get_current_branch(
    local_state: &Option<LocalStateManager>,
    config_path: &Option<std::path::PathBuf>,
) -> Option<String> {
    if let (Some(state_manager), Some(path)) = (local_state, config_path) {
        state_manager.get_current_branch(path)
    } else {
        None
    }
}

fn get_current_branch_with_default(
    local_state: &Option<LocalStateManager>,
    config_path: &Option<std::path::PathBuf>,
    config: &Config,
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
                log::debug!(
                    "Detecting default current branch from Git branch: {}",
                    current_git_branch
                );

                // If on main Git branch, default to main database
                if current_git_branch == config.git.main_branch {
                    log::debug!("On main Git branch, defaulting to main database");
                    return Some("_main".to_string());
                }

                // If current Git branch would create a database branch, default to that
                if config.should_create_branch(&current_git_branch) {
                    let normalized_branch = config.get_normalized_branch_name(&current_git_branch);
                    log::debug!(
                        "Git branch matches create filter, defaulting to: {}",
                        normalized_branch
                    );
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

fn set_current_branch(
    local_state: &mut Option<LocalStateManager>,
    config_path: &Option<std::path::PathBuf>,
    branch: Option<String>,
) -> Result<()> {
    if let (Some(state_manager), Some(path)) = (local_state, config_path) {
        state_manager.set_current_branch(path, branch)?;
    }
    Ok(())
}

fn show_effective_config(effective_config: &EffectiveConfig) -> Result<()> {
    println!("🔧 Effective Configuration");
    println!("==========================\n");

    // Show configuration status
    println!("📊 Status:");
    if effective_config.is_disabled() {
        println!("  ❌ pgbranch is DISABLED globally");
    } else {
        println!("  ✅ pgbranch is enabled");
    }

    if effective_config.should_skip_hooks() {
        println!("  ❌ Git hooks are DISABLED");
    } else {
        println!("  ✅ Git hooks are enabled");
    }

    if effective_config.is_current_branch_disabled() {
        println!("  ❌ Current branch operations are DISABLED");
    } else {
        println!("  ✅ Current branch operations are enabled");
    }

    // Check if current git branch is disabled
    match effective_config.check_current_git_branch_disabled() {
        Ok(true) => println!("  ❌ Current Git branch is DISABLED"),
        Ok(false) => {
            if let Ok(git_repo) = crate::git::GitRepository::new(".") {
                if let Ok(Some(branch)) = git_repo.get_current_branch() {
                    println!("  ✅ Current Git branch '{}' is enabled", branch);
                } else {
                    println!("  ⚠️  Could not determine current Git branch");
                }
            } else {
                println!("  ⚠️  Not in a Git repository");
            }
        }
        Err(e) => println!("  ⚠️  Error checking current branch: {}", e),
    }

    println!();

    // Show environment variable overrides
    println!("🌍 Environment Variable Overrides:");
    let has_env_overrides = effective_config.env_config.disabled.is_some()
        || effective_config.env_config.skip_hooks.is_some()
        || effective_config.env_config.auto_create.is_some()
        || effective_config.env_config.auto_switch.is_some()
        || effective_config.env_config.branch_filter_regex.is_some()
        || effective_config.env_config.disabled_branches.is_some()
        || effective_config
            .env_config
            .current_branch_disabled
            .is_some()
        || effective_config.env_config.database_host.is_some()
        || effective_config.env_config.database_port.is_some()
        || effective_config.env_config.database_user.is_some()
        || effective_config.env_config.database_password.is_some()
        || effective_config.env_config.database_prefix.is_some();

    if !has_env_overrides {
        println!("  (none)");
    } else {
        if let Some(disabled) = effective_config.env_config.disabled {
            println!("  PGBRANCH_DISABLED: {}", disabled);
        }
        if let Some(skip_hooks) = effective_config.env_config.skip_hooks {
            println!("  PGBRANCH_SKIP_HOOKS: {}", skip_hooks);
        }
        if let Some(auto_create) = effective_config.env_config.auto_create {
            println!("  PGBRANCH_AUTO_CREATE: {}", auto_create);
        }
        if let Some(auto_switch) = effective_config.env_config.auto_switch {
            println!("  PGBRANCH_AUTO_SWITCH: {}", auto_switch);
        }
        if let Some(ref regex) = effective_config.env_config.branch_filter_regex {
            println!("  PGBRANCH_BRANCH_FILTER_REGEX: {}", regex);
        }
        if let Some(ref branches) = effective_config.env_config.disabled_branches {
            println!("  PGBRANCH_DISABLED_BRANCHES: {}", branches.join(","));
        }
        if let Some(current_disabled) = effective_config.env_config.current_branch_disabled {
            println!("  PGBRANCH_CURRENT_BRANCH_DISABLED: {}", current_disabled);
        }
        if let Some(ref host) = effective_config.env_config.database_host {
            println!("  PGBRANCH_DATABASE_HOST: {}", host);
        }
        if let Some(port) = effective_config.env_config.database_port {
            println!("  PGBRANCH_DATABASE_PORT: {}", port);
        }
        if let Some(ref user) = effective_config.env_config.database_user {
            println!("  PGBRANCH_DATABASE_USER: {}", user);
        }
        if effective_config.env_config.database_password.is_some() {
            println!("  PGBRANCH_DATABASE_PASSWORD: [hidden]");
        }
        if let Some(ref prefix) = effective_config.env_config.database_prefix {
            println!("  PGBRANCH_DATABASE_PREFIX: {}", prefix);
        }
    }

    println!();

    // Show local config overrides
    println!("📁 Local Config File Overrides:");
    if let Some(ref local_config) = effective_config.local_config {
        println!("  ✅ Local config file found (.pgbranch.local.yml)");
        if local_config.disabled.is_some()
            || local_config.disabled_branches.is_some()
            || local_config.database.is_some()
            || local_config.git.is_some()
            || local_config.behavior.is_some()
            || local_config.post_commands.is_some()
        {
            println!("  Local overrides present (see merged config below)");
        } else {
            println!("  No overrides in local config");
        }
    } else {
        println!("  (no local config file found)");
    }

    println!();

    // Show backend source
    println!("Backends:");
    if let Ok(state) = LocalStateManager::new() {
        // Try to find config path to look up state backends
        let config_path = Config::find_config_file().ok().flatten();
        let state_backends = config_path.as_ref().and_then(|p| state.get_backends(p));

        if let Some(ref backends) = state_backends {
            println!("  Source: local state (~/.config/pgbranch/local_state.yml)");
            for b in backends {
                let default_marker = if b.default { " (default)" } else { "" };
                println!("  - {} [{}]{}", b.name, b.backend_type, default_marker);
            }
        } else {
            let committed_backends = effective_config.config.resolve_backends();
            if committed_backends.is_empty() {
                println!("  (none configured)");
            } else {
                println!("  Source: committed config (.pgbranch.yml)");
                for b in &committed_backends {
                    let default_marker = if b.default { " (default)" } else { "" };
                    println!("  - {} [{}]{}", b.name, b.backend_type, default_marker);
                }
            }
        }
    }

    println!();

    // Show final merged configuration
    println!("Final Merged Configuration:");
    let merged_config = effective_config.get_merged_config();
    println!("{}", serde_yaml_ng::to_string(&merged_config)?);

    Ok(())
}
