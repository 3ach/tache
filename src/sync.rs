//! Reconcile Todoist labels with the computed DAG frontier.
//!
//! Scoping: only projects that opt in — `#dag` anywhere in the project
//! description — get DAG treatment. Tasks everywhere else are left alone,
//! except that stale `next`/`blocked` labels are stripped (so un-marking
//! a project cleans it up).

use anyhow::Result;
use std::collections::{HashMap, HashSet};

use crate::dag::{Dag, LABEL_BLOCKED, LABEL_NEXT};
use crate::todoist::{Client, Project, Task};

pub const DAG_MARKER: &str = "#dag";

#[derive(Debug, Default)]
pub struct Report {
    pub projects: usize,
    pub scoped: usize,
    pub next: usize,
    pub blocked: usize,
    pub relabeled: usize,
    pub stripped: usize,
    pub unresolved: Vec<(String, String)>,
    pub ambiguous: Vec<(String, String)>,
    pub cycles: usize,
}

pub struct Scope {
    pub tasks: Vec<Task>,
    pub enabled_projects: HashSet<String>,
}

impl Scope {
    pub async fn fetch(client: &Client) -> Result<Self> {
        let projects = client.active_projects().await?;
        let tasks = client.active_tasks().await?;
        Ok(Self {
            tasks,
            enabled_projects: enabled_ids(&projects),
        })
    }

    pub fn enabled_tasks(&self) -> Vec<Task> {
        self.tasks
            .iter()
            .filter(|t| self.enabled_projects.contains(&t.project_id))
            .cloned()
            .collect()
    }
}

pub fn enabled_ids(projects: &[Project]) -> HashSet<String> {
    projects
        .iter()
        .filter(|p| p.description.to_lowercase().contains(DAG_MARKER))
        .map(|p| p.id.clone())
        .collect()
}

/// Fetch projects and tasks, classify tasks in `#dag` projects, and fix
/// any task whose `next`/`blocked` labels disagree. All other labels on a
/// task are preserved untouched.
pub async fn reconcile(client: &Client) -> Result<Report> {
    let scope = Scope::fetch(client).await?;
    let scoped = scope.enabled_tasks();
    let dag = Dag::build(&scoped);
    let classes = dag.classify(&scoped);

    let mut report = Report {
        projects: scope.enabled_projects.len(),
        scoped: scoped.len(),
        unresolved: dag.unresolved.clone(),
        ambiguous: dag.ambiguous.clone(),
        cycles: dag.cycle_members().len(),
        ..Default::default()
    };

    for task in &scope.tasks {
        let desired = match classes.get(&task.id) {
            Some(&class) => {
                match class {
                    LABEL_NEXT => report.next += 1,
                    _ => report.blocked += 1,
                }
                desired_labels(task, Some(class))
            }
            None => desired_labels(task, None),
        };
        if desired != task.labels {
            client.set_labels(&task.id, &desired).await?;
            match classes.get(&task.id) {
                Some(&class) => {
                    report.relabeled += 1;
                    tracing::info!(task = %task.content, label = class, "relabeled");
                }
                None => {
                    report.stripped += 1;
                    tracing::info!(task = %task.content, "stripped");
                }
            }
        }
    }
    Ok(report)
}

/// A task's labels with ours normalized: managed labels removed, then the
/// classification (if the task is in scope) appended.
fn desired_labels(task: &Task, class: Option<&str>) -> Vec<String> {
    let mut labels: Vec<String> = task
        .labels
        .iter()
        .filter(|l| l.as_str() != LABEL_NEXT && l.as_str() != LABEL_BLOCKED)
        .cloned()
        .collect();
    if let Some(class) = class {
        labels.push(class.to_string());
    }
    labels
}

/// Human-readable frontier listing for the CLI.
pub fn format_frontier(tasks: &[Task], classes: &HashMap<String, &'static str>) -> String {
    let mut out = String::new();
    for t in tasks {
        if classes.get(&t.id).copied() == Some(LABEL_NEXT) {
            let due = t
                .due
                .as_ref()
                .map(|d| format!("  (due {})", d.date))
                .unwrap_or_default();
            out.push_str(&format!("• {}{}\n", t.content, due));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(id: &str, description: &str) -> Project {
        Project {
            id: id.into(),
            name: String::new(),
            description: description.into(),
        }
    }

    #[test]
    fn marker_in_description_enables_project() {
        let projects = vec![
            project("p1", "#dag"),
            project("p2", "planning stuff #DAG here"),
            project("p3", ""),
            project("p4", "dag"), // no marker: needs the #
        ];
        let enabled = enabled_ids(&projects);
        assert!(enabled.contains("p1"));
        assert!(enabled.contains("p2"));
        assert!(!enabled.contains("p3"));
        assert!(!enabled.contains("p4"));
    }

    #[test]
    fn out_of_scope_tasks_get_labels_stripped() {
        let task = Task {
            id: "1".into(),
            content: "t".into(),
            description: String::new(),
            labels: vec!["errand".into(), "next".into()],
            project_id: "p3".into(),
            due: None,
        };
        assert_eq!(desired_labels(&task, None), vec!["errand".to_string()]);
        assert_eq!(
            desired_labels(&task, Some("blocked")),
            vec!["errand".to_string(), "blocked".to_string()]
        );
    }
}
