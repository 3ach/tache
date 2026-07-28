//! Minimal Todoist unified API (v1) client — just the calls tache needs.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::sync::RwLock;

const BASE: &str = "https://api.todoist.com/api/v1";

#[derive(Debug, Clone, Deserialize)]
pub struct Task {
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: Vec<String>,
    pub project_id: String,
    pub due: Option<Due>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Due {
    pub date: String,
}

#[derive(Debug, Deserialize)]
struct TasksPage {
    results: Vec<Task>,
    next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub id: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize)]
struct ProjectsPage {
    results: Vec<Project>,
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
}

/// The token sits behind a lock so the OAuth callback can swap in a fresh
/// one on a live server.
pub struct Client {
    http: reqwest::Client,
    token: RwLock<String>,
}

impl Client {
    pub fn new(token: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            token: RwLock::new(token),
        }
    }

    pub fn has_token(&self) -> bool {
        !self.token.read().unwrap().is_empty()
    }

    pub fn set_token(&self, token: String) {
        *self.token.write().unwrap() = token;
    }

    fn bearer(&self) -> String {
        self.token.read().unwrap().clone()
    }

    /// Exchange an OAuth authorization code for an access token. The
    /// returned token doubles as a normal API bearer token.
    pub async fn exchange_code(
        &self,
        client_id: &str,
        client_secret: &str,
        code: &str,
    ) -> Result<String> {
        let resp: TokenResponse = self
            .http
            .post("https://todoist.com/oauth/access_token")
            .form(&[
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("code", code),
            ])
            .send()
            .await?
            .error_for_status()
            .context("exchanging oauth code")?
            .json()
            .await?;
        Ok(resp.access_token)
    }

    /// All active tasks across all projects. Completed tasks are absent,
    /// which is exactly what the DAG logic relies on: a prerequisite id
    /// that no longer appears in this set is done.
    pub async fn active_tasks(&self) -> Result<Vec<Task>> {
        let mut tasks = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut req = self
                .http
                .get(format!("{BASE}/tasks"))
                .bearer_auth(self.bearer())
                .query(&[("limit", "200")]);
            if let Some(c) = &cursor {
                req = req.query(&[("cursor", c)]);
            }
            let page: TasksPage = req
                .send()
                .await?
                .error_for_status()
                .context("fetching tasks")?
                .json()
                .await?;
            tasks.extend(page.results);
            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Ok(tasks)
    }

    pub async fn active_projects(&self) -> Result<Vec<Project>> {
        let mut projects = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut req = self
                .http
                .get(format!("{BASE}/projects"))
                .bearer_auth(self.bearer())
                .query(&[("limit", "200")]);
            if let Some(c) = &cursor {
                req = req.query(&[("cursor", c)]);
            }
            let page: ProjectsPage = req
                .send()
                .await?
                .error_for_status()
                .context("fetching projects")?
                .json()
                .await?;
            projects.extend(page.results);
            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Ok(projects)
    }

    pub async fn set_labels(&self, task_id: &str, labels: &[String]) -> Result<()> {
        self.http
            .post(format!("{BASE}/tasks/{task_id}"))
            .bearer_auth(self.bearer())
            .json(&serde_json::json!({ "labels": labels }))
            .send()
            .await?
            .error_for_status()
            .with_context(|| format!("updating labels on task {task_id}"))?;
        Ok(())
    }

    pub async fn set_description(&self, task_id: &str, description: &str) -> Result<()> {
        self.http
            .post(format!("{BASE}/tasks/{task_id}"))
            .bearer_auth(self.bearer())
            .json(&serde_json::json!({ "description": description }))
            .send()
            .await?
            .error_for_status()
            .with_context(|| format!("updating description on task {task_id}"))?;
        Ok(())
    }
}
