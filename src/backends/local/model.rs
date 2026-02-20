use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub image: String,
    pub storage_backend: StorageBackend,
    pub storage_config: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub parent_branch_id: Option<String>,
    pub state: BranchState,
    pub data_dir: String,
    pub container_name: String,
    pub port: u16,
    pub storage_metadata: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageBackend {
    Zfs,
    ApfsClone,
    Reflink,
    Copy,
}

impl StorageBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Zfs => "zfs",
            Self::ApfsClone => "apfs_clone",
            Self::Reflink => "reflink",
            Self::Copy => "copy",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "zfs" => Some(Self::Zfs),
            "apfs_clone" => Some(Self::ApfsClone),
            "reflink" => Some(Self::Reflink),
            "copy" => Some(Self::Copy),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BranchState {
    Provisioning,
    Stopped,
    Running,
    Failed,
}

impl BranchState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Stopped => "stopped",
            Self::Running => "running",
            Self::Failed => "failed",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "provisioning" => Some(Self::Provisioning),
            "stopped" => Some(Self::Stopped),
            "running" => Some(Self::Running),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

pub fn now_epoch_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    duration.as_millis() as i64
}
