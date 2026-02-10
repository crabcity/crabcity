use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct GitCommit {
    pub hash: String,
    #[serde(rename = "shortHash")]
    pub short_hash: String,
    #[serde(rename = "authorName")]
    pub author_name: String,
    #[serde(rename = "authorEmail")]
    pub author_email: String,
    pub date: i64,
    pub message: String,
    pub body: String,
    pub refs: Vec<String>,
}

#[derive(Serialize)]
pub struct GitLogResponse {
    pub commits: Vec<GitCommit>,
    #[serde(rename = "hasMore")]
    pub has_more: bool,
}

#[derive(Serialize)]
pub struct GitBranch {
    pub name: String,
    pub current: bool,
    /// True for remote-tracking branches (refs/remotes/...)
    pub remote: bool,
    #[serde(rename = "lastCommitHash")]
    pub last_commit_hash: String,
    #[serde(rename = "lastCommitDate")]
    pub last_commit_date: i64,
    #[serde(rename = "lastCommitMessage")]
    pub last_commit_message: String,
    pub upstream: Option<String>,
    pub ahead: i64,
    pub behind: i64,
}

#[derive(Serialize)]
pub struct InstanceBranchInfo {
    pub instance_id: String,
    pub instance_name: String,
    pub branch: String,
}

#[derive(Serialize)]
pub struct GitBranchesResponse {
    pub branches: Vec<GitBranch>,
    pub current: String,
    /// The remote's default branch (e.g. "main"), resolved from origin/HEAD.
    #[serde(rename = "defaultBranch", skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    #[serde(rename = "instanceBranches")]
    pub instance_branches: Vec<InstanceBranchInfo>,
}

#[derive(Serialize)]
pub struct GitFileStatus {
    pub path: String,
    pub status: String,
    #[serde(rename = "oldPath", skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

#[derive(Serialize)]
pub struct GitStatusResponse {
    pub branch: String,
    pub staged: Vec<GitFileStatus>,
    pub unstaged: Vec<GitFileStatus>,
    pub untracked: Vec<GitFileStatus>,
    #[serde(rename = "aheadBehind")]
    pub ahead_behind: Option<(i64, i64)>,
}

#[derive(Serialize)]
pub struct InlineHighlight {
    pub start: usize,
    pub end: usize,
}

#[derive(Serialize)]
pub struct GitDiffLine {
    #[serde(rename = "type")]
    pub line_type: String,
    pub content: String,
    #[serde(rename = "oldNum", skip_serializing_if = "Option::is_none")]
    pub old_num: Option<i64>,
    #[serde(rename = "newNum", skip_serializing_if = "Option::is_none")]
    pub new_num: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highlights: Option<Vec<InlineHighlight>>,
}

#[derive(Serialize)]
pub struct GitDiffHunk {
    pub header: String,
    pub lines: Vec<GitDiffLine>,
}

#[derive(Serialize)]
pub struct GitDiffFile {
    pub path: String,
    #[serde(rename = "oldPath", skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: String,
    pub additions: i64,
    pub deletions: i64,
    pub hunks: Vec<GitDiffHunk>,
}

#[derive(Serialize)]
pub struct GitDiffStats {
    pub additions: i64,
    pub deletions: i64,
    #[serde(rename = "filesChanged")]
    pub files_changed: i64,
}

#[derive(Serialize)]
pub struct GitDiffResponse {
    pub files: Vec<GitDiffFile>,
    pub stats: GitDiffStats,
    /// Which engine actually produced the diff: "structural", "patience", or "standard".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
}

#[derive(Deserialize)]
pub struct GitLogQuery {
    #[serde(default = "default_git_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    pub branch: Option<String>,
}

fn default_git_limit() -> i64 {
    50
}

#[derive(Deserialize)]
pub struct GitDiffQuery {
    pub commit: Option<String>,
    pub path: Option<String>,
    /// Diff engine: "structural", "patience", or "standard".
    pub engine: Option<String>,
    /// Base ref for branch-to-branch comparison (e.g. "main")
    pub base: Option<String>,
    /// Head ref for branch-to-branch comparison (e.g. "feature-branch")
    pub head: Option<String>,
    /// "twodot" or "threedot" (default). Controls `..` vs `...` range syntax.
    pub diff_mode: Option<String>,
    /// When true, return only file stats (no hunks) via --numstat
    #[serde(default)]
    pub stat_only: bool,
}
