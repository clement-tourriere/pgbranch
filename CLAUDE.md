# pgbranch - Postgres Database Branching Tool

## Overview
pgbranch is a Rust-based tool that provides simple branching support for PostgreSQL databases during development. It enables developers to create and manage database branches that automatically synchronize with Git branches, making it easy to test migrations, features, and changes in isolation.

## Core Concept
The tool leverages PostgreSQL's TEMPLATE database feature to efficiently create database copies without the overhead of traditional `pg_dump`/`pg_restore` operations. When you create a new Git branch, pgbranch can automatically create a corresponding PostgreSQL database branch for isolated development.

## Key Features
- **Automatic Git Integration**: Creates PostgreSQL database branches when Git branches are created (via Git hooks)
- **Template-based Copying**: Uses PostgreSQL's TEMPLATE feature for fast database duplication
- **Configuration-driven**: Managed through a `.pgbranch` configuration file in your Git repository
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
- Configuration parsing and validation
- PostgreSQL connection and template management
- Git hook integration
- Database branch creation and management
- CLI interface for manual operations

## References
- PostgreSQL TEMPLATE documentation for implementation details
- Git hooks for automatic branch creation integration