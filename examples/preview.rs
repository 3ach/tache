//! Renders the graph page HTML from fixture data shaped like the real Library
//! project — many parallel build chains converging on one install hub —
//! so layout/pan-zoom changes can be eyeballed without a Todoist token:
//!
//! ```sh
//! cargo run --example preview   # writes /tmp/tache-graph-preview.html
//! ```

use tache::dag::Dag;
use tache::graph;
use tache::todoist::{Project, Task};

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

fn main() {
    let projects = vec![
        Project {
            id: "lib".into(),
            name: "Library".into(),
            description: "#dag".into(),
        },
        Project {
            id: "house".into(),
            name: "House".into(),
            description: "#dag".into(),
        },
    ];

    let mut tasks: Vec<Task> = Vec::new();
    let mut hub_prereqs: Vec<String> = Vec::new();

    // 16 parallel shelf chains: a frontier "cut notch" head followed by
    // 4–7 blocked steps (5–8 tasks total), tail feeding the install hub.
    let steps = [
        "assemble",
        "chamfer",
        "sand 120",
        "sand 220",
        "first coat",
        "second coat",
        "final buff",
    ];
    for i in 1..=16u32 {
        let len = 5 + (i as usize % 4); // chain lengths cycle 5..=8
        let head = format!("c{i}s0");
        tasks.push(task(&head, &format!("cut notch {i}"), "", "lib"));
        let mut prev = head;
        for (j, step) in steps.iter().take(len - 1).enumerate() {
            let id = format!("c{i}s{}", j + 1);
            tasks.push(task(
                &id,
                &format!("{step} {i}"),
                &format!("after: {prev}"),
                "lib",
            ));
            prev = id;
        }
        hub_prereqs.push(prev);
    }

    // A few loose parts feeding the hub directly, pushing it past 20
    // prerequisites (16 chain tails + 6 of these).
    for (i, part) in ["brackets", "rails", "anchors", "shims", "trim", "felt pads"]
        .iter()
        .enumerate()
    {
        let id = format!("part{i}");
        tasks.push(task(&id, &format!("buy {part}"), "", "lib"));
        hub_prereqs.push(id);
    }

    // One cross-project prerequisite: the hub also waits on a House task,
    // drawn as a dashed ghost edge.
    tasks.push(task("wall", "clear the wall", "", "house"));
    tasks.push(task("sofa", "move the sofa", "after: wall", "house"));
    hub_prereqs.push("wall".into());

    tasks.push(task(
        "hub",
        "install shelves",
        &format!("after: {}", hub_prereqs.join(", ")),
        "lib",
    ));
    // Downstream of the hub.
    tasks.push(task("books", "load books", "after: hub", "lib"));

    // A chain of realistically long task names — exercises server-side
    // label wrapping on both frontier and blocked nodes.
    tasks.push(task(
        "long0",
        "Template the corner-lower cabinet scribe against the wall irregularities",
        "",
        "lib",
    ));
    tasks.push(task(
        "long1",
        "Apply second finish coat to the corner-lower cabinet carcass",
        "after: long0",
        "lib",
    ));
    tasks.push(task(
        "long2",
        "Wet-sand & inspect the corner-lower cabinet carcass for drips",
        "after: long1",
        "lib",
    ));
    tasks.push(task(
        "long3",
        "Fit the corner-lower cabinet \"french cleat\" and check level",
        "after: long2",
        "lib",
    ));
    hub_prereqs.push("long3".into());

    // Frontier-only singletons, one with a long name.
    tasks.push(task("saw", "oil the miter saw", "", "lib"));
    tasks.push(task("sweep", "sweep the shop", "", "lib"));
    tasks.push(task(
        "vac",
        "Empty the dust extractor bag and wash the pleated HEPA pre-filter",
        "",
        "lib",
    ));

    // Due dates: fixed values far in the past/future so the preview
    // looks the same whenever it is rendered — one overdue frontier
    // head (red), one upcoming (muted, current-ish year), one blocked
    // with a date (gray), one cross-year (shows the year), and one on
    // a wrapped long-name node.
    let dues = [
        ("c1s0", "2020-03-01"),          // overdue
        ("c2s0", "2099-08-05"),          // upcoming, cross-year
        ("c1s2", "2099-09-12T14:30:00"), // blocked, with time-of-day tail
        ("hub", "2099-12-24"),
        ("long0", "2020-11-30"), // overdue on a wrapped label
    ];
    for (id, date) in dues {
        let t = tasks
            .iter_mut()
            .find(|t| t.id == id)
            .expect("due fixture id");
        t.due = Some(tache::todoist::Due { date: date.into() });
    }

    let dag = Dag::build(&tasks);
    let classes = dag.classify(&tasks);
    let html = graph::page(&projects, &tasks, &dag, &classes);
    let path = "/tmp/tache-graph-preview.html";
    std::fs::write(path, html).expect("write preview html");
    println!("wrote {path}");
}
