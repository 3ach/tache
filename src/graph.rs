//! Read-only HTML view of the dependency graph (GET /graph).
//!
//! Renders every `#dag` project's active tasks as its own Mermaid
//! flowchart behind a vanilla-JS tab bar — one tab per project, nodes
//! colored by frontier status, edges pointing prerequisite → dependent.
//! Cross-project edges show the external endpoint as a dashed "ghost"
//! node labeled with its name and project. Charts render lazily on first
//! tab activation (Mermaid can't lay out inside display:none) and the
//! SVG is CSS-capped to fit the viewport. The server emits static HTML;
//! Mermaid runs client-side from a CDN. Gated by TACHE_GRAPH_KEY since
//! task contents are personal.

use std::collections::{HashMap, HashSet};

use crate::dag::{Dag, LABEL_NEXT};
use crate::todoist::{Project, Task};

pub fn page(
    projects: &[Project],
    tasks: &[Task],
    dag: &Dag,
    classes: &HashMap<String, &'static str>,
) -> String {
    let mut tabs = String::new();
    let mut panes = String::new();
    for p in projects {
        let Some(chart) = project_chart(p, projects, tasks, dag, classes) else {
            continue;
        };
        let title = escape(display_name(p));
        tabs.push_str(&format!(
            "<button class=\"tab\" data-tab=\"{id}\">{title}</button>",
            id = escape(&p.id),
        ));
        panes.push_str(&format!(
            "<div class=\"pane\" id=\"pane-{id}\"><pre class=\"mermaid\">\n{chart}</pre></div>\n",
            id = escape(&p.id),
        ));
    }
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>tache graph</title>
<style>
  body {{ margin: 0; padding: 0.75rem 1rem; background: #fafafa; font-family: system-ui, sans-serif; }}
  h1 {{ font-size: 1.1rem; font-weight: 600; margin: 0 0 0.6rem; }}
  #tabs {{ display: flex; flex-wrap: wrap; gap: 0.4rem; margin-bottom: 0.6rem; }}
  .tab {{ font: inherit; padding: 0.3rem 0.9rem; border: 1px solid #cfd8dc; border-radius: 999px;
         background: #fff; color: #455a64; cursor: pointer; }}
  .tab.active {{ background: #2e7d32; border-color: #2e7d32; color: #fff; }}
  .pane {{ display: none; }}
  .pane.active {{ display: block; }}
  .pane pre.mermaid {{ margin: 0; text-align: center; }}
  /* Hide raw Mermaid source until it has been rendered into an SVG. */
  pre.mermaid:not([data-processed]) {{ visibility: hidden; }}
  /* Fit-to-screen: scale down to the viewport (minus header + tab bar),
     preserving aspect ratio; never scale up past natural size. */
  .pane svg {{ display: block; margin: 0 auto; width: auto; height: auto;
              max-width: 100%; max-height: calc(100vh - 6.5rem); }}
</style>
</head>
<body>
<h1>tache — dependency graph</h1>
<nav id="tabs">{tabs}</nav>
{panes}<script type="module">
import mermaid from "https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs";
// useMaxWidth:false keeps mermaid from pinning inline width styles so the
// CSS max-width/max-height rules above control the final size.
mermaid.initialize({{ startOnLoad: false, theme: "neutral", flowchart: {{ useMaxWidth: false }} }});
const rendered = new Set();
async function activate(id) {{
  for (const b of document.querySelectorAll(".tab"))
    b.classList.toggle("active", b.dataset.tab === id);
  for (const p of document.querySelectorAll(".pane"))
    p.classList.toggle("active", p.id === "pane-" + id);
  if (!rendered.has(id)) {{
    rendered.add(id);
    const el = document.querySelector('[id="pane-' + id + '"] .mermaid');
    if (el) await mermaid.run({{ nodes: [el] }});
  }}
}}
for (const b of document.querySelectorAll(".tab"))
  b.addEventListener("click", () => activate(b.dataset.tab));
const first = document.querySelector(".tab");
if (first) activate(first.dataset.tab);
</script>
</body>
</html>
"#
    )
}

fn display_name(p: &Project) -> &str {
    if p.name.is_empty() { &p.id } else { &p.name }
}

/// Mermaid flowchart for one project, or None if it has no tasks.
///
/// Cross-project edges keep their prereq → dependent direction but the
/// external endpoint becomes a dashed ghost node labeled with the task's
/// name and project, so each tab stays self-contained.
fn project_chart(
    project: &Project,
    projects: &[Project],
    tasks: &[Task],
    dag: &Dag,
    classes: &HashMap<String, &'static str>,
) -> Option<String> {
    let members: Vec<&Task> = tasks.iter().filter(|t| t.project_id == project.id).collect();
    if members.is_empty() {
        return None;
    }
    let by_id: HashMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    let project_names: HashMap<&str, &str> = projects
        .iter()
        .map(|p| (p.id.as_str(), display_name(p)))
        .collect();

    let mut out = String::from("flowchart TD\n");
    out.push_str("  classDef next fill:#c8e6c9,stroke:#2e7d32,color:#1b5e20\n");
    out.push_str("  classDef blocked fill:#eceff1,stroke:#b0bec5,color:#78909c\n");
    out.push_str("  classDef ghost fill:#fafafa,stroke:#90a4ae,stroke-dasharray:5 4,color:#90a4ae\n");

    let member_ids: HashSet<&str> = members.iter().map(|t| t.id.as_str()).collect();
    for t in &members {
        let class = match classes.get(&t.id).copied() {
            Some(LABEL_NEXT) => "next",
            _ => "blocked",
        };
        out.push_str(&format!(
            "  T{}[\"{}\"]:::{}\n",
            t.id,
            escape(&t.content),
            class
        ));
    }

    // HashMap order is unstable; sort so output (and tests) are deterministic
    let mut edges: Vec<(&str, &str)> = dag
        .prereqs
        .iter()
        .flat_map(|(task, ps)| ps.iter().map(move |p| (p.as_str(), task.as_str())))
        .collect();
    edges.sort_unstable();

    let mut ghosts: HashSet<&str> = HashSet::new();
    for (prereq, task) in edges {
        let p_in = member_ids.contains(prereq);
        let t_in = member_ids.contains(task);
        match (p_in, t_in) {
            (true, true) => out.push_str(&format!("  T{prereq} --> T{task}\n")),
            // External dependent: ghost it in the prerequisite's tab.
            (true, false) => {
                let Some(label) = ghost_label(task, &by_id, &project_names) else {
                    continue;
                };
                if ghosts.insert(task) {
                    out.push_str(&format!("  G{task}[\"{label}\"]:::ghost\n"));
                }
                out.push_str(&format!("  T{prereq} -.-> G{task}\n"));
            }
            // External prerequisite: ghost it in the dependent's tab.
            (false, true) => {
                let Some(label) = ghost_label(prereq, &by_id, &project_names) else {
                    continue;
                };
                if ghosts.insert(prereq) {
                    out.push_str(&format!("  G{prereq}[\"{label}\"]:::ghost\n"));
                }
                out.push_str(&format!("  G{prereq} -.-> T{task}\n"));
            }
            (false, false) => {}
        }
    }
    Some(out)
}

/// "Task name (Project name)" for a ghost node, escaped; None if the task
/// is unknown (its edge is dropped rather than drawn dangling).
fn ghost_label(
    id: &str,
    by_id: &HashMap<&str, &Task>,
    project_names: &HashMap<&str, &str>,
) -> Option<String> {
    let t = by_id.get(id)?;
    let project = project_names
        .get(t.project_id.as_str())
        .copied()
        .unwrap_or(t.project_id.as_str());
    Some(format!("{} ({})", escape(&t.content), escape(project)))
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

    fn project(id: &str, name: &str) -> Project {
        Project {
            id: id.into(),
            name: name.into(),
            description: "#dag".into(),
        }
    }

    #[test]
    fn renders_nodes_and_edges_per_project() {
        let projects = vec![project("p1", "Shop")];
        let tasks = vec![
            task("1", "buy lumber", "", "p1"),
            task("2", "sand \"rough\" boards", "after: buy lumber", "p1"),
        ];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        let chart = project_chart(&projects[0], &projects, &tasks, &dag, &classes).unwrap();
        assert!(chart.starts_with("flowchart TD"));
        assert!(chart.contains("T1[\"buy lumber\"]:::next"));
        assert!(chart.contains("T2[\"sand &quot;rough&quot; boards\"]:::blocked"));
        assert!(chart.contains("T1 --> T2"));
        assert!(!chart.contains("subgraph"));
    }

    #[test]
    fn skips_projects_without_tasks() {
        let projects = vec![project("empty", "Empty")];
        assert!(
            project_chart(&projects[0], &projects, &[], &Dag::default(), &HashMap::new()).is_none()
        );
    }

    #[test]
    fn cross_project_edges_become_ghost_nodes_in_both_tabs() {
        let projects = vec![project("p1", "Shop"), project("p2", "House")];
        let tasks = vec![
            task("1", "build cabinet", "", "p1"),
            task("2", "install cabinet", "after: 1", "p2"),
        ];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);

        // Prereq's tab: dependent appears as a ghost.
        let shop = project_chart(&projects[0], &projects, &tasks, &dag, &classes).unwrap();
        assert!(shop.contains("T1[\"build cabinet\"]:::next"));
        assert!(shop.contains("G2[\"install cabinet (House)\"]:::ghost"));
        assert!(shop.contains("T1 -.-> G2"));
        assert!(!shop.contains("T2[\"install cabinet\"]"));

        // Dependent's tab: prerequisite appears as a ghost.
        let house = project_chart(&projects[1], &projects, &tasks, &dag, &classes).unwrap();
        assert!(house.contains("T2[\"install cabinet\"]:::blocked"));
        assert!(house.contains("G1[\"build cabinet (Shop)\"]:::ghost"));
        assert!(house.contains("G1 -.-> T2"));
        assert!(!house.contains("T1[\"build cabinet\"]"));
    }

    #[test]
    fn ghost_nodes_are_deduplicated() {
        let projects = vec![project("p1", "Shop"), project("p2", "House")];
        let tasks = vec![
            task("1", "build cabinet", "", "p1"),
            task("2", "build shelf", "", "p1"),
            task("3", "install everything", "after: 1\nafter: 2", "p2"),
        ];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        let shop = project_chart(&projects[0], &projects, &tasks, &dag, &classes).unwrap();
        assert_eq!(shop.matches("G3[").count(), 1);
        assert!(shop.contains("T1 -.-> G3"));
        assert!(shop.contains("T2 -.-> G3"));
    }

    #[test]
    fn page_has_one_tab_and_pane_per_nonempty_project() {
        let projects = vec![
            project("p1", "Shop"),
            project("p2", "House"),
            project("p3", "Empty"),
        ];
        let tasks = vec![
            task("1", "build cabinet", "", "p1"),
            task("2", "install cabinet", "after: 1", "p2"),
        ];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        let html = page(&projects, &tasks, &dag, &classes);
        assert!(html.contains("<button class=\"tab\" data-tab=\"p1\">Shop</button>"));
        assert!(html.contains("<button class=\"tab\" data-tab=\"p2\">House</button>"));
        assert!(!html.contains("Empty"));
        assert!(html.contains("id=\"pane-p1\""));
        assert!(html.contains("id=\"pane-p2\""));
        assert_eq!(html.matches("flowchart TD").count(), 2);
        assert!(html.contains("startOnLoad: false"));
        assert!(html.contains("mermaid.run"));
    }

    #[test]
    fn page_escapes_project_names() {
        let projects = vec![project("p1", "A <b>&\"quoted\"</b>")];
        let tasks = vec![task("1", "t", "", "p1")];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        let html = page(&projects, &tasks, &dag, &classes);
        assert!(html.contains("A &lt;b&gt;&amp;&quot;quoted&quot;&lt;/b&gt;"));
        assert!(!html.contains("<b>&\"quoted\"</b>"));
    }
}
