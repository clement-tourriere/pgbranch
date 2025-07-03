# pgbranch

A Rust tool for creating PostgreSQL database branches that automatically sync with Git branches.

## Features

- **Automatic Database Branching**: Creates PostgreSQL database branches when you create Git branches
- **Fast Database Copying**: Uses PostgreSQL's TEMPLATE feature for efficient database duplication
- **Configurable**: Fully configurable via `.pgbranch` configuration file
- **Git Integration**: Automatic setup via Git hooks
- **Regex Filtering**: Create database branches only for specific branch patterns
- **CLI Interface**: Manual database branch management

## Installation

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

## Quick Start

1. Initialize configuration in your Git repository:
   ```bash
   pgbranch init
   ```

2. Edit `.pgbranch.yml` file to configure your PostgreSQL connection:
   ```yaml
   database:
     host: localhost
     port: 5432
     user: postgres
     password: null
     template_database: myapp_dev
     database_prefix: myapp
   
   git:
     auto_create_on_branch: true
     auto_switch_on_branch: true
     main_branch: main
     auto_create_branch_filter: "^(feature|bugfix)/.*"
     exclude_branches:
       - main
       - master
   
   behavior:
     auto_cleanup: false
     max_branches: 10
     naming_strategy: prefix
   ```

3. Install Git hooks for automatic database branch creation:
   ```bash
   pgbranch install-hooks
   ```

4. Now when you create a new Git branch, a corresponding database will be created automatically!

## CLI Usage

### Manual Database Branch Management

```bash
# Create a database branch
pgbranch create feature-auth

# List all database branches (shows current branch with *)
pgbranch list

# Switch to a database branch (creates if doesn't exist)
pgbranch switch feature-auth

# Interactive switch with arrow keys and fuzzy filtering
pgbranch switch

# Switch to main/template database
pgbranch switch --template

# Delete a database branch
pgbranch delete feature-auth

# Clean up old branches (keeps most recent N)
pgbranch cleanup --max-count 5

# Show current configuration
pgbranch config
```

### Git Hook Management

```bash
# Install Git hooks
pgbranch install-hooks

# Uninstall Git hooks
pgbranch uninstall-hooks
```

### Configuration and Testing

```bash
# Initialize configuration (with Docker Compose detection)
pgbranch init

# Check configuration and database connectivity
pgbranch check

# Test post-commands without database connection
pgbranch test-post-commands feature-branch

# Show available template variables
pgbranch templates feature-branch

# Test switch functionality without database operations
pgbranch test-switch feature-branch
```

## Configuration

The `.pgbranch.yml` file supports the following configuration options (YAML format only):

### Database Configuration

- `host`: PostgreSQL server host (default: "localhost")
- `port`: PostgreSQL server port (default: 5432)
- `user`: PostgreSQL username (default: "postgres")
- `password`: PostgreSQL password (optional)
- `template_database`: Database to use as template (default: "template0")
- `database_prefix`: Prefix for created database branches (default: "pgbranch")

### Git Configuration

- `auto_create_on_branch`: Enable automatic database branch creation (default: true)
- `auto_switch_on_branch`: Enable automatic database branch switching via Git hooks (default: true)
- `main_branch`: Name of the main Git branch (default: "main", auto-detected during init)
- `auto_create_branch_filter`: Only create database branches for Git branches matching this regex (optional)
- `branch_filter_regex`: Legacy alias for auto_create_branch_filter (optional)
- `exclude_branches`: List of Git branches to exclude from database branch creation

### Behavior Configuration

- `auto_cleanup`: Automatically clean up old database branches (default: false)
- `max_branches`: Maximum number of database branches to keep (default: 10)
- `naming_strategy`: How to name database branches ("prefix", "suffix", "replace")

### Post-Commands Configuration

Post-commands allow you to automatically execute actions after database branch creation, such as updating application configuration, running migrations, or restarting services. This is particularly useful for frameworks like Django where you need to switch the application to use the new database.

- `post_commands`: Array of commands to execute after database branch creation

#### Post-Command Types

**1. Simple Commands (String)**
```yaml
post_commands:
  - "echo 'Database ready for {branch_name}!'"
  - "npm run migrate"
```

**2. Complex Commands (Object)**
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

**3. Replace Actions (Built-in)**
```yaml
post_commands:
  - action: "replace"
    name: "Update database configuration"
    file: ".env.local"
    pattern: "DATABASE_URL=.*"
    replacement: "DATABASE_URL=postgresql://{db_user}@{db_host}:{db_port}/{db_name}"
    create_if_missing: true
    condition: "file_exists:manage.py"
    continue_on_error: false
```

#### Template Variables

All post-commands support template variable substitution:

- `{branch_name}`: Current Git branch name
- `{db_name}`: Generated database name (with prefix/suffix)
- `{db_host}`: Database host
- `{db_port}`: Database port
- `{db_user}`: Database username
- `{db_password}`: Database password (if configured)
- `{template_db}`: Template database name
- `{prefix}`: Database prefix

#### Command Options

- `name`: Optional descriptive name for the command
- `working_dir`: Directory to run the command in
- `condition`: Conditional execution (supports `file_exists:filename`, `always`, `never`)
- `continue_on_error`: Continue execution if this command fails (default: false)
- `environment`: Environment variables to set for the command

#### Replace Action Options

- `action`: Must be "replace"
- `file`: Path to the file to modify
- `pattern`: Regular expression pattern to match
- `replacement`: Text to replace matches with (supports template variables)
- `create_if_missing`: Create the file if it doesn't exist (default: false)

## Examples

### Basic Setup for Development

```yaml
database:
  host: localhost
  port: 5432
  user: dev_user
  template_database: myapp_dev
  database_prefix: myapp

git:
  auto_create_on_branch: true
  auto_switch_on_branch: true
  main_branch: main
  exclude_branches:
    - main
    - develop
```

### Django Integration with Post-Commands

```yaml
database:
  host: localhost
  port: 5432
  user: postgres
  template_database: myapp_dev
  database_prefix: myapp

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

### Node.js/Express Setup

```yaml
database:
  host: localhost
  port: 5432
  user: postgres
  template_database: myapp_dev
  database_prefix: myapp

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

### Manual Mode (No Auto-Creation or Auto-Switching)

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
   - Creates `myapp_feature_user_authentication` database
   - Runs post-commands to update your app configuration
   - Restarts services if configured

3. **Develop your feature**:
   - Your application now uses the isolated database branch
   - Make schema changes, test migrations, etc.
   - Everything is isolated from `main` branch database

4. **Switch back to main**:
   ```bash
   git checkout main
   ```
   - Automatically switches to main database via Git hooks
   - Post-commands update configuration back to main database
   - Your application switches back to main database state

5. **Review someone else's PR**:
   ```bash
   git fetch origin
   git checkout feature/other-feature
   ```
   - Automatically creates database branch for the PR (if auto_create enabled)
   - Automatically switches to that database branch (if auto_switch enabled)
   - Updates your local environment to use that database

6. **Manual database branch switching**:
   ```bash
   # Interactive selection with arrow keys and fuzzy filtering
   pgbranch switch
   
   # Direct switch to specific branch
   pgbranch switch feature-authentication
   
   # Switch to main/template database
   pgbranch switch --template
   ```

### Testing Post-Commands

Before committing your configuration:

```bash
# Test your post-commands
pgbranch test-post-commands feature/test-branch

# Test switch functionality without database operations
pgbranch test-switch feature/test-branch

# Check what template variables are available
pgbranch templates feature/test-branch

# Verify configuration is valid
pgbranch check
```

## How It Works

1. **Template-Based Copying**: Uses PostgreSQL's `CREATE DATABASE ... WITH TEMPLATE` feature for fast database duplication
2. **Git Hook Integration**: Installs `post-checkout` and `post-merge` hooks to automatically create and switch database branches
3. **Smart Filtering**: Uses regex patterns and exclude lists to control which Git branches trigger database creation/switching
4. **Configuration Discovery**: Searches for `.pgbranch.yml` or `.pgbranch.yaml` files in current directory and parent directories
5. **Branch State Management**: Tracks current database branch in configuration file for consistent state
6. **Interactive Selection**: Provides arrow-key navigation with fuzzy filtering for easy branch switching
7. **Post-Command Execution**: Runs on every branch switch to update application configuration

## Use Cases

- **Migration Testing**: Test database migrations in isolation before merging
- **Feature Development**: Each feature branch gets its own database state
- **Preview Environments**: Automatically provision database branches for feature previews
- **Parallel Development**: Multiple developers can work on different features without database conflicts
- **PR Review**: Quickly switch to any branch and have the correct database state
- **Manual Database Management**: Switch between database branches independently of Git branches
- **Multi-Environment Testing**: Test the same code against different database states

## Requirements

- PostgreSQL server with template database access
- Git repository
- Rust 1.70+ (for building from source)

## License

MIT License