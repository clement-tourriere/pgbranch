# pgbranch

Database branching for PostgreSQL that syncs with Git.

pgbranch gives every Git branch its own PostgreSQL database. It works in two modes: **local mode** spins up Docker containers with Copy-on-Write storage for zero-setup isolated environments, while **template mode** uses PostgreSQL's TEMPLATE feature to branch databases on an existing server. Both modes integrate with Git hooks so database branches are created and switched automatically.

## Features

- **Local Docker branching** with Copy-on-Write storage (APFS clones on macOS, ZFS snapshots, reflinks on Btrfs/XFS)
- **PostgreSQL TEMPLATE branching** for existing servers — no Docker needed
- **Automatic Git integration** via post-checkout and post-merge hooks
- **Post-commands** with template variables for automatic env/config updates after branch switches
- **Multi-backend support** — local, postgres_template, Neon, DBLab, Xata
- **Seed databases** from a PostgreSQL server, local dump files, or S3
- **JSON output + non-interactive mode** for CI/CD pipelines and AI agent workflows
- **Interactive branch switching** with fuzzy search
- **Branch lifecycle management** — start, stop, reset, destroy (local backend)
- **Diagnostics** with `doctor` command and connection info in multiple formats

## Installation

### From Source

```bash
git clone https://github.com/clement-tourriere/pgbranch.git
cd pgbranch

# Install directly with cargo (recommended)
cargo install --path .

# Or build manually and copy to PATH
cargo build --release
# Copy target/release/pgbranch to your PATH
```

## Quick Start

### Local Mode (Docker + CoW)

No existing PostgreSQL server needed — pgbranch manages Docker containers for you.

```bash
# Initialize with local backend (default)
pgbranch init myapp

# Optionally seed from an existing database
pgbranch init myapp --from postgresql://user:pass@localhost:5432/mydb

# Install Git hooks for automatic branching
pgbranch install-hooks

# Create a branch manually
pgbranch create feature-auth

# Check status
pgbranch status
```

### Template Mode (Existing PostgreSQL Server)

Use an existing PostgreSQL server — branches are created using `CREATE DATABASE ... WITH TEMPLATE`.

```bash
# Initialize with postgres_template backend
pgbranch init myapp --backend postgres_template

# Install Git hooks
pgbranch install-hooks
```

## How It Works

### Local Backend

Each database branch runs in its own Docker container with PostgreSQL data stored on the host filesystem.

1. **Init** creates a "main" branch: pulls the PostgreSQL Docker image, starts a container, and bind-mounts a data directory
2. **Branch creation** pauses the parent container, uses Copy-on-Write to clone the data directory, then starts a new container pointing at the clone
3. **Storage efficiency** depends on the filesystem:
   - **APFS** (macOS): `cp -c` clones — near-zero disk overhead, instant copy
   - **ZFS** (Linux): snapshots and clones — near-zero overhead
   - **Btrfs/XFS** (Linux): reflink copies — near-zero overhead
   - **Other**: full recursive copy (fallback)
4. **Lifecycle** — containers can be stopped, started, and reset independently
5. **Destroy** removes all containers and data for a project

### Template Backend

Uses PostgreSQL's built-in `CREATE DATABASE ... WITH TEMPLATE` for server-side copies. Fast, no Docker required, but branches share the same PostgreSQL instance and the template database must have no active connections during branching.

### Cloud Backends

Neon, DBLab, and Xata backends use their respective APIs to manage branches remotely. Configure them with API keys in your backend config.

## CLI Reference

### Branch Management

```bash
pgbranch create <branch>            # Create a database branch
pgbranch create <branch> --from <parent>  # Create from a specific parent branch
pgbranch delete <branch>            # Delete a database branch
pgbranch list                       # List all branches (tree view)
pgbranch switch                     # Interactive switch with fuzzy search
pgbranch switch <branch>            # Switch to a branch (creates if needed)
pgbranch switch --template          # Switch to main/template database
pgbranch cleanup --max-count 5      # Remove old branches, keep most recent N
```

### Lifecycle (Local Backend)

```bash
pgbranch start <branch>             # Start a stopped container
pgbranch stop <branch>              # Stop a running container
pgbranch reset <branch>             # Reset branch to its parent state
pgbranch destroy                    # Remove all containers and data for the project
pgbranch destroy --force            # Skip confirmation prompt
```

### Setup & Hooks

```bash
pgbranch init [name]                # Initialize configuration
pgbranch init [name] --backend <type>  # Specify backend: local, postgres_template, neon, dblab, xata
pgbranch init [name] --from <source>   # Seed main branch (PostgreSQL URL, file, or s3:// URL)
pgbranch install-hooks              # Install Git post-checkout/post-merge hooks
pgbranch uninstall-hooks            # Remove Git hooks
pgbranch setup-zfs                  # Create a file-backed ZFS pool (Linux)
pgbranch setup-zfs --size 20G       # Custom pool size
pgbranch setup-zfs --pool-name mypool  # Custom pool name
```

### Info & Diagnostics

```bash
pgbranch status                     # Show project and backend status
pgbranch config                     # Show current configuration
pgbranch config -v                  # Show effective config with precedence details
pgbranch doctor                     # Run diagnostics (config, git, backend health)
pgbranch connection <branch>        # Connection URI (default)
pgbranch connection <branch> --format env   # Environment variables
pgbranch connection <branch> --format json  # JSON object
```

### Global Flags

```bash
pgbranch --json <command>           # JSON output for all commands
pgbranch --non-interactive <command>  # Skip prompts, use defaults
pgbranch -d <name> <command>        # Target a specific named database (multi-backend)
```

## Configuration

### `.pgbranch.yml`

The configuration file is created by `pgbranch init` and supports these sections:

#### Git Configuration

```yaml
git:
  auto_create_on_branch: true       # Auto-create database branch on git checkout
  auto_switch_on_branch: true       # Auto-switch database on git checkout
  main_branch: main                 # Main git branch (auto-detected on init)
  auto_create_branch_filter: "^feature/.*"  # Only branch for matching patterns
  exclude_branches:                 # Never create branches for these
    - main
    - master
    - develop
```

#### Behavior Configuration

```yaml
behavior:
  auto_cleanup: false               # Auto-cleanup old branches
  max_branches: 10                  # Max branches to keep
  naming_strategy: prefix           # prefix, suffix, or replace
```

#### Local Backend Configuration

The local backend is configured per-database via `pgbranch init`. Settings are stored in local state (`~/.config/pgbranch/local_state.yml`), not in the committed config file. Available options:

- `image` — Docker image (default: `postgres:17`)
- `data_root` — Root directory for data storage
- `port_range_start` — Starting port for containers (default: `55432`)
- `postgres_user`, `postgres_password`, `postgres_db` — PostgreSQL credentials

### Post-Commands

Post-commands run automatically after branch creation and switching, updating your application configuration to point to the new database.

#### Simple Commands

```yaml
post_commands:
  - "echo 'Database ready for {branch_name}!'"
  - "npm run migrate"
```

#### Complex Commands

```yaml
post_commands:
  - name: "Run Django migrations"
    command: "python manage.py migrate"
    working_dir: "./backend"
    condition: "file_exists:manage.py"
    continue_on_error: false
    environment:
      DATABASE_URL: "postgresql://{db_user}@{db_host}:{db_port}/{db_name}"
```

#### Replace Actions

```yaml
post_commands:
  - action: "replace"
    name: "Update database configuration"
    file: ".env.local"
    pattern: "DATABASE_URL=.*"
    replacement: "DATABASE_URL=postgresql://{db_user}@{db_host}:{db_port}/{db_name}"
    create_if_missing: true
    condition: "file_exists:manage.py"
```

#### Template Variables

| Variable | Description |
|---|---|
| `{branch_name}` | Current Git branch name |
| `{db_name}` | Generated database name (with prefix/suffix) |
| `{db_host}` | Database host |
| `{db_port}` | Database port |
| `{db_user}` | Database username |
| `{db_password}` | Database password (if configured) |
| `{template_db}` | Template database name |
| `{prefix}` | Database prefix |

### Local Configuration Overrides

pgbranch supports a three-level configuration hierarchy (highest to lowest precedence):

1. **Environment variables** — quick toggles
2. **`.pgbranch.local.yml`** — project-specific local overrides (add to `.gitignore`)
3. **`.pgbranch.yml`** — team shared configuration

#### Environment Variables

```bash
PGBRANCH_DISABLED=true              # Completely disable pgbranch
PGBRANCH_SKIP_HOOKS=true            # Skip Git hook execution
PGBRANCH_AUTO_CREATE=false          # Override auto_create_on_branch
PGBRANCH_AUTO_SWITCH=false          # Override auto_switch_on_branch
PGBRANCH_BRANCH_FILTER_REGEX=...    # Override branch filtering
PGBRANCH_DISABLED_BRANCHES=main,release/*  # Disable for specific branches
PGBRANCH_CURRENT_BRANCH_DISABLED=true      # Disable for current branch only
PGBRANCH_DATABASE_HOST=...          # Override database host
PGBRANCH_DATABASE_PORT=...          # Override database port
PGBRANCH_DATABASE_USER=...          # Override database user
PGBRANCH_DATABASE_PASSWORD=...      # Override database password
PGBRANCH_DATABASE_PREFIX=...        # Override database prefix
```

#### Local Config File

```yaml
# .pgbranch.local.yml (add to .gitignore)
disabled: false
disabled_branches:
  - "feature/*"
  - hotfix
git:
  auto_switch_on_branch: false
  main_branch: develop
```

## Examples

### Django Integration

```yaml
git:
  auto_create_on_branch: true
  auto_switch_on_branch: true
  main_branch: main
  exclude_branches:
    - main
    - master
    - develop

post_commands:
  - action: "replace"
    name: "Update Django database configuration"
    file: ".env.local"
    pattern: "DATABASE_URL=.*"
    replacement: "DATABASE_URL=postgresql://{db_user}@{db_host}:{db_port}/{db_name}"
    create_if_missing: true
    condition: "file_exists:manage.py"

  - name: "Run Django migrations"
    command: "python manage.py migrate"
    condition: "file_exists:manage.py"
    continue_on_error: false
    environment:
      DATABASE_URL: "postgresql://{db_user}@{db_host}:{db_port}/{db_name}"

  - name: "Restart Docker services"
    command: "docker compose restart"
    continue_on_error: true
```

### Node.js / Express

```yaml
git:
  auto_create_on_branch: true
  auto_switch_on_branch: true
  main_branch: main
  exclude_branches:
    - main
    - master

post_commands:
  - action: "replace"
    name: "Update environment configuration"
    file: ".env"
    pattern: "DB_NAME=.*"
    replacement: "DB_NAME={db_name}"
    create_if_missing: true
    condition: "file_exists:package.json"

  - name: "Run database migrations"
    command: "npm run migrate"
    condition: "file_exists:package.json"
    continue_on_error: false
```

### Local Docker Backend with Seeding

```bash
# Initialize and seed from production dump
pgbranch init myapp --from /path/to/prod-dump.sql

# Or seed from a running database
pgbranch init myapp --from postgresql://readonly:pass@prod-replica:5432/mydb

# Or seed from S3
pgbranch init myapp --from s3://my-bucket/backups/latest.dump

# Create feature branches — near-instant thanks to CoW
pgbranch create feature-auth
pgbranch create feature-payments

# Check what's running
pgbranch list
pgbranch status
```

### AI Agent / CI Automation

pgbranch's `--json` and `--non-interactive` flags make it easy to integrate with AI coding agents and CI/CD pipelines.

```bash
# Create an isolated branch for the agent to work in
pgbranch --json --non-interactive create agent-task-42

# Get connection info as JSON
pgbranch --json connection agent-task-42
# Output:
# {
#   "host": "localhost",
#   "port": 55434,
#   "database": "myapp",
#   "user": "postgres",
#   "password": "postgres",
#   "connection_string": "postgresql://postgres:postgres@localhost:55434/myapp"
# }

# Get connection as environment variables
pgbranch connection agent-task-42 --format env
# Output:
# DATABASE_HOST=localhost
# DATABASE_PORT=55434
# DATABASE_NAME=myapp
# DATABASE_USER=postgres
# DATABASE_URL=postgresql://postgres:postgres@localhost:55434/myapp

# Agent runs migrations, tests, etc. against the isolated database
# ...

# Reset to a clean state if needed
pgbranch reset agent-task-42

# Clean up when done
pgbranch delete agent-task-42

# Or destroy everything for a fresh start
pgbranch --non-interactive destroy --force
```

### Feature Branch Only

```yaml
git:
  auto_create_on_branch: true
  auto_switch_on_branch: true
  main_branch: main
  auto_create_branch_filter: "^feature/.*"
  exclude_branches:
    - main
    - master
    - develop
```

### Manual Mode (No Auto-Creation)

```yaml
git:
  auto_create_on_branch: false
  auto_switch_on_branch: false
  main_branch: main
```

## Workflow

### Typical Development Flow

1. **Start a new feature**:
   ```bash
   git checkout -b feature/user-authentication
   ```

2. **Database branch is created automatically** (via Git hooks):
   - Creates an isolated database for this branch
   - Runs post-commands to update your app configuration

3. **Develop your feature**:
   - Make schema changes, test migrations
   - Everything is isolated from the main database

4. **Switch back to main**:
   ```bash
   git checkout main
   ```
   - Automatically switches back to the main database
   - Post-commands restore your app configuration

5. **Review someone else's PR**:
   ```bash
   git checkout feature/other-feature
   ```
   - Automatically creates and switches to a database branch for the PR

6. **Manual branch switching**:
   ```bash
   pgbranch switch              # Interactive selection with fuzzy filtering
   pgbranch switch feature-auth # Direct switch
   pgbranch switch --template   # Switch to main database
   ```

### AI Agent Workflow

1. **Agent creates an isolated database**:
   ```bash
   BRANCH=$(pgbranch --json create task-123 | jq -r '.name')
   CONN=$(pgbranch --json connection "$BRANCH" | jq -r '.connection_string')
   ```

2. **Agent works against the isolated database** using `$CONN`

3. **If something goes wrong, reset and retry**:
   ```bash
   pgbranch reset "$BRANCH"
   ```

4. **Clean up after the task**:
   ```bash
   pgbranch delete "$BRANCH"
   ```

## Use Cases

- **Migration testing** — test database migrations in isolation before merging
- **Feature development** — each feature branch gets its own database state
- **PR review** — switch to any branch and have the correct database state
- **AI agent sandboxing** — give each agent task an isolated database with programmatic access
- **CI/CD preview databases** — spin up per-PR databases, destroy on merge
- **Migration rollback testing** — use `reset` to return a branch to its parent state
- **Parallel development** — multiple developers work without database conflicts
- **Data migration testing** — seed from production, test migrations, reset, iterate

## Requirements

- **Local mode**: Docker
- **Template mode**: PostgreSQL server with template database access
- **Both**: Git repository, Rust 1.70+ (for building from source)

#### Copy-on-Write storage (Local mode)

When pgbranch creates a database branch, it needs to copy the PostgreSQL data directory. **Copy-on-Write (CoW)** makes this near-instant and uses almost no extra disk space — only changed data blocks are actually duplicated. Without CoW, pgbranch falls back to a full copy, which still works but is slower and uses more disk for large databases.

pgbranch auto-detects the best available strategy. **On macOS, no setup is needed** — APFS cloning is used automatically.

**On Linux**, it depends on your filesystem. Check yours with:

```bash
df -T /home   # or wherever pgbranch stores data
```

| Your filesystem | CoW support | What you need to do |
|---|---|---|
| **ext4** (Ubuntu default) | No CoW | Nothing — pgbranch uses full copies. Works fine, just slower and uses more disk for large databases. |
| **Btrfs** | Built-in CoW | Nothing — pgbranch detects it automatically. Some distros use Btrfs by default (Fedora, openSUSE). |
| **XFS** | CoW via reflinks | Nothing — pgbranch detects it automatically, as long as the XFS partition was created with reflink support (default since xfsprogs 5.1). |
| **ZFS** | CoW via snapshots | Install `zfsutils-linux` and have a ZFS pool available. This is the best option if you're on ext4 and want CoW without reformatting. |

**ZFS is the only option you can add on top of an existing ext4 system** — it manages its own storage pools, so you can set one up on a spare disk or partition without touching your root filesystem. Btrfs and XFS require the partition to already be formatted with that filesystem.

#### Full install on Ubuntu

```bash
# Install Docker
sudo apt-get update
sudo apt-get install -y docker.io
sudo usermod -aG docker $USER
newgrp docker

# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# Build and install pgbranch
git clone https://github.com/clement-tourriere/pgbranch.git
cd pgbranch
cargo install --path .
```

##### Optional: set up ZFS for Copy-on-Write support

If you're on ext4 (Ubuntu default) and want near-instant branching:

```bash
# Install ZFS
sudo apt-get install -y zfsutils-linux

# Option 1: Automatic setup (recommended)
# pgbranch init will detect ZFS tools and offer to create a file-backed pool:
pgbranch init myapp
# → "ZFS tools detected but no ZFS pool found."
# → "Create a file-backed ZFS pool? (Y/n)"
# This creates a 10G sparse image, sets compression=lz4, recordsize=8k,
# and configures the mountpoint — all automatically.

# Option 2: Standalone ZFS setup
# If you already ran init, or want to set up ZFS separately:
pgbranch setup-zfs                  # Uses defaults (10G pool named "pgbranch")
pgbranch setup-zfs --size 20G       # Custom size
pgbranch setup-zfs --pool-name mypool  # Custom pool name

# Option 3: Manual setup with a spare disk
sudo zpool create pgdata /dev/sdX
sudo zfs set mountpoint=/pgdata pgdata
sudo chown $USER:$USER /pgdata
pgbranch init myapp
```

pgbranch auto-detects ZFS by matching the data directory against `zfs list` mountpoints. The detected dataset is persisted in local state, so no further configuration is needed. If auto-detection doesn't work (e.g. the mountpoints don't align), set `PGBRANCH_ZFS_DATASET=pgdata` before running `pgbranch init`. You can verify storage detection with `pgbranch doctor`.

## License

MIT License
