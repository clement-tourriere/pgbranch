# pgbranch

A powerful multi-backend database branching tool that automatically syncs database branches with Git branches. Originally designed for PostgreSQL, pgbranch now supports multiple database branching providers including local PostgreSQL, Neon, Database Lab Engine, and Xata.

## 🚀 Features

- **Multi-Backend Support**: Choose from local PostgreSQL, Neon, Database Lab Engine, or Xata
- **Automatic Database Branching**: Creates database branches when you create Git branches
- **Fast Database Operations**: Uses provider-specific optimizations for efficient branching
- **Unified Configuration**: Same `.pgbranch.yml` format works across all backends
- **Git Integration**: Automatic setup via Git hooks
- **Post-Commands**: Run commands after branch creation/switching (migrations, config updates, etc.)
- **Interactive CLI**: Browse and switch branches with arrow keys and fuzzy filtering

## 📦 Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/clement-tourriere/pgbranch.git
cd pgbranch

# Install directly with cargo (recommended)
cargo install --path .

# Or build manually and copy to PATH
cargo build --release
# Copy target/release/pgbranch to your PATH
```

## 🚀 Quick Start

### 1. Initialize Configuration

```bash
pgbranch init
```

### 2. Choose Your Backend

pgbranch supports multiple database branching backends. Choose the one that fits your needs:

#### Local PostgreSQL (Default)

Traditional local PostgreSQL using TEMPLATE databases:

```yaml
# .pgbranch.yml
database:
  host: localhost
  port: 5432
  user: postgres
  password: null
  template_database: myapp_dev
  database_prefix: myapp

git:
  auto_create_on_branch: true
  exclude_branches: [main, master]

post_commands:
  - echo 'DATABASE_URL=postgresql://{db_user}@{db_host}:{db_port}/{db_name}' >> .env
```

#### Neon (Serverless PostgreSQL)

Cloud-native PostgreSQL with instant branching:

```yaml
# .pgbranch.yml
backend:
  type: neon
  neon:
    api_key: ${NEON_API_KEY}
    project_id: ${NEON_PROJECT_ID}

git:
  auto_create_on_branch: true
  exclude_branches: [main, master]

post_commands:
  - echo 'DATABASE_URL=postgresql://{db_user}@{db_host}:{db_port}/{db_name}' >> .env
```

#### Database Lab Engine

Thin cloning for large databases:

```yaml
# .pgbranch.yml
backend:
  type: dblab
  dblab:
    api_url: ${DBLAB_API_URL}
    auth_token: ${DBLAB_AUTH_TOKEN}

git:
  auto_create_on_branch: true
  exclude_branches: [main, master]

post_commands:
  - echo 'DATABASE_URL=postgresql://{db_user}:{db_password}@{db_host}:{db_port}/{db_name}' >> .env
```

#### Xata

Serverless PostgreSQL with built-in branching:

```yaml
# .pgbranch.yml
backend:
  type: xata
  xata:
    organization_id: ${XATA_ORG_ID}
    project_id: ${XATA_PROJECT_ID}
    api_key: ${XATA_API_KEY}

git:
  auto_create_on_branch: true
  exclude_branches: [main, master]

post_commands:
  - echo 'DATABASE_URL={connection_string}' >> .env
```

### 3. Install Git Hooks

```bash
pgbranch install-hooks
```

### 4. Start Branching!

Now when you create a Git branch, a corresponding database branch is created automatically:

```bash
git checkout -b feature/new-feature
# Database branch created and switched automatically!
```

## 🔧 CLI Usage

### Core Commands

```bash
# Create a database branch
pgbranch create feature-auth

# List all database branches (shows current branch with *)
pgbranch list

# Switch to a database branch (creates if doesn't exist)
pgbranch switch feature-auth

# Interactive switch with arrow keys and fuzzy filtering
pgbranch switch

# Delete a database branch
pgbranch delete feature-auth

# Clean up old branches
pgbranch cleanup --max-count 5
```

### Configuration & Testing

```bash
# Show current configuration
pgbranch config

# Check configuration and connectivity
pgbranch check

# Test post-commands without database operations
pgbranch test-post-commands feature-branch

# Show available template variables
pgbranch templates feature-branch
```

## 📋 Configuration

### Backend Configuration

Each backend has its own configuration requirements:

**Local PostgreSQL** (default, backward compatible):
```yaml
database:
  host: localhost
  port: 5432
  user: postgres
  password: null
  template_database: myapp_dev
  database_prefix: myapp
```

**Multi-Backend** (new format):
```yaml
backend:
  type: neon  # or 'postgres_local', 'dblab', 'xata'
  neon:
    api_key: ${NEON_API_KEY}
    project_id: ${NEON_PROJECT_ID}
  # ... backend-specific config
```

### Post-Commands

Post-commands run after branch creation/switching and work identically across all backends:

```yaml
post_commands:
  # Simple command
  - echo 'Switched to {branch_name}!'
  
  # Complex command with options
  - name: "Run migrations"
    command: "npm run migrate"
    working_dir: "./backend"
    condition: "file_exists:package.json"
    continue_on_error: false
    environment:
      DATABASE_URL: "postgresql://{db_user}@{db_host}:{db_port}/{db_name}"
  
  # Replace action for updating files
  - action: "replace"
    file: ".env.local"
    pattern: "DATABASE_URL=.*"
    replacement: "DATABASE_URL=postgresql://{db_user}@{db_host}:{db_port}/{db_name}"
    create_if_missing: true
```

### Template Variables

Available in all post-commands:
- `{branch_name}` - Git branch name
- `{db_name}` - Database name
- `{db_host}` - Database host
- `{db_port}` - Database port
- `{db_user}` - Database username
- `{db_password}` - Database password (if available)
- `{connection_string}` - Full connection string (some backends)

## 🌟 Backend Comparison

| Feature | Local PostgreSQL | Neon | Database Lab | Xata |
|---------|-----------------|------|--------------|------|
| **Setup Complexity** | Low | Medium | Medium | Medium |
| **Cloud Native** | ❌ | ✅ | Optional | ✅ |
| **Branch Speed** | Fast | Instant | Instant | Fast |
| **Large Databases** | Slow | Fast | Very Fast | Fast |
| **Cost** | Self-hosted | Usage-based | Self-hosted/Cloud | Usage-based |
| **Point-in-time** | ❌ | ✅ | Via snapshots | ❌ |
| **Best For** | Development | Cloud apps | Large databases | Serverless |

## 🔄 Workflow Examples

### Feature Development

```bash
# Start new feature
git checkout -b feature/user-auth

# Database automatically created and switched
# Run your app - it's now using the feature database!

# Make schema changes, run migrations
npm run migrate

# Switch back to main
git checkout main
# Automatically switches to main database
```

### PR Review

```bash
# Fetch and checkout PR branch
git fetch origin
git checkout feature/cool-feature

# Database automatically created from main
# Your local env now matches the PR's database state!
```

### Manual Database Management

```bash
# Interactive branch selection
pgbranch switch

# Direct switch
pgbranch switch feature-auth

# Create without Git branch
pgbranch create experiment-1
```

## 🏗️ Architecture

pgbranch uses a flexible backend architecture:

```
┌─────────────┐     ┌──────────────┐
│   CLI/Git   │────▶│ Backend Trait│
└─────────────┘     └──────┬───────┘
                           │
        ┌──────────────────┼──────────────────┐
        │                  │                  │
   ┌────▼─────┐      ┌────▼────┐      ┌─────▼────┐
   │PostgreSQL│      │  Neon   │      │   Xata   │
   │  Local   │      │   API   │      │   API    │
   └──────────┘      └─────────┘      └──────────┘
```

All backends implement the same interface, ensuring consistent behavior and allowing you to switch providers without changing your configuration structure or post-commands.

## 📚 Advanced Usage

### Environment Variables

All configuration values can use environment variables:
```yaml
backend:
  type: neon
  neon:
    api_key: ${NEON_API_KEY}
    project_id: ${NEON_PROJECT_ID:-default-project}
```

### Conditional Execution

Control when commands run:
```yaml
post_commands:
  - name: "Django migrations"
    command: "python manage.py migrate"
    condition: "file_exists:manage.py"
    
  - name: "Node migrations"
    command: "npm run migrate"
    condition: "file_exists:package.json"
```

### Multiple Environments

Use different backends for different environments:
```bash
# Development
cp .pgbranch.local.yml .pgbranch.yml

# Staging (using Neon)
cp .pgbranch.neon.yml .pgbranch.yml

# Production (using Database Lab)
cp .pgbranch.dblab.yml .pgbranch.yml
```

## 🤝 Contributing

Contributions are welcome! The backend system is extensible - new providers can be added by implementing the `DatabaseBranchingBackend` trait.

## 📄 License

MIT License

## 🙏 Acknowledgments

- PostgreSQL for the TEMPLATE feature
- Neon, Database Lab Engine, and Xata teams for their excellent APIs
- The Rust community for amazing libraries