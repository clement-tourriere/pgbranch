use super::{BranchInfo, ConnectionInfo, DatabaseBranchingBackend, DoctorCheck, DoctorReport};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct NeonBackend {
    client: Client,
    api_key: String,
    project_id: String,
    base_url: String,
}

#[derive(Debug, Serialize)]
struct CreateBranchRequest {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NeonBranch {
    id: String,
    name: String,
    created_at: DateTime<Utc>,
    #[serde(default)]
    parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListBranchesResponse {
    branches: Vec<NeonBranch>,
}

#[derive(Debug, Deserialize)]
struct CreateBranchResponse {
    branch: NeonBranch,
}

#[derive(Debug, Deserialize)]
struct NeonEndpoint {
    #[allow(dead_code)]
    id: String,
    database_host: String,
    database_name: String,
    database_user: String,
    #[serde(default)]
    database_password: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListEndpointsResponse {
    endpoints: Vec<NeonEndpoint>,
}

impl NeonBackend {
    pub fn new(api_key: String, project_id: String, base_url: Option<String>) -> Result<Self> {
        let client = Client::new();
        let base_url = base_url.unwrap_or_else(|| "https://console.neon.tech/api/v2".to_string());

        Ok(Self {
            client,
            api_key,
            project_id,
            base_url,
        })
    }

    async fn make_request<T: for<'de> Deserialize<'de>>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> Result<T> {
        let url = format!("{}/{}", self.base_url, path);
        let mut request = self
            .client
            .request(method, &url)
            .header("Authorization", format!("Bearer {}", self.api_key))
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
                "Neon API request failed with status {}: {}",
                status,
                error_text
            );
        }

        response
            .json()
            .await
            .with_context(|| "Failed to parse JSON response from Neon API")
    }

    async fn get_branch_endpoint(&self, branch_name: &str) -> Result<NeonEndpoint> {
        let path = format!("projects/{}/endpoints", self.project_id);
        let response: ListEndpointsResponse = self
            .make_request(reqwest::Method::GET, &path, None::<&()>)
            .await?;

        for endpoint in response.endpoints {
            if endpoint.database_name == branch_name || endpoint.id.contains(branch_name) {
                return Ok(endpoint);
            }
        }

        anyhow::bail!("No endpoint found for branch: {}", branch_name);
    }
}

#[async_trait]
impl DatabaseBranchingBackend for NeonBackend {
    async fn create_branch(
        &self,
        branch_name: &str,
        from_branch: Option<&str>,
    ) -> Result<BranchInfo> {
        let request = CreateBranchRequest {
            name: branch_name.to_string(),
            parent_id: from_branch.map(|s| s.to_string()),
        };

        let path = format!("projects/{}/branches", self.project_id);
        let response: CreateBranchResponse = self
            .make_request(reqwest::Method::POST, &path, Some(&request))
            .await?;

        Ok(BranchInfo {
            name: response.branch.name,
            created_at: Some(response.branch.created_at),
            parent_branch: response.branch.parent_id,
            database_name: response.branch.id,
            state: Some("running".to_string()),
        })
    }

    async fn delete_branch(&self, branch_name: &str) -> Result<()> {
        let branches = self.list_branches().await?;
        let branch = branches
            .into_iter()
            .find(|b| b.name == branch_name)
            .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", branch_name))?;

        let path = format!(
            "projects/{}/branches/{}",
            self.project_id, branch.database_name
        );
        let _: serde_json::Value = self
            .make_request(reqwest::Method::DELETE, &path, None::<&()>)
            .await?;

        Ok(())
    }

    async fn list_branches(&self) -> Result<Vec<BranchInfo>> {
        let path = format!("projects/{}/branches", self.project_id);
        let response: ListBranchesResponse = self
            .make_request(reqwest::Method::GET, &path, None::<&()>)
            .await?;

        let branches = response
            .branches
            .into_iter()
            .map(|branch| BranchInfo {
                name: branch.name,
                created_at: Some(branch.created_at),
                parent_branch: branch.parent_id,
                database_name: branch.id,
                state: Some("running".to_string()),
            })
            .collect();

        Ok(branches)
    }

    async fn branch_exists(&self, branch_name: &str) -> Result<bool> {
        let branches = self.list_branches().await?;
        Ok(branches.iter().any(|b| b.name == branch_name))
    }

    async fn switch_to_branch(&self, branch_name: &str) -> Result<BranchInfo> {
        let branches = self.list_branches().await?;
        branches
            .into_iter()
            .find(|b| b.name == branch_name)
            .ok_or_else(|| anyhow::anyhow!("Branch '{}' does not exist", branch_name))
    }

    async fn get_connection_info(&self, branch_name: &str) -> Result<ConnectionInfo> {
        let endpoint = self.get_branch_endpoint(branch_name).await?;

        let connection_string = if let Some(ref password) = endpoint.database_password {
            format!(
                "postgresql://{}:{}@{}/{}",
                endpoint.database_user, password, endpoint.database_host, endpoint.database_name
            )
        } else {
            format!(
                "postgresql://{}@{}/{}",
                endpoint.database_user, endpoint.database_host, endpoint.database_name
            )
        };

        Ok(ConnectionInfo {
            host: endpoint.database_host,
            port: 5432,
            database: endpoint.database_name,
            user: endpoint.database_user,
            password: endpoint.database_password,
            connection_string: Some(connection_string),
        })
    }

    async fn test_connection(&self) -> Result<()> {
        let _ = self.list_branches().await?;
        Ok(())
    }

    async fn doctor(&self) -> Result<DoctorReport> {
        let check = match self.test_connection().await {
            Ok(_) => DoctorCheck {
                name: "Neon API".to_string(),
                available: true,
                detail: "Connected to Neon API".to_string(),
            },
            Err(e) => DoctorCheck {
                name: "Neon API".to_string(),
                available: false,
                detail: format!("Failed: {}", e),
            },
        };
        Ok(DoctorReport {
            checks: vec![check],
        })
    }

    fn backend_name(&self) -> &'static str {
        "Neon"
    }

    fn supports_template_from_time(&self) -> bool {
        true
    }
}
