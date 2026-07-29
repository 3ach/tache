//! Read-only HTML view of the dependency graph (GET /graph).
//!
//! Renders every `#dag` project's active tasks as its own left-to-right
//! Mermaid flowchart behind a vanilla-JS tab bar — one tab per project,
//! nodes colored by frontier status, edges pointing prerequisite →
//! dependent. Maximal linear runs of blocked tasks collapse into a
//! single "run" pill (first ⇢ last, count) so each chart reads as
//! frontier tasks + branch/merge points + collapsed chains.
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
///
/// Maximal linear runs of ≥2 all-blocked tasks — each with at most one
/// incoming and one outgoing edge, counting ghost edges, and no ghost
/// edge of its own — collapse into one subroutine-shaped `run` node
/// labeled "first ⇢ last (n tasks)". Frontier tasks and branch/merge
/// points always render individually; edges into a run's head and out of
/// its tail reconnect to the collapsed node.
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

    let member_ids: HashSet<&str> = members.iter().map(|t| t.id.as_str()).collect();

    // HashMap order is unstable; sort so output (and tests) are deterministic
    let mut edges: Vec<(&str, &str)> = dag
        .prereqs
        .iter()
        .flat_map(|(task, ps)| ps.iter().map(move |p| (p.as_str(), task.as_str())))
        .collect();
    edges.sort_unstable();

    // Degrees over every drawable edge touching a member — internal edges
    // plus ghost edges whose external endpoint resolves — so a task with an
    // external dependency/dependent is never mistaken for a linear link.
    let mut deg_in: HashMap<&str, usize> = HashMap::new();
    let mut deg_out: HashMap<&str, usize> = HashMap::new();
    let mut has_ghost: HashSet<&str> = HashSet::new();
    let mut internal: Vec<(&str, &str)> = Vec::new();
    for &(prereq, task) in &edges {
        match (member_ids.contains(prereq), member_ids.contains(task)) {
            (true, true) => {
                internal.push((prereq, task));
                *deg_out.entry(prereq).or_default() += 1;
                *deg_in.entry(task).or_default() += 1;
            }
            (true, false) => {
                if by_id.contains_key(task) {
                    *deg_out.entry(prereq).or_default() += 1;
                    has_ghost.insert(prereq);
                }
            }
            (false, true) => {
                if by_id.contains_key(prereq) {
                    *deg_in.entry(task).or_default() += 1;
                    has_ghost.insert(task);
                }
            }
            (false, false) => {}
        }
    }

    // A task may join a run only if it is blocked, purely linear (≤1 in,
    // ≤1 out counting ghost edges), and free of ghost edges itself.
    let is_next = |id: &str| matches!(classes.get(id).copied(), Some(LABEL_NEXT));
    let collapsible = |id: &str| {
        !is_next(id)
            && !has_ghost.contains(id)
            && deg_in.get(id).copied().unwrap_or(0) <= 1
            && deg_out.get(id).copied().unwrap_or(0) <= 1
    };

    // Chain links: an internal edge between two collapsible tasks is, by
    // the degree bound, the source's only out-edge and the target's only
    // in-edge, so following links yields maximal linear runs.
    let mut next_of: HashMap<&str, &str> = HashMap::new();
    let mut prev_of: HashMap<&str, &str> = HashMap::new();
    for &(a, b) in &internal {
        if collapsible(a) && collapsible(b) {
            next_of.insert(a, b);
            prev_of.insert(b, a);
        }
    }
    let mut in_run: HashMap<&str, &str> = HashMap::new(); // member id → run head id
    let mut run_tail: HashMap<&str, (usize, &str)> = HashMap::new(); // head → (len, tail)
    for t in &members {
        let head = t.id.as_str();
        if prev_of.contains_key(head) || !next_of.contains_key(head) {
            continue; // not the head of a ≥2-task chain
        }
        let mut cur = head;
        let mut len = 1;
        in_run.insert(head, head);
        while let Some(&nx) = next_of.get(cur) {
            in_run.insert(nx, head);
            cur = nx;
            len += 1;
        }
        run_tail.insert(head, (len, cur));
    }
    let rid = |id: &str| match in_run.get(id) {
        Some(head) => format!("C{head}"),
        None => format!("T{id}"),
    };

    let mut out = String::from("flowchart LR\n");
    out.push_str("  classDef next fill:#c8e6c9,stroke:#2e7d32,color:#1b5e20\n");
    out.push_str("  classDef blocked fill:#eceff1,stroke:#b0bec5,color:#78909c\n");
    out.push_str("  classDef ghost fill:#fafafa,stroke:#90a4ae,stroke-dasharray:5 4,color:#90a4ae\n");
    out.push_str("  classDef run fill:#eceff1,stroke:#78909c,stroke-width:2px,color:#546e7a\n");

    for t in &members {
        let id = t.id.as_str();
        if let Some(&head) = in_run.get(id) {
            if head != id {
                continue; // rendered as part of its run's collapsed node
            }
            let (len, tail) = run_tail[head];
            // [[…]] draws Mermaid's subroutine shape: double side bars,
            // reading as "a stack of steps".
            out.push_str(&format!(
                "  C{id}[[\"{} ⇢ {} ({len} tasks)\"]]:::run\n",
                escape(&t.content),
                escape(&by_id[tail].content),
            ));
            continue;
        }
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

    let mut ghosts: HashSet<&str> = HashSet::new();
    for (prereq, task) in edges {
        let p_in = member_ids.contains(prereq);
        let t_in = member_ids.contains(task);
        match (p_in, t_in) {
            (true, true) => {
                let (a, b) = (rid(prereq), rid(task));
                if a != b {
                    out.push_str(&format!("  {a} --> {b}\n"));
                }
            }
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
        assert!(chart.starts_with("flowchart LR"));
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
        assert_eq!(html.matches("flowchart LR").count(), 2);
        assert!(html.contains("startOnLoad: false"));
        assert!(html.contains("mermaid.run"));
    }

    #[test]
    fn blocked_linear_run_collapses_and_reconnects() {
        let projects = vec![project("p1", "Shop")];
        let tasks = vec![
            task("1", "assemble", "", "p1"),
            task("2", "sand", "after: 1", "p1"),
            task("3", "first coat", "after: 2", "p1"),
            task("4", "buff", "after: 3", "p1"),
            task("5", "install", "after: 4\nafter: 6", "p1"),
            task("6", "buy hardware", "", "p1"),
        ];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        let chart = project_chart(&projects[0], &projects, &tasks, &dag, &classes).unwrap();
        // 2→3→4 are blocked and linear: one pill, first ⇢ last, count.
        assert!(chart.contains("C2[[\"sand ⇢ buff (3 tasks)\"]]:::run"));
        assert!(!chart.contains("T2["));
        assert!(!chart.contains("T3["));
        assert!(!chart.contains("T4["));
        // Edges reconnect through the pill; intra-run edges vanish.
        assert!(chart.contains("T1 --> C2"));
        assert!(chart.contains("C2 --> T5"));
        assert!(!chart.contains("C2 --> C2"));
        // The merge point (two prereqs) stays individual.
        assert!(chart.contains("T5[\"install\"]:::blocked"));
        assert!(chart.contains("T6 --> T5"));
        assert!(chart.contains("classDef run"));
    }

    #[test]
    fn run_at_chain_end_collapses() {
        let projects = vec![project("p1", "Shop")];
        let tasks = vec![
            task("1", "assemble", "", "p1"),
            task("2", "sand", "after: 1", "p1"),
            task("3", "buff", "after: 2", "p1"),
        ];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        let chart = project_chart(&projects[0], &projects, &tasks, &dag, &classes).unwrap();
        assert!(chart.contains("T1[\"assemble\"]:::next"));
        assert!(chart.contains("C2[[\"sand ⇢ buff (2 tasks)\"]]:::run"));
        assert!(chart.contains("T1 --> C2"));
        assert!(!chart.contains("T2["));
        assert!(!chart.contains("T3["));
    }

    #[test]
    fn chain_containing_next_task_does_not_collapse() {
        let projects = vec![project("p1", "Shop")];
        let tasks = vec![
            task("1", "assemble", "", "p1"),
            task("2", "sand", "after: 1", "p1"),
            task("3", "buff", "after: 2", "p1"),
        ];
        let dag = Dag::build(&tasks);
        let mut classes = dag.classify(&tasks);
        // Force a mid-chain frontier task: it must render individually and
        // break the run; the lone blocked neighbor is too short to collapse.
        classes.insert("2".into(), LABEL_NEXT);
        let chart = project_chart(&projects[0], &projects, &tasks, &dag, &classes).unwrap();
        assert!(chart.contains("T2[\"sand\"]:::next"));
        assert!(chart.contains("T3[\"buff\"]:::blocked"));
        assert!(!chart.contains("[["));
        assert!(chart.contains("T1 --> T2"));
        assert!(chart.contains("T2 --> T3"));
    }

    #[test]
    fn ghost_edge_tasks_are_never_absorbed_into_runs() {
        let projects = vec![project("p1", "Shop"), project("p2", "House")];
        let tasks = vec![
            task("1", "assemble", "", "p1"),
            task("2", "sand", "after: 1", "p1"),
            task("3", "buff", "after: 2", "p1"),
            task("9", "hang shelf", "after: 2", "p2"),
            task("10", "style shelf", "after: 9", "p2"),
        ];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);

        // "sand" has an external dependent: it stays individual, so no run
        // forms even though 2→3 would otherwise be a blocked chain.
        let shop = project_chart(&projects[0], &projects, &tasks, &dag, &classes).unwrap();
        assert!(!shop.contains("[["));
        assert!(shop.contains("T2[\"sand\"]:::blocked"));
        assert!(shop.contains("T3[\"buff\"]:::blocked"));
        assert!(shop.contains("T2 -.-> G9"));

        // "hang shelf" has an external prerequisite: same rule on the
        // dependent side, so the 9→10 chain stays uncollapsed too.
        let house = project_chart(&projects[1], &projects, &tasks, &dag, &classes).unwrap();
        assert!(!house.contains("[["));
        assert!(house.contains("T9[\"hang shelf\"]:::blocked"));
        assert!(house.contains("T10[\"style shelf\"]:::blocked"));
        assert!(house.contains("G2 -.-> T9"));
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
