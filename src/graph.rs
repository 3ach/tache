//! Read-only HTML view of the dependency graph, served at the root.
//!
//! Renders every `#dag` project's active tasks as its own left-to-right
//! Graphviz digraph behind a vanilla-JS tab bar — one tab per project,
//! nodes colored by frontier status, edges pointing prerequisite →
//! dependent. Tasks with a due date get a second, smaller label line
//! ("due Aug 5" — year appended only when it differs from the current
//! one, red when the date is past). Overdue-ness compares against UTC
//! today, which can disagree with the user's local date near midnight;
//! close enough for a glanceable view. Ghost nodes stay name-only —
//! they are de-emphasized context, not actionable rows.
//! Cross-project edges show the external endpoint as a dashed "ghost"
//! node labeled with its name and project. Charts render lazily on
//! first tab activation into a pan/zoom pane (wheel zooms at the
//! cursor, drag pans; on touch one finger pans, two pinch-zoom, and
//! double-tap replaces double-click; initial view is fit-to-width and
//! top-aligned, double-click/tap toggles fit-width and fit-all). The
//! server
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
use crate::todoist::{Due, Project, Task};

pub fn page(
    projects: &[Project],
    tasks: &[Task],
    dag: &Dag,
    classes: &HashMap<String, &'static str>,
) -> String {
    let mut tabs = String::new();
    let mut panes = String::new();
    let today = today_utc();
    for p in projects {
        let Some(chart) = project_chart(p, projects, tasks, dag, classes, today) else {
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
  // Touch pointers: one drags, two pinch-zoom. The Map holds live
  // pointers in pane coords; every down/up re-baselines the gesture
  // from the current view, so drag→pinch→drag transitions don't jump.
  const pointers = new Map();
  let base = null, moved = false;
  const at = (e) => {{
    const r = pane.getBoundingClientRect();
    return {{ x: e.clientX - r.left, y: e.clientY - r.top }};
  }};
  const centroid = () => {{
    const pts = [...pointers.values()];
    const c = pts.reduce((s, p) => ({{ x: s.x + p.x, y: s.y + p.y }}), {{ x: 0, y: 0 }});
    return {{ x: c.x / pts.length, y: c.y / pts.length,
             d: pts.length > 1 ? Math.hypot(pts[0].x - pts[1].x, pts[0].y - pts[1].y) : 0 }};
  }};
  const rebase = () => {{
    const v = views.get(pane.id);
    base = v && pointers.size ? {{ v: {{ ...v }}, c: centroid() }} : null;
  }};
  pane.addEventListener("pointerdown", (e) => {{
    if (!views.get(pane.id) || (e.pointerType === "mouse" && e.button !== 0)) return;
    e.preventDefault();
    pane.setPointerCapture(e.pointerId);
    if (!pointers.size) moved = false;
    pointers.set(e.pointerId, at(e));
    rebase();
  }});
  pane.addEventListener("pointermove", (e) => {{
    if (!pointers.has(e.pointerId) || !base) return;
    pointers.set(e.pointerId, at(e));
    const v = views.get(pane.id);
    const c = centroid();
    if (pointers.size > 1 || Math.hypot(c.x - base.c.x, c.y - base.c.y) > 8) moved = true;
    // Same anchor math as wheel zoom: scale by the pinch-distance ratio
    // and keep the content point under the gesture's start centroid
    // pinned to the current one (one pointer reduces to a plain drag).
    const k = base.c.d ? Math.min(Math.max((base.v.k * c.d) / base.c.d, 0.05), 8) : base.v.k;
    v.k = k;
    v.x = c.x - ((base.c.x - base.v.x) * k) / base.v.k;
    v.y = c.y - ((base.c.y - base.v.y) * k) / base.v.k;
    apply(pane);
  }});
  // Double-tap stands in for dblclick on touch, where the pointerdown
  // preventDefault stops the browser from synthesizing one.
  let tap = null;
  const up = (e) => {{
    if (!pointers.delete(e.pointerId)) return;
    rebase();
    if (e.pointerType === "mouse" || pointers.size) return;
    const p = at(e);
    if (!moved && tap && e.timeStamp - tap.t < 350 &&
        Math.hypot(p.x - tap.x, p.y - tap.y) < 40) {{
      fit(pane, modes.get(pane.id) === "width" ? "all" : "width");
      tap = null;
    }} else {{
      tap = moved ? null : {{ t: e.timeStamp, x: p.x, y: p.y }};
    }}
  }};
  pane.addEventListener("pointerup", up);
  pane.addEventListener("pointercancel", up);
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
    today: (i32, u32, u32),
) -> Option<String> {
    let members: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.project_id == project.id)
        .collect();
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
    const NEXT: &str =
        r##"class="next", fillcolor="#c8e6c9", color="#2e7d32", fontcolor="#1b5e20""##;
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
        // Due-line color tracks the node's palette (medium green on
        // next, gray on blocked) so the date reads as secondary text.
        let (attrs, due_color) = match classes.get(&t.id).copied() {
            Some(LABEL_NEXT) => (NEXT, "#558b2f"),
            _ => (BLOCKED, "#90a4ae"),
        };
        out.push_str(&format!(
            "  T{} [{}, {attrs}]\n",
            t.id,
            node_label(&t.content, t.due.as_ref(), due_color, today)
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

/// The full `label=...` attribute for a task node. Without a due date
/// this is the plain double-quoted label; with one it becomes an
/// HTML-like label (`label=<...>`) so the date line can drop to a
/// smaller point size and its own color — quoted labels are single-font.
///
/// HTML-like labels escape differently from quoted ones: backslashes
/// and quotes are literal, but text runs through graphviz's own entity
/// decoding, so content is entity-escaped once for graphviz and the
/// whole label (tags, attributes and all — including the `<`/`>`
/// delimiters around it) once more for the `<pre>` transport. `escape()`
/// serves both layers; double-escaped text like `&amp;amp;` decodes to
/// `&amp;` in textContent, which graphviz then reads as `&`.
fn node_label(content: &str, due: Option<&Due>, due_color: &str, today: (i32, u32, u32)) -> String {
    let Some(due) = due else {
        return format!("label=\"{}\"", label(content));
    };
    // Unparseable dates (nothing Todoist currently emits) fall back to
    // the raw string, shown muted rather than dropped.
    let (text, overdue) = match parse_iso_date(&due.date) {
        Some(d) => (format_due(d, today), d < today),
        None => (due.date.clone(), false),
    };
    let color = if overdue { "#c62828" } else { due_color };
    let name = wrap(content, WRAP_COLS)
        .iter()
        .map(|line| escape(line))
        .collect::<Vec<_>>()
        .join("<BR/>");
    let html = format!(
        "{name}<BR/><FONT POINT-SIZE=\"10\" COLOR=\"{color}\">due {}</FONT>",
        escape(&text)
    );
    format!("label=&lt;{}&gt;", escape(&html))
}

/// (year, month, day) from the leading `YYYY-MM-DD` of a Todoist due
/// date, which may carry a `THH:MM:SS` tail; None on anything malformed.
fn parse_iso_date(s: &str) -> Option<(i32, u32, u32)> {
    let b = s.as_bytes();
    if b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
        return None;
    }
    let y = s.get(0..4)?.parse().ok()?;
    let m: u32 = s.get(5..7)?.parse().ok()?;
    let d: u32 = s.get(8..10)?.parse().ok()?;
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d))
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "Aug 5", or "Aug 5 2027" when the year isn't today's — the common
/// case stays terse and cross-year dates stay unambiguous.
fn format_due(due: (i32, u32, u32), today: (i32, u32, u32)) -> String {
    let (y, m, d) = due;
    let month = MONTHS[(m - 1) as usize];
    if y == today.0 {
        format!("{month} {d}")
    } else {
        format!("{month} {d} {y}")
    }
}

/// Today's UTC civil date, no chrono: epoch seconds → days →
/// year/month/day via Howard Hinnant's civil_from_days algorithm.
fn today_utc() -> (i32, u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    civil_from_days((secs / 86_400) as i64)
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y as i32, m, d)
}

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

    const TODAY: (i32, u32, u32) = (2026, 8, 1);

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

    fn due_task(id: &str, content: &str, description: &str, project_id: &str, date: &str) -> Task {
        let mut t = task(id, content, description, project_id);
        t.due = Some(Due { date: date.into() });
        t
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
        let chart = project_chart(&projects[0], &projects, &tasks, &dag, &classes, TODAY).unwrap();
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
        let chart = project_chart(&projects[0], &projects, &tasks, &dag, &classes, TODAY).unwrap();
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
        let chart = project_chart(&projects[0], &projects, &tasks, &dag, &classes, TODAY).unwrap();
        // Greedy 30-col wrap; the literal \n survives to graphviz.
        assert!(
            chart.contains(
                r#"label="Apply second finish coat to\nthe corner-lower cabinet\ncarcass""#
            )
        );
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
    fn due_dates_render_as_smaller_second_line() {
        let projects = vec![project("p1", "Shop")];
        let tasks = vec![
            due_task("1", "buy lumber", "", "p1", "2026-08-05"),
            task("2", "sand boards", "after: buy lumber", "p1"),
        ];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        let chart = project_chart(&projects[0], &projects, &tasks, &dag, &classes, TODAY).unwrap();
        // HTML-like label, double-escaped for the <pre> transport: the
        // `label=<...>` delimiters and tags arrive entity-escaped, the
        // due line sits in a smaller status-colored FONT span.
        assert!(chart.contains(
            "T1 [label=&lt;buy lumber&lt;BR/&gt;&lt;FONT POINT-SIZE=&quot;10&quot; \
             COLOR=&quot;#558b2f&quot;&gt;due Aug 5&lt;/FONT&gt;&gt;, class=\"next\""
        ));
        // Due-less tasks keep the plain quoted label.
        assert!(chart.contains("T2 [label=\"sand boards\", class=\"blocked\""));
    }

    #[test]
    fn overdue_dates_render_red() {
        let projects = vec![project("p1", "Shop")];
        let tasks = vec![due_task("1", "buy lumber", "", "p1", "2026-07-20")];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        let chart = project_chart(&projects[0], &projects, &tasks, &dag, &classes, TODAY).unwrap();
        assert!(chart.contains("COLOR=&quot;#c62828&quot;&gt;due Jul 20&lt;"));
    }

    #[test]
    fn due_line_color_tracks_blocked_status() {
        let projects = vec![project("p1", "Shop")];
        let tasks = vec![
            task("1", "buy lumber", "", "p1"),
            due_task("2", "sand boards", "after: buy lumber", "p1", "2026-08-09"),
        ];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        let chart = project_chart(&projects[0], &projects, &tasks, &dag, &classes, TODAY).unwrap();
        assert!(chart.contains("COLOR=&quot;#90a4ae&quot;&gt;due Aug 9&lt;"));
    }

    #[test]
    fn due_dates_outside_current_year_show_the_year() {
        assert_eq!(format_due((2027, 1, 15), TODAY), "Jan 15 2027");
        assert_eq!(format_due((2026, 8, 5), TODAY), "Aug 5");
    }

    #[test]
    fn due_content_is_double_escaped_in_html_labels() {
        let out = node_label(
            "glue & clamp",
            Some(&Due {
                date: "2026-08-05".into(),
            }),
            "#558b2f",
            TODAY,
        );
        // & escapes once for graphviz's entity decoding, once more for
        // the <pre> transport.
        assert!(out.contains("glue &amp;amp; clamp"));
    }

    #[test]
    fn unparseable_due_dates_fall_back_to_raw_text() {
        let out = node_label(
            "buy lumber",
            Some(&Due {
                date: "someday".into(),
            }),
            "#558b2f",
            TODAY,
        );
        assert!(out.contains("due someday"));
        assert!(!out.contains("#c62828"));
    }

    #[test]
    fn iso_dates_parse_with_and_without_time() {
        assert_eq!(parse_iso_date("2026-08-05"), Some((2026, 8, 5)));
        assert_eq!(parse_iso_date("2026-08-05T14:30:00"), Some((2026, 8, 5)));
        assert_eq!(parse_iso_date("someday"), None);
        assert_eq!(parse_iso_date("2026/08/05"), None);
        assert_eq!(parse_iso_date("2026-13-05"), None);
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20666), (2026, 8, 1));
        // Leap day.
        assert_eq!(civil_from_days(19782), (2024, 2, 29));
    }

    #[test]
    fn skips_projects_without_tasks() {
        let projects = vec![project("empty", "Empty")];
        assert!(
            project_chart(
                &projects[0],
                &projects,
                &[],
                &Dag::default(),
                &HashMap::new(),
                TODAY
            )
            .is_none()
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
        let shop = project_chart(&projects[0], &projects, &tasks, &dag, &classes, TODAY).unwrap();
        assert!(shop.contains("T1 [label=\"build cabinet\", class=\"next\""));
        assert!(shop.contains("G2 [label=\"install cabinet (House)\", class=\"ghost\""));
        assert!(shop.contains("T1 -> G2 [style=dashed]"));
        assert!(!shop.contains("T2 [label=\"install cabinet\""));

        // Dependent's tab: prerequisite appears as a ghost.
        let house = project_chart(&projects[1], &projects, &tasks, &dag, &classes, TODAY).unwrap();
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
        let shop = project_chart(&projects[0], &projects, &tasks, &dag, &classes, TODAY).unwrap();
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
        let chart = project_chart(&projects[0], &projects, &tasks, &dag, &classes, TODAY).unwrap();
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
        let chart = project_chart(&projects[0], &projects, &tasks, &dag, &classes, TODAY).unwrap();
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
