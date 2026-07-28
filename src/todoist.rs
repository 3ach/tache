//! Minimal Todoist unified API (v1) client — just the calls tache needs.

use anyhow::{Context, Result};
use serde::Deserialize;

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

pub struct Client {
    http: reqwest::Client,
    token: String,
}

impl Client {
    pub fn new(token: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            token,
        }
    }

    pub fn from_env() -> Result<Self> {
        let token = std::env::var("TODOIST_API_TOKEN")
            .context("TODOIST_API_TOKEN is not set (see .env.example)")?;
        Ok(Self::new(token))
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
                .bearer_auth(&self.token)
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

    pub async fn set_labels(&self, task_id: &str, labels: &[String]) -> Result<()> {
        self.http
            .post(format!("{BASE}/tasks/{task_id}"))
            .bearer_auth(&self.token)
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
            .bearer_auth(&self.token)
            .json(&serde_json::json!({ "description": description }))
            .send()
            .await?
            .error_for_status()
            .with_context(|| format!("updating description on task {task_id}"))?;
        Ok(())
    }
}
