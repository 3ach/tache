//! Read-only HTML view of the dependency graph (GET /graph).
//!
//! Renders every `#dag` project's active tasks as a Mermaid flowchart —
//! one subgraph per project, nodes colored by frontier status, edges
//! pointing prerequisite → dependent. The server emits static HTML;
//! Mermaid lays the graph out client-side from a CDN. Gated by
//! TACHE_GRAPH_KEY since task contents are personal.

use std::collections::HashMap;

use crate::dag::{Dag, LABEL_NEXT};
use crate::todoist::{Project, Task};

pub fn page(
    projects: &[Project],
    tasks: &[Task],
    dag: &Dag,
    classes: &HashMap<String, &'static str>,
) -> String {
    let chart = mermaid(projects, tasks, dag, classes);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="robots" content="noindex">
<title>tache graph</title>
<style>
  body {{ margin: 1.5rem; font-family: system-ui, sans-serif; background: #fafafa; }}
  h1 {{ font-size: 1.1rem; font-weight: 600; }}
</style>
</head>
<body>
<h1>tache — dependency graph</h1>
<pre class="mermaid">
{chart}
</pre>
<script type="module">
import mermaid from "https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs";
mermaid.initialize({{ startOnLoad: true, theme: "neutral", flowchart: {{ useMaxWidth: false }} }});
</script>
</body>
</html>
"#
    )
}

fn mermaid(
    projects: &[Project],
    tasks: &[Task],
    dag: &Dag,
    classes: &HashMap<String, &'static str>,
) -> String {
    let mut out = String::from("flowchart TD\n");
    out.push_str("  classDef next fill:#c8e6c9,stroke:#2e7d32,color:#1b5e20\n");
    out.push_str("  classDef blocked fill:#eceff1,stroke:#b0bec5,color:#78909c\n");

    for p in projects {
        let members: Vec<&Task> = tasks.iter().filter(|t| t.project_id == p.id).collect();
        if members.is_empty() {
            continue;
        }
        let title = if p.name.is_empty() { &p.id } else { &p.name };
        out.push_str(&format!("  subgraph P{}[\"{}\"]\n", p.id, escape(title)));
        for t in members {
            let class = match classes.get(&t.id).copied() {
                Some(LABEL_NEXT) => "next",
                _ => "blocked",
            };
            out.push_str(&format!(
                "    T{}[\"{}\"]:::{}\n",
                t.id,
                escape(&t.content),
                class
            ));
        }
        out.push_str("  end\n");
    }

    // HashMap order is unstable; sort so output (and tests) are deterministic
    let mut edges: Vec<(&str, &str)> = dag
        .prereqs
        .iter()
        .flat_map(|(task, ps)| ps.iter().map(move |p| (p.as_str(), task.as_str())))
        .collect();
    edges.sort_unstable();
    for (prereq, task) in edges {
        out.push_str(&format!("  T{prereq} --> T{task}\n"));
    }
    out
}

/// Escape for a Mermaid quoted node label inside a `<pre>` block: the
/// browser decodes HTML entities before Mermaid parses, so entities cover
/// both layers.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, content: &str, description: &str, project_id: &str) -> Task {
        Task {
            id: id.into(),
            content: content.into(),
            description: description.into(),
            labels: vec![],
            project_id: project_id.into(),
            due: None,
        }
    }

    #[test]
    fn renders_subgraphs_nodes_and_edges() {
        let projects = vec![Project {
            id: "p1".into(),
            name: "Shop".into(),
            description: "#dag".into(),
        }];
        let tasks = vec![
            task("1", "buy lumber", "", "p1"),
            task("2", "sand \"rough\" boards", "after: buy lumber", "p1"),
        ];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        let chart = mermaid(&projects, &tasks, &dag, &classes);
        assert!(chart.contains("subgraph Pp1[\"Shop\"]"));
        assert!(chart.contains("T1[\"buy lumber\"]:::next"));
        assert!(chart.contains("T2[\"sand &quot;rough&quot; boards\"]:::blocked"));
        assert!(chart.contains("T1 --> T2"));
    }

    #[test]
    fn skips_projects_without_tasks() {
        let projects = vec![Project {
            id: "empty".into(),
            name: "Empty".into(),
            description: "#dag".into(),
        }];
        let chart = mermaid(&projects, &[], &Dag::default(), &HashMap::new());
        assert!(!chart.contains("subgraph"));
    }
}
