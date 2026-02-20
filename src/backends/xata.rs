use super::{BranchInfo, ConnectionInfo, DatabaseBranchingBackend, DoctorCheck, DoctorReport};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};

const DEFAULT_BASE_URL: &str = "https://api.xata.tech";

#[derive(Debug, Clone)]
pub struct XataBackend {
    client: Client,
    api_key: String,
    base_url: String,
    organization_id: String,
    project_id: String,
}

#[derive(Debug, Deserialize)]
struct XataBranch {
    id: String,
    name: String,
    #[serde(rename = "createdAt")]
    created_at: Option<DateTime<Utc>>,
    #[serde(rename = "parentID")]
    #[allow(dead_code)]
    parent_id: Option<String>,
    #[allow(dead_code)]
    region: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListBranchesResponse {
    branches: Vec<XataBranch>,
}

#[derive(Debug, Serialize)]
struct CreateBranchRequest {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "parentID")]
    parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BranchCredentials {
    username: String,
    password: String,
    host: Option<String>,
    port: Option<u16>,
    database: Option<String>,
}

impl XataBackend {
    pub fn new(
        api_key: String,
        organization_id: String,
        project_id: String,
        base_url: Option<String>,
    ) -> Result<Self> {
        let client = Client::new();

        Ok(Self {
            client,
            api_key,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            organization_id,
            project_id,
        })
    }

    fn branches_url(&self) -> String {
        format!(
            "{}/organizations/{}/projects/{}/branches",
            self.base_url, self.organization_id, self.project_id
        )
    }

    fn branch_url(&self, branch_id: &str) -> String {
        format!("{}/{}", self.branches_url(), branch_id)
    }

    async fn api_request<T: for<'de> Deserialize<'de>>(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<&impl Serialize>,
    ) -> Result<T> {
        let mut request = self
            .client
            .request(method, url)
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
                "Xata API request failed with status {}: {}",
                status,
                error_text
            );
        }

        response
            .json()
            .await
            .with_context(|| "Failed to parse JSON response from Xata API")
    }

    async fn api_request_no_body(&self, method: reqwest::Method, url: &str) -> Result<()> {
        let request = self
            .client
            .request(method, url)
            .header("Authorization", format!("Bearer {}", self.api_key));

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
                "Xata API request failed with status {}: {}",
                status,
                error_text
            );
        }

        Ok(())
    }

    async fn fetch_branches(&self) -> Result<Vec<XataBranch>> {
        let response: ListBranchesResponse = self
            .api_request(reqwest::Method::GET, &self.branches_url(), None::<&()>)
            .await?;
        Ok(response.branches)
    }

    async fn find_branch_by_name(&self, branch_name: &str) -> Result<Option<XataBranch>> {
        let normalized = Self::normalize_branch_name(branch_name);
        let branches = self.fetch_branches().await?;
        Ok(branches.into_iter().find(|b| b.name == normalized))
    }

    fn normalize_branch_name(branch_name: &str) -> String {
        branch_name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
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
impl DatabaseBranchingBackend for XataBackend {
    async fn create_branch(
        &self,
        branch_name: &str,
        from_branch: Option<&str>,
    ) -> Result<BranchInfo> {
        let normalized_name = Self::normalize_branch_name(branch_name);

        // Resolve parent branch ID if a from_branch name is given
        let parent_id = if let Some(from_name) = from_branch {
            let parent = self
                .find_branch_by_name(from_name)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Parent branch '{}' not found", from_name))?;
            Some(parent.id)
        } else {
            None
        };

        let request = CreateBranchRequest {
            name: normalized_name.clone(),
            parent_id,
        };

        let branch: XataBranch = self
            .api_request(reqwest::Method::POST, &self.branches_url(), Some(&request))
            .await?;

        Ok(BranchInfo {
            name: branch.name,
            created_at: branch.created_at,
            parent_branch: from_branch.map(|s| s.to_string()),
            database_name: self.project_id.clone(),
            state: Some("running".to_string()),
        })
    }

    async fn delete_branch(&self, branch_name: &str) -> Result<()> {
        let branch = self
            .find_branch_by_name(branch_name)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", branch_name))?;

        self.api_request_no_body(reqwest::Method::DELETE, &self.branch_url(&branch.id))
            .await
    }

    async fn list_branches(&self) -> Result<Vec<BranchInfo>> {
        let branches = self.fetch_branches().await?;

        Ok(branches
            .into_iter()
            .map(|branch| BranchInfo {
                name: branch.name,
                created_at: branch.created_at,
                parent_branch: None,
                database_name: self.project_id.clone(),
                state: Some("running".to_string()),
            })
            .collect())
    }

    async fn branch_exists(&self, branch_name: &str) -> Result<bool> {
        Ok(self.find_branch_by_name(branch_name).await?.is_some())
    }

    async fn switch_to_branch(&self, branch_name: &str) -> Result<BranchInfo> {
        let normalized_name = Self::normalize_branch_name(branch_name);
        let branches = self.list_branches().await?;
        branches
            .into_iter()
            .find(|b| b.name == normalized_name)
            .ok_or_else(|| anyhow::anyhow!("Branch '{}' does not exist", branch_name))
    }

    async fn get_connection_info(&self, branch_name: &str) -> Result<ConnectionInfo> {
        let branch = self
            .find_branch_by_name(branch_name)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", branch_name))?;

        let creds_url = format!("{}/credentials", self.branch_url(&branch.id));
        let creds: BranchCredentials = self
            .api_request(reqwest::Method::GET, &creds_url, None::<&()>)
            .await?;

        let host = creds.host.unwrap_or_else(|| "localhost".to_string());
        let port = creds.port.unwrap_or(5432);
        let database = creds.database.unwrap_or_else(|| branch.name.clone());

        let connection_string = format!(
            "postgresql://{}:{}@{}:{}/{}",
            creds.username, creds.password, host, port, database
        );

        Ok(ConnectionInfo {
            host,
            port,
            database,
            user: creds.username,
            password: Some(creds.password),
            connection_string: Some(connection_string),
        })
    }

    async fn test_connection(&self) -> Result<()> {
        let _ = self.fetch_branches().await?;
        Ok(())
    }

    async fn doctor(&self) -> Result<DoctorReport> {
        let check = match self.test_connection().await {
            Ok(_) => DoctorCheck {
                name: "Xata API".to_string(),
                available: true,
                detail: "Connected to Xata API".to_string(),
            },
            Err(e) => DoctorCheck {
                name: "Xata API".to_string(),
                available: false,
                detail: format!("Failed: {}", e),
            },
        };
        Ok(DoctorReport {
            checks: vec![check],
        })
    }

    fn backend_name(&self) -> &'static str {
        "Xata"
    }

    fn max_branch_name_length(&self) -> usize {
        255
    }
}
