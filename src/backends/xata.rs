use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use super::{DatabaseBranchingBackend, BranchInfo, ConnectionInfo};

#[derive(Debug, Clone)]
pub struct XataBackend {
    client: Client,
    organization_id: String,
    project_id: String,
    api_key: String,
    base_url: String,
}

#[derive(Debug, Serialize)]
struct CreateBranchRequest {
    name: String,
    #[serde(rename = "parentID", skip_serializing_if = "Option::is_none")]
    parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct XataBranch {
    id: String,
    name: String,
    #[serde(rename = "createdAt")]
    created_at: DateTime<Utc>,
    #[serde(rename = "parentID", default)]
    parent_id: Option<String>,
    #[serde(rename = "databaseName")]
    database_name: String,
    #[serde(rename = "connectionString", default)]
    connection_string: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListBranchesResponse {
    branches: Vec<XataBranch>,
}

#[derive(Debug, Deserialize)]
struct CreateBranchResponse {
    branch: XataBranch,
}

#[derive(Debug, Deserialize)]
struct BranchDetailsResponse {
    branch: XataBranch,
    #[serde(rename = "connectionDetails")]
    connection_details: XataConnectionDetails,
}

#[derive(Debug, Deserialize)]
struct XataConnectionDetails {
    host: String,
    port: u16,
    database: String,
    user: String,
    password: String,
    #[serde(rename = "connectionString")]
    connection_string: String,
}

impl XataBackend {
    pub fn new(organization_id: String, project_id: String, api_key: String) -> Result<Self> {
        let client = Client::new();
        let base_url = "https://api.xata.io".to_string();
        
        Ok(Self {
            client,
            organization_id,
            project_id,
            api_key,
            base_url,
        })
    }

    async fn make_request<T: for<'de> Deserialize<'de>>(&self, method: reqwest::Method, path: &str, body: Option<&impl Serialize>) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let mut request = self.client.request(method, &url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json");

        if let Some(body) = body {
            request = request.json(body);
        }

        let response = request.send().await
            .with_context(|| format!("Failed to send request to {}", url))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            anyhow::bail!("Xata API request failed with status {}: {}", status, error_text);
        }

        response.json().await
            .with_context(|| "Failed to parse JSON response from Xata API")
    }

    async fn get_branch_details(&self, branch_name: &str) -> Result<BranchDetailsResponse> {
        let path = format!("/organizations/{}/projects/{}/branches/{}", 
            self.organization_id, 
            self.project_id,
            branch_name
        );
        self.make_request(reqwest::Method::GET, &path, None::<&()>).await
    }

    fn normalize_branch_name(branch_name: &str) -> String {
        // Xata has specific naming requirements
        branch_name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .to_string()
    }
}

#[async_trait]
impl DatabaseBranchingBackend for XataBackend {
    async fn create_branch(&self, branch_name: &str, from_branch: Option<&str>) -> Result<BranchInfo> {
        let normalized_name = Self::normalize_branch_name(branch_name);
        
        let request = CreateBranchRequest {
            name: normalized_name.clone(),
            parent_id: from_branch.map(|s| s.to_string()),
        };

        let path = format!("/organizations/{}/projects/{}/branches", 
            self.organization_id, 
            self.project_id
        );
        let response: CreateBranchResponse = self.make_request(reqwest::Method::POST, &path, Some(&request)).await?;

        Ok(BranchInfo {
            name: response.branch.name,
            created_at: Some(response.branch.created_at),
            parent_branch: response.branch.parent_id,
            database_name: response.branch.database_name,
        })
    }

    async fn delete_branch(&self, branch_name: &str) -> Result<()> {
        let normalized_name = Self::normalize_branch_name(branch_name);
        
        let path = format!("/organizations/{}/projects/{}/branches/{}", 
            self.organization_id, 
            self.project_id,
            normalized_name
        );
        let _: serde_json::Value = self.make_request(reqwest::Method::DELETE, &path, None::<&()>).await?;

        Ok(())
    }

    async fn list_branches(&self) -> Result<Vec<BranchInfo>> {
        let path = format!("/organizations/{}/projects/{}/branches", 
            self.organization_id, 
            self.project_id
        );
        let response: ListBranchesResponse = self.make_request(reqwest::Method::GET, &path, None::<&()>).await?;

        let branches = response.branches.into_iter()
            .map(|branch| BranchInfo {
                name: branch.name,
                created_at: Some(branch.created_at),
                parent_branch: branch.parent_id,
                database_name: branch.database_name,
            })
            .collect();

        Ok(branches)
    }

    async fn branch_exists(&self, branch_name: &str) -> Result<bool> {
        let normalized_name = Self::normalize_branch_name(branch_name);
        let branches = self.list_branches().await?;
        Ok(branches.iter().any(|b| b.name == normalized_name))
    }

    async fn switch_to_branch(&self, branch_name: &str) -> Result<BranchInfo> {
        let normalized_name = Self::normalize_branch_name(branch_name);
        
        // For Xata, switching is handled by post_commands via connection string
        // We just verify the branch exists
        let branches = self.list_branches().await?;
        branches.into_iter()
            .find(|b| b.name == normalized_name)
            .ok_or_else(|| anyhow::anyhow!("Branch '{}' does not exist", branch_name))
    }

    async fn get_connection_info(&self, branch_name: &str) -> Result<ConnectionInfo> {
        let normalized_name = Self::normalize_branch_name(branch_name);
        let details = self.get_branch_details(&normalized_name).await?;
        
        Ok(ConnectionInfo {
            host: details.connection_details.host,
            port: details.connection_details.port,
            database: details.connection_details.database,
            user: details.connection_details.user,
            password: Some(details.connection_details.password),
            connection_string: Some(details.connection_details.connection_string),
        })
    }

    async fn test_connection(&self) -> Result<()> {
        // Test by listing branches
        let _ = self.list_branches().await?;
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "Xata"
    }

    fn supports_cleanup(&self) -> bool {
        true
    }

    fn supports_template_from_time(&self) -> bool {
        false // Xata uses parent branches, not point-in-time
    }

    fn max_branch_name_length(&self) -> usize {
        255 // Xata supports longer names than PostgreSQL
    }
}