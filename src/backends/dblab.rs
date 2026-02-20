use super::{BranchInfo, ConnectionInfo, DatabaseBranchingBackend, DoctorCheck, DoctorReport};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct DBLabBackend {
    client: Client,
    api_url: String,
    auth_token: String,
}

#[derive(Debug, Serialize)]
struct CreateCloneRequest {
    #[serde(rename = "cloneName")]
    clone_name: String,
    #[serde(rename = "snapshotID", skip_serializing_if = "Option::is_none")]
    snapshot_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DBLabClone {
    id: String,
    name: String,
    #[serde(rename = "createdAt")]
    created_at: DateTime<Utc>,
    #[serde(rename = "snapshotID")]
    snapshot_id: String,
    #[allow(dead_code)]
    status: String,
    db: DBLabDatabase,
}

#[derive(Debug, Deserialize)]
struct DBLabDatabase {
    host: String,
    port: u16,
    #[serde(rename = "dbname")]
    database: String,
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct ListClonesResponse {
    clones: Vec<DBLabClone>,
}

#[derive(Debug, Deserialize)]
struct CreateCloneResponse {
    clone: DBLabClone,
}

#[derive(Debug, Deserialize)]
struct DBLabSnapshot {
    id: String,
    #[serde(rename = "createdAt")]
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct ListSnapshotsResponse {
    snapshots: Vec<DBLabSnapshot>,
}

impl DBLabBackend {
    pub fn new(api_url: String, auth_token: String) -> Result<Self> {
        let client = Client::new();

        Ok(Self {
            client,
            api_url: api_url.trim_end_matches('/').to_string(),
            auth_token,
        })
    }

    async fn make_request<T: for<'de> Deserialize<'de>>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> Result<T> {
        let url = format!("{}{}", self.api_url, path);
        let mut request = self
            .client
            .request(method, &url)
            .header("Verification-Token", &self.auth_token)
            .header("Content-Type", "application/json");

        if let Some(body) = body {
            request = request.json(body);
        }

        let response = request
            .send()
            .await
            .with_context(|| format!("Failed to send request to {}", url))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            anyhow::bail!(
                "DBLab API request failed with status {}: {}",
                status,
                error_text
            );
        }

        response
            .json()
            .await
            .with_context(|| "Failed to parse JSON response from DBLab API")
    }

    async fn get_latest_snapshot(&self) -> Result<String> {
        let response: ListSnapshotsResponse = self
            .make_request(reqwest::Method::GET, "/api/snapshots", None::<&()>)
            .await?;

        response
            .snapshots
            .into_iter()
            .max_by_key(|s| s.created_at)
            .map(|s| s.id)
            .ok_or_else(|| anyhow::anyhow!("No snapshots available"))
    }

    fn normalize_clone_name(branch_name: &str) -> String {
        branch_name
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_string()
    }
}

#[async_trait]
impl DatabaseBranchingBackend for DBLabBackend {
    async fn create_branch(
        &self,
        branch_name: &str,
        from_branch: Option<&str>,
    ) -> Result<BranchInfo> {
        let clone_name = Self::normalize_clone_name(branch_name);

        let snapshot_id = if let Some(from) = from_branch {
            let clones = self.list_branches().await?;
            clones
                .into_iter()
                .find(|c| c.name == from)
                .map(|c| c.database_name)
        } else {
            None
        };

        let snapshot_id = match snapshot_id {
            Some(id) => id,
            None => self.get_latest_snapshot().await?,
        };

        let request = CreateCloneRequest {
            clone_name: clone_name.clone(),
            snapshot_id: Some(snapshot_id),
        };

        let response: CreateCloneResponse = self
            .make_request(reqwest::Method::POST, "/api/clones", Some(&request))
            .await?;

        Ok(BranchInfo {
            name: branch_name.to_string(),
            created_at: Some(response.clone.created_at),
            parent_branch: from_branch.map(|s| s.to_string()),
            database_name: response.clone.snapshot_id,
            state: Some("running".to_string()),
        })
    }

    async fn delete_branch(&self, branch_name: &str) -> Result<()> {
        let clone_name = Self::normalize_clone_name(branch_name);

        let clones: ListClonesResponse = self
            .make_request(reqwest::Method::GET, "/api/clones", None::<&()>)
            .await?;
        let clone = clones
            .clones
            .into_iter()
            .find(|c| c.name == clone_name)
            .ok_or_else(|| anyhow::anyhow!("Clone '{}' not found", branch_name))?;

        let path = format!("/api/clones/{}", clone.id);
        let _: serde_json::Value = self
            .make_request(reqwest::Method::DELETE, &path, None::<&()>)
            .await?;

        Ok(())
    }

    async fn list_branches(&self) -> Result<Vec<BranchInfo>> {
        let response: ListClonesResponse = self
            .make_request(reqwest::Method::GET, "/api/clones", None::<&()>)
            .await?;

        let branches = response
            .clones
            .into_iter()
            .map(|clone| BranchInfo {
                name: clone.name,
                created_at: Some(clone.created_at),
                parent_branch: None,
                database_name: clone.snapshot_id,
                state: Some("running".to_string()),
            })
            .collect();

        Ok(branches)
    }

    async fn branch_exists(&self, branch_name: &str) -> Result<bool> {
        let clone_name = Self::normalize_clone_name(branch_name);
        let branches = self.list_branches().await?;
        Ok(branches.iter().any(|b| b.name == clone_name))
    }

    async fn switch_to_branch(&self, branch_name: &str) -> Result<BranchInfo> {
        let clone_name = Self::normalize_clone_name(branch_name);
        let branches = self.list_branches().await?;
        branches
            .into_iter()
            .find(|b| b.name == clone_name)
            .ok_or_else(|| anyhow::anyhow!("Branch '{}' does not exist", branch_name))
    }

    async fn get_connection_info(&self, branch_name: &str) -> Result<ConnectionInfo> {
        let clone_name = Self::normalize_clone_name(branch_name);

        let clones: ListClonesResponse = self
            .make_request(reqwest::Method::GET, "/api/clones", None::<&()>)
            .await?;
        let clone = clones
            .clones
            .into_iter()
            .find(|c| c.name == clone_name)
            .ok_or_else(|| anyhow::anyhow!("Clone '{}' not found", branch_name))?;

        let db = clone.db;
        let connection_string = format!(
            "postgresql://{}:{}@{}:{}/{}",
            db.username, db.password, db.host, db.port, db.database
        );

        Ok(ConnectionInfo {
            host: db.host,
            port: db.port,
            database: db.database,
            user: db.username,
            password: Some(db.password),
            connection_string: Some(connection_string),
        })
    }

    async fn test_connection(&self) -> Result<()> {
        let _: ListClonesResponse = self
            .make_request(reqwest::Method::GET, "/api/clones", None::<&()>)
            .await?;
        Ok(())
    }

    async fn doctor(&self) -> Result<DoctorReport> {
        let check = match self.test_connection().await {
            Ok(_) => DoctorCheck {
                name: "DBLab API".to_string(),
                available: true,
                detail: "Connected to DBLab API".to_string(),
            },
            Err(e) => DoctorCheck {
                name: "DBLab API".to_string(),
                available: false,
                detail: format!("Failed: {}", e),
            },
        };
        Ok(DoctorReport {
            checks: vec![check],
        })
    }

    fn backend_name(&self) -> &'static str {
        "Database Lab Engine"
    }
}
