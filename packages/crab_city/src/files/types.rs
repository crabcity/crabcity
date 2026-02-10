use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    #[serde(rename = "isDirectory")]
    pub is_directory: bool,
    #[serde(rename = "isSymlink")]
    pub is_symlink: bool,
    #[serde(rename = "symlinkTarget", skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,
    pub size: Option<u64>,
    #[serde(rename = "modifiedAt")]
    pub modified_at: Option<String>,
}

#[derive(Serialize)]
pub struct DirectoryListing {
    pub path: String,
    pub entries: Vec<FileEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Deserialize)]
pub struct FilePathQuery {
    pub path: String,
}

#[derive(Deserialize)]
pub struct FileSearchQuery {
    pub q: String,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
}

fn default_search_limit() -> usize {
    100
}

#[derive(Serialize)]
pub struct FileSearchResult {
    pub name: String,
    pub path: String,
    #[serde(rename = "relativePath")]
    pub relative_path: String,
    #[serde(rename = "isDirectory")]
    pub is_directory: bool,
    pub score: i32,
}

#[derive(Serialize)]
pub struct FileSearchResponse {
    pub query: String,
    pub results: Vec<FileSearchResult>,
    pub truncated: bool,
}
