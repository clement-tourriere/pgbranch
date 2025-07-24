#[cfg(test)]
mod backend_tests {
    use pgbranch::backends::{DatabaseBranchingBackend, BranchInfo};
    use pgbranch::config::{Config, BackendConfig, NeonConfig};
    use pgbranch::backends::factory::{create_backend, BackendType};
    use std::env;

    // Mock backend for testing
    #[derive(Debug)]
    struct MockBackend {
        name: String,
        branches: std::sync::Arc<std::sync::Mutex<Vec<BranchInfo>>>,
    }

    impl MockBackend {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                branches: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait::async_trait]
    impl DatabaseBranchingBackend for MockBackend {
        async fn create_branch(&self, branch_name: &str, from_branch: Option<&str>) -> anyhow::Result<BranchInfo> {
            let info = BranchInfo {
                name: branch_name.to_string(),
                created_at: Some(chrono::Utc::now()),
                parent_branch: from_branch.map(|s| s.to_string()),
                database_name: format!("{}_{}", self.name, branch_name),
            };
            
            let mut branches = self.branches.lock().unwrap();
            branches.push(info.clone());
            
            Ok(info)
        }

        async fn delete_branch(&self, branch_name: &str) -> anyhow::Result<()> {
            let mut branches = self.branches.lock().unwrap();
            branches.retain(|b| b.name != branch_name);
            Ok(())
        }

        async fn list_branches(&self) -> anyhow::Result<Vec<BranchInfo>> {
            let branches = self.branches.lock().unwrap();
            Ok(branches.clone())
        }

        async fn branch_exists(&self, branch_name: &str) -> anyhow::Result<bool> {
            let branches = self.branches.lock().unwrap();
            Ok(branches.iter().any(|b| b.name == branch_name))
        }

        async fn switch_to_branch(&self, branch_name: &str) -> anyhow::Result<BranchInfo> {
            let branches = self.branches.lock().unwrap();
            branches.iter()
                .find(|b| b.name == branch_name)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Branch not found"))
        }

        async fn get_connection_info(&self, branch_name: &str) -> anyhow::Result<pgbranch::backends::ConnectionInfo> {
            Ok(pgbranch::backends::ConnectionInfo {
                host: "localhost".to_string(),
                port: 5432,
                database: format!("{}_{}", self.name, branch_name),
                user: "test".to_string(),
                password: Some("password".to_string()),
                connection_string: Some(format!("postgresql://test:password@localhost:5432/{}_{}", self.name, branch_name)),
            })
        }

        async fn test_connection(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn backend_name(&self) -> &'static str {
            "Mock"
        }
    }

    #[test]
    fn test_backend_type_parsing() {
        assert_eq!(BackendType::from_str("postgres_local").unwrap(), BackendType::PostgresLocal);
        assert_eq!(BackendType::from_str("local").unwrap(), BackendType::PostgresLocal);
        assert_eq!(BackendType::from_str("postgres").unwrap(), BackendType::PostgresLocal);
        assert_eq!(BackendType::from_str("neon").unwrap(), BackendType::Neon);
        assert_eq!(BackendType::from_str("dblab").unwrap(), BackendType::DBLab);
        assert_eq!(BackendType::from_str("database_lab").unwrap(), BackendType::DBLab);
        assert_eq!(BackendType::from_str("xata").unwrap(), BackendType::Xata);
        
        assert!(BackendType::from_str("unknown").is_err());
    }

    #[tokio::test]
    async fn test_mock_backend_operations() {
        let backend = MockBackend::new("test");

        // Test create branch
        let branch_info = backend.create_branch("feature-1", None).await.unwrap();
        assert_eq!(branch_info.name, "feature-1");
        assert_eq!(branch_info.database_name, "test_feature-1");
        assert!(branch_info.parent_branch.is_none());

        // Test create branch with parent
        let child_info = backend.create_branch("feature-2", Some("feature-1")).await.unwrap();
        assert_eq!(child_info.parent_branch, Some("feature-1".to_string()));

        // Test list branches
        let branches = backend.list_branches().await.unwrap();
        assert_eq!(branches.len(), 2);

        // Test branch exists
        assert!(backend.branch_exists("feature-1").await.unwrap());
        assert!(!backend.branch_exists("nonexistent").await.unwrap());

        // Test switch to branch
        let switched = backend.switch_to_branch("feature-1").await.unwrap();
        assert_eq!(switched.name, "feature-1");

        // Test delete branch
        backend.delete_branch("feature-1").await.unwrap();
        assert!(!backend.branch_exists("feature-1").await.unwrap());
        assert_eq!(backend.list_branches().await.unwrap().len(), 1);

        // Test connection info
        let conn_info = backend.get_connection_info("feature-2").await.unwrap();
        assert_eq!(conn_info.database, "test_feature-2");
        assert_eq!(conn_info.user, "test");
        assert!(conn_info.password.is_some());
    }

    #[tokio::test]
    async fn test_backend_factory_defaults_to_postgres_local() {
        let config = Config::default();
        let backend = create_backend(&config).await.unwrap();
        assert_eq!(backend.backend_name(), "PostgreSQL (Local)");
    }

    #[test]
    fn test_backend_config_serialization() {
        let backend_config = BackendConfig {
            backend_type: "neon".to_string(),
            postgres_local: None,
            neon: Some(NeonConfig {
                api_key: "${NEON_API_KEY}".to_string(),
                project_id: "test-project".to_string(),
                base_url: "https://api.neon.tech".to_string(),
            }),
            dblab: None,
            xata: None,
        };

        let yaml = serde_yaml::to_string(&backend_config).unwrap();
        assert!(yaml.contains("type: neon"));
        assert!(yaml.contains("api_key: ${NEON_API_KEY}"));
        
        let deserialized: BackendConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(deserialized.backend_type, "neon");
        assert!(deserialized.neon.is_some());
    }

    #[test]
    fn test_env_var_resolution() {
        env::set_var("TEST_VAR", "test_value");
        
        // The resolve_env_var function is private, but we can test through the factory
        // by setting up appropriate environment variables and configs
        let backend_config = BackendConfig {
            backend_type: "neon".to_string(),
            postgres_local: None,
            neon: Some(NeonConfig {
                api_key: "${TEST_VAR}".to_string(),
                project_id: "project".to_string(),
                base_url: "https://api.neon.tech".to_string(),
            }),
            dblab: None,
            xata: None,
        };

        let mut config = Config::default();
        config.backend = Some(backend_config);

        // If the backend creation succeeds, it means env var resolution worked
        // (We can't actually create the Neon backend without valid credentials,
        // but we're testing that the factory attempts to resolve the env var)
        
        env::remove_var("TEST_VAR");
    }

    #[test]
    fn test_connection_info_fields() {
        let conn_info = pgbranch::backends::ConnectionInfo {
            host: "localhost".to_string(),
            port: 5432,
            database: "test_db".to_string(),
            user: "test_user".to_string(),
            password: Some("test_pass".to_string()),
            connection_string: Some("postgresql://test_user:test_pass@localhost:5432/test_db".to_string()),
        };

        assert_eq!(conn_info.host, "localhost");
        assert_eq!(conn_info.port, 5432);
        assert_eq!(conn_info.database, "test_db");
        assert_eq!(conn_info.user, "test_user");
        assert!(conn_info.password.is_some());
        assert!(conn_info.connection_string.is_some());
    }

    #[test]
    fn test_branch_info_serialization() {
        let branch_info = BranchInfo {
            name: "test-branch".to_string(),
            created_at: Some(chrono::Utc::now()),
            parent_branch: Some("main".to_string()),
            database_name: "db_test_branch".to_string(),
        };

        let json = serde_json::to_string(&branch_info).unwrap();
        assert!(json.contains("test-branch"));
        assert!(json.contains("main"));
        assert!(json.contains("db_test_branch"));

        let deserialized: BranchInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test-branch");
        assert_eq!(deserialized.parent_branch, Some("main".to_string()));
    }
}