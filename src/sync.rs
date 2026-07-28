//! Reconcile Todoist labels with the computed DAG frontier.

use anyhow::Result;
use std::collections::HashMap;

use crate::dag::{Dag, LABEL_BLOCKED, LABEL_NEXT};
use crate::todoist::{Client, Task};

#[derive(Debug, Default)]
pub struct Report {
    pub total: usize,
    pub next: usize,
    pub blocked: usize,
    pub relabeled: usize,
    pub unresolved: Vec<(String, String)>,
    pub ambiguous: Vec<(String, String)>,
    pub cycles: usize,
}

/// Fetch all active tasks, classify them, and update any task whose
/// `next`/`blocked` labels disagree with the classification. All other
/// labels on a task are preserved untouched.
pub async fn reconcile(client: &Client) -> Result<Report> {
    let tasks = client.active_tasks().await?;
    let dag = Dag::build(&tasks);
    let classes = dag.classify(&tasks);

    let mut report = Report {
        total: tasks.len(),
        unresolved: dag.unresolved.clone(),
        ambiguous: dag.ambiguous.clone(),
        cycles: dag.cycle_members().len(),
        ..Default::default()
    };

    for task in &tasks {
        let class = classes[&task.id];
        match class {
            LABEL_NEXT => report.next += 1,
            _ => report.blocked += 1,
        }
        let desired = desired_labels(task, class);
        if desired != task.labels {
            client.set_labels(&task.id, &desired).await?;
            report.relabeled += 1;
            tracing::info!(task = %task.content, label = class, "relabeled");
        }
    }
    Ok(report)
}

fn desired_labels(task: &Task, class: &str) -> Vec<String> {
    let mut labels: Vec<String> = task
        .labels
        .iter()
        .filter(|l| l.as_str() != LABEL_NEXT && l.as_str() != LABEL_BLOCKED)
        .cloned()
        .collect();
    labels.push(class.to_string());
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
