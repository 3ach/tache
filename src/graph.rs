//! Read-only HTML view of the dependency graph, served at the root.
//!
//! Renders every `#dag` project's active tasks as its own left-to-right
//! Graphviz digraph behind a vanilla-JS tab bar — one tab per project,
//! nodes colored by frontier status, edges pointing prerequisite →
//! dependent.
//! Cross-project edges show the external endpoint as a dashed "ghost"
//! node labeled with its name and project. Charts render lazily on
//! first tab activation into a pan/zoom pane (wheel zooms at the
//! cursor, drag pans; initial view is fit-to-width and top-aligned,
//! double-click toggles between fit-width and fit-all). The server
//! emits static HTML with DOT source per pane; layout runs client-side
//! via @viz-js/viz (Graphviz compiled to WASM, one self-contained ESM
//! from jsdelivr). Public: no auth, the page is deliberately shareable.
//!
//! Sizing: the DOT carries no size/ratio — those depend on the client's
//! viewport, so the page computes the pane's aspect at render time and
//! passes it as a `ratio` graph attribute through viz-js's
//! graphAttributes option (Graphviz `-G` defaults, which apply because
//! the DOT leaves them unset). Numeric ratio makes dot redistribute
//! whitespace until drawing height/width matches the pane, so fit-width
//! fills both dimensions without stretching text the way ratio=fill
//! would. Graphviz's `unflatten` preprocessor (which staggers wide
//! rank fans) would also help, but viz-js only ships layout engines
//! (dot/neato/…), not unflatten, so it is not used.

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
        // The <pre> holds raw DOT source until the pane's first
        // activation swaps in the rendered SVG.
        panes.push_str(&format!(
            "<div class=\"pane\" id=\"pane-{id}\"><pre class=\"chart\">\n{chart}</pre></div>\n",
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
  html, body {{ height: 100%; }}
  body {{ margin: 0; padding: 0.75rem 1rem; box-sizing: border-box; display: flex;
         flex-direction: column; background: #fafafa; font-family: system-ui, sans-serif; }}
  h1 {{ font-size: 1.1rem; font-weight: 600; margin: 0 0 0.6rem; }}
  #tabs {{ display: flex; flex-wrap: wrap; gap: 0.4rem; margin-bottom: 0.6rem; }}
  .tab {{ font: inherit; padding: 0.3rem 0.9rem; border: 1px solid #cfd8dc; border-radius: 999px;
         background: #fff; color: #455a64; cursor: pointer; }}
  .tab.active {{ background: #2e7d32; border-color: #2e7d32; color: #fff; }}
  /* Each pane is a pan/zoom viewport filling the space below the tab bar:
     the chart renders at natural size and a translate+scale transform
     (per-tab view state) positions it. */
  #panes {{ flex: 1; min-height: 0; position: relative; }}
  .pane {{ display: none; position: absolute; inset: 0; overflow: hidden; cursor: grab;
          border: 1px solid #eceff1; border-radius: 6px; background: #fff; touch-action: none; }}
  .pane.active {{ display: block; }}
  .pane pre.chart {{ margin: 0; position: absolute; left: 0; top: 0;
                    transform-origin: 0 0; line-height: 0; }}
  .pane svg {{ display: block; }}
  /* Pin SVG text to the family graphviz measured with (Helvetica; Arial
     and Liberation Sans are metric-compatible) so the page's system-ui
     font-family can't substitute wider glyphs that overflow node boxes. */
  .pane svg text {{ font-family: Helvetica, Arial, "Liberation Sans", sans-serif; }}
  /* Hide raw DOT source until it has been rendered into an SVG. */
  pre.chart:not([data-processed]) {{ visibility: hidden; }}
</style>
</head>
<body>
<h1>tache — dependency graph</h1>
<nav id="tabs">{tabs}</nav>
<main id="panes">
{panes}</main>
<script type="module">
import {{ instance }} from "https://cdn.jsdelivr.net/npm/@viz-js/viz@3.28.0/dist/viz.js";
// Real Graphviz (dot engine) compiled to WASM; one self-contained
// module, renders synchronously once loaded. Kick off the load now so
// the first tab doesn't wait on it serially.
const vizReady = instance();

// Pan/zoom: per-tab {{x, y, k}} applied as a CSS transform on the chart.
const views = new Map();
function apply(pane) {{
  const v = views.get(pane.id);
  const pre = pane.querySelector("pre.chart");
  if (v && pre) pre.style.transform = `translate(${{v.x}}px, ${{v.y}}px) scale(${{v.k}})`;
}}
// Two fit modes: "width" (the default view) scales the chart to fill
// the pane's width and top-aligns it — tall charts are panned/scrolled
// through, not shrunk into frame. Capped at 1.5x so small charts don't
// blow up cartoonishly; charts wider than the pane scale down below
// natural size. "all" is the classic fit-everything overview.
// Double-click toggles between the two.
const modes = new Map();
function fit(pane, mode = "width") {{
  const svg = pane.querySelector("svg");
  if (!svg) return;
  const vb = svg.viewBox.baseVal;
  const w = vb && vb.width ? vb.width : svg.getBoundingClientRect().width;
  const h = vb && vb.height ? vb.height : svg.getBoundingClientRect().height;
  if (!w || !h) return;
  let k, y;
  const pad = 12;
  if (mode === "all") {{
    k = Math.min(pane.clientWidth / w, pane.clientHeight / h, 1);
    y = (pane.clientHeight - h * k) / 2;
  }} else {{
    k = Math.min((pane.clientWidth - 2 * pad) / w, 1.5);
    y = pad;
  }}
  views.set(pane.id, {{ k, x: (pane.clientWidth - w * k) / 2, y }});
  modes.set(pane.id, mode);
  apply(pane);
}}
for (const pane of document.querySelectorAll(".pane")) {{
  pane.addEventListener("wheel", (e) => {{
    const v = views.get(pane.id);
    if (!v) return;
    e.preventDefault();
    const k = Math.min(Math.max(v.k * Math.exp(-e.deltaY * 0.0015), 0.05), 8);
    const r = pane.getBoundingClientRect();
    const cx = e.clientX - r.left, cy = e.clientY - r.top;
    // Keep the point under the cursor fixed while scaling around it.
    v.x = cx - ((cx - v.x) * k) / v.k;
    v.y = cy - ((cy - v.y) * k) / v.k;
    v.k = k;
    apply(pane);
  }}, {{ passive: false }});
  pane.addEventListener("pointerdown", (e) => {{
    const v = views.get(pane.id);
    if (!v || e.button !== 0) return;
    e.preventDefault();
    pane.setPointerCapture(e.pointerId);
    const sx = e.clientX - v.x, sy = e.clientY - v.y;
    const move = (m) => {{ v.x = m.clientX - sx; v.y = m.clientY - sy; apply(pane); }};
    pane.addEventListener("pointermove", move);
    pane.addEventListener("pointerup",
      () => pane.removeEventListener("pointermove", move), {{ once: true }});
  }});
  pane.addEventListener("dblclick", () =>
    fit(pane, modes.get(pane.id) === "width" ? "all" : "width"));
}}

const rendered = new Set();
async function activate(id) {{
  for (const b of document.querySelectorAll(".tab"))
    b.classList.toggle("active", b.dataset.tab === id);
  for (const p of document.querySelectorAll(".pane"))
    p.classList.toggle("active", p.id === "pane-" + id);
  if (rendered.has(id)) return;
  rendered.add(id);
  const pane = document.getElementById("pane-" + id);
  const el = pane && pane.querySelector(".chart");
  if (!el) return;
  const viz = await vizReady;
  // Layout hint: the DOT sets no ratio, so this -G default applies.
  // Numeric ratio = desired drawing height/width; dot spreads node
  // positions in whichever dimension falls short, so the drawing's
  // aspect matches the pane and fit-width fills it top to bottom.
  const pad = 12;
  const ratio = Math.max(
    (pane.clientHeight - 2 * pad) / Math.max(pane.clientWidth - 2 * pad, 1), 0.05);
  const svg = viz.renderSVGElement(el.textContent,
    {{ graphAttributes: {{ ratio: ratio.toFixed(3) }} }});
  // viz sizes the SVG in pt (1pt = 4/3 px); pin the on-screen size to
  // the viewBox's unit count so fit()'s math is exact at scale 1.
  const vb = svg.viewBox.baseVal;
  svg.style.width = vb.width + "px";
  svg.style.height = vb.height + "px";
  el.replaceChildren(svg);
  el.setAttribute("data-processed", "");
  fit(pane);
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

/// Graphviz DOT digraph for one project, or None if it has no tasks.
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

    let member_ids: HashSet<&str> = members.iter().map(|t| t.id.as_str()).collect();

    // HashMap order is unstable; sort so output (and tests) are deterministic
    let mut edges: Vec<(&str, &str)> = dag
        .prereqs
        .iter()
        .flat_map(|(task, ps)| ps.iter().map(move |p| (p.as_str(), task.as_str())))
        .collect();
    edges.sort_unstable();

    // Per-status node attributes, the DOT equivalent of the old Mermaid
    // classDefs. `class=` carries through to the SVG for debugging and
    // tests; the inline attrs do the actual styling.
    const NEXT: &str = r##"class="next", fillcolor="#c8e6c9", color="#2e7d32", fontcolor="#1b5e20""##;
    const BLOCKED: &str =
        r##"class="blocked", fillcolor="#eceff1", color="#b0bec5", fontcolor="#78909c""##;
    const GHOST: &str = r##"class="ghost", style="filled,rounded,dashed", fillcolor="#fafafa", color="#90a4ae", fontcolor="#90a4ae""##;

    // Sizing lives client-side (see module docs): the page passes the
    // pane's aspect as a `ratio` -G default at render time, so the DOT
    // deliberately sets no size/ratio. The rest is tuned for the typical
    // shape — many parallel chains merging into hubs — which in LR is
    // row-count-bound: fit scale ≈ paneHeight / (rows × rowHeight), so
    // squat nodes (height=0.3 instead of graphviz's 0.5in minimum) and a
    // tight nodesep buy the labels ~40% more on-screen size. ranksep
    // barely matters: numeric ratio rescales rank spacing anyway.
    let mut out = String::from("digraph {\n");
    out.push_str("  rankdir=LR\n");
    out.push_str("  bgcolor=\"transparent\"\n");
    out.push_str("  nodesep=0.15\n");
    out.push_str("  ranksep=0.6\n");
    // fontname is plain "Helvetica" — a name graphviz's built-in metric
    // tables know — so node sizing uses the same metrics the browser
    // renders with (the page CSS pins svg text to Helvetica/Arial).
    // Font-list values like "Helvetica,Arial,sans-serif" miss the tables
    // and fall back to narrower estimates, overflowing the boxes.
    out.push_str("  fontname=\"Helvetica\"\n");
    out.push_str(
        "  node [shape=box, style=\"filled,rounded\", fontname=\"Helvetica\", fontsize=13, height=0.3, margin=\"0.15,0.08\"]\n",
    );
    out.push_str("  edge [color=\"#90a4ae\", arrowsize=0.8, fontname=\"Helvetica\"]\n");

    for t in &members {
        let attrs = match classes.get(&t.id).copied() {
            Some(LABEL_NEXT) => NEXT,
            _ => BLOCKED,
        };
        out.push_str(&format!(
            "  T{} [label=\"{}\", {attrs}]\n",
            t.id,
            label(&t.content)
        ));
    }

    let mut ghosts: HashSet<&str> = HashSet::new();
    for (prereq, task) in edges {
        let p_in = member_ids.contains(prereq);
        let t_in = member_ids.contains(task);
        match (p_in, t_in) {
            (true, true) => {
                out.push_str(&format!("  T{prereq} -> T{task}\n"));
            }
            // External dependent: ghost it in the prerequisite's tab.
            (true, false) => {
                let Some(label) = ghost_label(task, &by_id, &project_names) else {
                    continue;
                };
                if ghosts.insert(task) {
                    out.push_str(&format!("  G{task} [label=\"{label}\", {GHOST}]\n"));
                }
                out.push_str(&format!("  T{prereq} -> G{task} [style=dashed]\n"));
            }
            // External prerequisite: ghost it in the dependent's tab.
            (false, true) => {
                let Some(label) = ghost_label(prereq, &by_id, &project_names) else {
                    continue;
                };
                if ghosts.insert(prereq) {
                    out.push_str(&format!("  G{prereq} [label=\"{label}\", {GHOST}]\n"));
                }
                out.push_str(&format!("  G{prereq} -> T{task} [style=dashed]\n"));
            }
            (false, false) => {}
        }
    }
    out.push_str("}\n");
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
    Some(label(&format!("{} ({project})", t.content)))
}

/// Wrap threshold: graphviz never wraps labels itself, so lines longer
/// than this many chars break at word boundaries server-side.
const WRAP_COLS: usize = 30;

/// Wrap + escape for a DOT double-quoted label inside a `<pre>` block.
///
/// Wrapping happens first, on the raw text, so line widths count real
/// characters rather than entity expansions. Each line is then escaped
/// in two layers: backslash-escaping makes it safe for DOT (which the
/// client reads via textContent, i.e. after HTML entity decoding), and
/// entity-escaping &/</> keeps the surrounding HTML intact. Backslash
/// first so we don't re-escape our own escapes; & before </> so we
/// don't re-escape the entities'. Lines join with a literal `\n` added
/// after escaping, so it reaches graphviz as the DOT newline escape.
fn label(s: &str) -> String {
    wrap(s, WRAP_COLS)
        .iter()
        .map(|line| {
            line.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        })
        .collect::<Vec<_>>()
        .join("\\n")
}

/// Greedy word wrap at `cols` characters. Words longer than a whole
/// line stand alone unbroken; runs of whitespace collapse to one space.
fn wrap(s: &str, cols: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut width = 0;
    for word in s.split_whitespace() {
        let wlen = word.chars().count();
        if width > 0 && width + 1 + wlen > cols {
            lines.push(std::mem::take(&mut line));
            width = 0;
        }
        if width > 0 {
            line.push(' ');
            width += 1;
        }
        line.push_str(word);
        width += wlen;
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// Escape for HTML text and attribute values (tab titles, pane ids).
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
        assert!(chart.starts_with("digraph {"));
        assert!(chart.trim_end().ends_with('}'));
        assert!(chart.contains("rankdir=LR"));
        assert!(chart.contains("T1 [label=\"buy lumber\", class=\"next\""));
        assert!(chart.contains("T2 [label=\"sand \\\"rough\\\" boards\", class=\"blocked\""));
        assert!(chart.contains("T1 -> T2"));
        // Sizing is a client-side -G default; the DOT must not pin it.
        assert!(!chart.contains("ratio"));
        assert!(!chart.contains(" size="));
    }

    #[test]
    fn labels_escape_dot_and_html() {
        let projects = vec![project("p1", "Shop")];
        let tasks = vec![task("1", "glue A\\B <&> \"joints\"", "", "p1")];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        let chart = project_chart(&projects[0], &projects, &tasks, &dag, &classes).unwrap();
        // Backslash and quote are DOT-escaped; &, <, > are HTML entities
        // (the browser decodes them back before viz parses the DOT).
        assert!(chart.contains(r#"label="glue A\\B &lt;&amp;&gt; \"joints\"""#));
    }

    #[test]
    fn long_labels_wrap_at_word_boundaries() {
        let projects = vec![project("p1", "Shop")];
        let tasks = vec![task(
            "1",
            "Apply second finish coat to the corner-lower cabinet carcass",
            "",
            "p1",
        )];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        let chart = project_chart(&projects[0], &projects, &tasks, &dag, &classes).unwrap();
        // Greedy 30-col wrap; the literal \n survives to graphviz.
        assert!(chart.contains(
            r#"label="Apply second finish coat to\nthe corner-lower cabinet\ncarcass""#
        ));
    }

    #[test]
    fn short_labels_stay_on_one_line() {
        assert_eq!(label("sand the panel"), "sand the panel");
        assert_eq!(wrap("sand the panel", 30), vec!["sand the panel"]);
        // Exactly at the limit: still one line.
        assert_eq!(wrap(&"x".repeat(30), 30), vec!["x".repeat(30)]);
        // A single overlong word is never broken mid-word.
        assert_eq!(wrap(&"y".repeat(40), 30), vec!["y".repeat(40)]);
    }

    #[test]
    fn wrapping_counts_raw_chars_not_escaped_entities() {
        // Widths are measured before escaping, so quotes count as one
        // char and &/< don't balloon into entity-length "words"; the
        // join's \n lands outside any escape sequence.
        let name = r#"sand the "extra-long" panels & <braces> before glue-up"#;
        assert_eq!(
            label(name),
            "sand the \\\"extra-long\\\" panels &amp;\\n&lt;braces&gt; before glue-up"
        );
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
        assert!(shop.contains("T1 [label=\"build cabinet\", class=\"next\""));
        assert!(shop.contains("G2 [label=\"install cabinet (House)\", class=\"ghost\""));
        assert!(shop.contains("T1 -> G2 [style=dashed]"));
        assert!(!shop.contains("T2 [label=\"install cabinet\""));

        // Dependent's tab: prerequisite appears as a ghost.
        let house = project_chart(&projects[1], &projects, &tasks, &dag, &classes).unwrap();
        assert!(house.contains("T2 [label=\"install cabinet\", class=\"blocked\""));
        assert!(house.contains("G1 [label=\"build cabinet (Shop)\", class=\"ghost\""));
        assert!(house.contains("G1 -> T2 [style=dashed]"));
        assert!(!house.contains("T1 [label=\"build cabinet\""));
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
        assert_eq!(shop.matches("G3 [label=").count(), 1);
        assert!(shop.contains("T1 -> G3 [style=dashed]"));
        assert!(shop.contains("T2 -> G3 [style=dashed]"));
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
        assert_eq!(html.matches("digraph {").count(), 2);
        assert!(html.contains("@viz-js/viz"));
        assert!(html.contains("renderSVGElement"));
        assert!(!html.contains("mermaid"));
    }

    #[test]
    fn linear_chains_render_every_task_individually() {
        let projects = vec![project("p1", "Shop")];
        let tasks = vec![
            task("1", "assemble", "", "p1"),
            task("2", "sand", "after: 1", "p1"),
            task("3", "first coat", "after: 2", "p1"),
            task("4", "buff", "after: 3", "p1"),
        ];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        let chart = project_chart(&projects[0], &projects, &tasks, &dag, &classes).unwrap();
        // No run-collapsing: every task in a blocked chain gets its own node.
        assert!(chart.contains("T1 [label=\"assemble\", class=\"next\""));
        assert!(chart.contains("T2 [label=\"sand\", class=\"blocked\""));
        assert!(chart.contains("T3 [label=\"first coat\", class=\"blocked\""));
        assert!(chart.contains("T4 [label=\"buff\", class=\"blocked\""));
        assert!(chart.contains("T1 -> T2"));
        assert!(chart.contains("T2 -> T3"));
        assert!(chart.contains("T3 -> T4"));
        assert!(!chart.contains("class=\"run\""));
    }

    #[test]
    fn many_incoming_edges_all_draw() {
        let projects = vec![project("p1", "Shop")];
        let n = 20;
        let mut tasks: Vec<Task> = (1..=n)
            .map(|i| task(&i.to_string(), &format!("part {i}"), "", "p1"))
            .collect();
        let after: Vec<String> = (1..=n).map(|i| i.to_string()).collect();
        tasks.push(task(
            "99",
            "install",
            &format!("after: {}", after.join(", ")),
            "p1",
        ));
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        let chart = project_chart(&projects[0], &projects, &tasks, &dag, &classes).unwrap();
        assert!(chart.contains("T99 [label=\"install\", class=\"blocked\""));
        assert_eq!(chart.matches("-> T99").count(), n);
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
