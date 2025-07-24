# pgbranch - Multi-Backend Database Branching Tool

## Overview
pgbranch is a Rust-based tool that provides database branching support during development. It enables developers to create and manage database branches that automatically synchronize with Git branches, making it easy to test migrations, features, and changes in isolation. Originally built for PostgreSQL, pgbranch now supports multiple backend providers.

## Core Concept
The tool provides a unified interface for database branching across multiple providers:
- **Local PostgreSQL**: Uses TEMPLATE database feature for efficient copying
- **Neon**: Cloud-native PostgreSQL with instant branching
- **Database Lab Engine**: Thin cloning for large databases
- **Xata**: Serverless PostgreSQL with built-in branching

When you create a new Git branch, pgbranch can automatically create a corresponding database branch for isolated development using your chosen backend.

## Key Features
- **Multi-Backend Support**: Choose from local PostgreSQL, Neon, Database Lab, or Xata
- **Automatic Git Integration**: Creates database branches when Git branches are created (via Git hooks)
- **Backend-Specific Optimizations**: Each backend uses its native branching capabilities
- **Unified Configuration**: Same `.pgbranch.yml` format works across all backends
- **Post-Commands**: Run migrations, update configs, restart services after branch operations
- **Rust Implementation**: Single binary with cross-platform support

## Use Cases
- Test database migrations in isolation before merging to main
- Create feature-specific database environments
- Provision preview environments for database changes
- Quickly revert to main development database state

## Configuration
The tool is configured via a `.pgbranch` file in your Git repository root. This file should contain:
- Database connection settings
- Template database configuration
- Branch naming conventions
- Git hook preferences

## Local Configuration System

pgbranch supports a comprehensive local configuration system with three levels of precedence:

### Configuration Hierarchy (highest to lowest):
1. **Environment Variables** - Quick toggles and overrides
2. **Local Config File** (`.pgbranch.local.yml`) - Project-specific local overrides  
3. **Committed Config** (`.pgbranch.yml`) - Team shared configuration

### Environment Variables:
- `PGBRANCH_DISABLED=true` - Completely disable pgbranch
- `PGBRANCH_SKIP_HOOKS=true` - Skip Git hook execution
- `PGBRANCH_AUTO_CREATE=false` - Override auto_create_on_branch
- `PGBRANCH_AUTO_SWITCH=false` - Override auto_switch_on_branch
- `PGBRANCH_BRANCH_FILTER_REGEX=...` - Override branch filtering
- `PGBRANCH_DISABLED_BRANCHES=main,release/*` - Disable for specific branches
- `PGBRANCH_CURRENT_BRANCH_DISABLED=true` - Disable for current branch only
- `PGBRANCH_DATABASE_HOST=...` - Override database connection settings

### Local Config File:
Create `.pgbranch.local.yml` in your project root to override team settings locally:

```yaml
# Example .pgbranch.local.yml
disabled: false
disabled_branches:
  - "feature/*"
  - hotfix
database:
  host: localhost
  port: 5433
  database_prefix: dev_prefix
git:
  auto_switch_on_branch: false
  main_branch: develop
```

### Commands:
- `pgbranch config-show` - Display effective configuration with all overrides
- `pgbranch init` - Suggests adding local config to gitignore

## Development Commands
When working on this project, use these commands:

```bash
# Build the project
cargo build

# Run tests
cargo test

# Run with development profile
cargo run

# Build release version
cargo build --release

# Run linting
cargo clippy

# Format code
cargo fmt

# Check for issues
cargo check
```

## Project Structure
- **Backend Abstraction Layer** (`src/backends/`):
  - `mod.rs`: Core trait definitions (`DatabaseBranchingBackend`)
  - `postgres_local.rs`: Local PostgreSQL implementation
  - `neon.rs`: Neon API implementation
  - `dblab.rs`: Database Lab Engine implementation
  - `xata.rs`: Xata API implementation
  - `factory.rs`: Backend instantiation logic
- **Core Components**:
  - `config.rs`: Multi-backend configuration parsing and validation
  - `cli.rs`: Backend-agnostic CLI commands
  - `post_commands.rs`: Post-command execution with backend connection info
  - `git.rs`: Git hook integration
  - `database.rs`: Legacy PostgreSQL-specific utilities

## Architecture

The tool uses a trait-based architecture for backend abstraction:

```rust
#[async_trait]
pub trait DatabaseBranchingBackend: Send + Sync {
    async fn create_branch(&self, branch_name: &str, from_branch: Option<&str>) -> Result<BranchInfo>;
    async fn delete_branch(&self, branch_name: &str) -> Result<()>;
    async fn list_branches(&self) -> Result<Vec<BranchInfo>>;
    async fn branch_exists(&self, branch_name: &str) -> Result<bool>;
    async fn get_connection_info(&self, branch_name: &str) -> Result<ConnectionInfo>;
    // ... more methods
}
```

This allows all backends to provide consistent functionality while leveraging their specific capabilities.

## References
- PostgreSQL TEMPLATE documentation for implementation details
- Git hooks for automatic branch creation integration