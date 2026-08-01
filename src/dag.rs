//! Dependency graph over Todoist tasks.
//!
//! Edges live inside Todoist itself, as `after:` lines in a task's
//! description — e.g. `after: buy lumber` or `after: sand boards, buy lumber`.
//! References are resolved against active tasks: by task id anywhere, or by
//! exact name / unique case-insensitive substring within the same project.

use std::collections::{HashMap, HashSet};

use crate::todoist::Task;

pub const LABEL_NEXT: &str = "next";
pub const LABEL_BLOCKED: &str = "blocked";

#[derive(Debug, Default)]
pub struct Dag {
    /// task id -> prerequisite task ids (all still active)
    pub prereqs: HashMap<String, Vec<String>>,
    /// (task id, reference text) pairs that matched no active task in the
    /// project. Usually means the prerequisite was completed — harmless —
    /// but `doctor` surfaces them in case of typos.
    pub unresolved: Vec<(String, String)>,
    /// (task id, reference text) pairs that matched more than one task.
    /// These block the task until disambiguated.
    pub ambiguous: Vec<(String, String)>,
}

/// Extract `after:` references from a task description.
pub fn parse_refs(description: &str) -> Vec<String> {
    description
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix("after:")
                .or_else(|| line.strip_prefix("After:"))
        })
        .flat_map(|rest| rest.split(','))
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .collect()
}

/// Resolve a reference: by task id anywhere, then by name within one project.
fn resolve<'a>(
    reference: &str,
    project_tasks: &[&'a Task],
    by_id: &HashMap<&str, &'a Task>,
) -> Resolution<'a> {
    if let Some(t) = by_id.get(reference) {
        return Resolution::One(t);
    }
    let needle = reference.to_lowercase();
    let exact: Vec<&&Task> = project_tasks
        .iter()
        .filter(|t| t.content.to_lowercase() == needle)
        .collect();
    match exact.as_slice() {
        [t] => return Resolution::One(t),
        [_, ..] => return Resolution::Many,
        [] => {}
    }
    let partial: Vec<&&Task> = project_tasks
        .iter()
        .filter(|t| t.content.to_lowercase().contains(&needle))
        .collect();
    match partial.as_slice() {
        [t] => Resolution::One(t),
        [] => Resolution::None,
        _ => Resolution::Many,
    }
}

enum Resolution<'a> {
    One(&'a Task),
    None,
    Many,
}

impl Dag {
    pub fn build(tasks: &[Task]) -> Self {
        let mut by_project: HashMap<&str, Vec<&Task>> = HashMap::new();
        for t in tasks {
            by_project.entry(t.project_id.as_str()).or_default().push(t);
        }
        let by_id: HashMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();

        let mut dag = Dag::default();
        for t in tasks {
            let siblings = &by_project[t.project_id.as_str()];
            for reference in parse_refs(&t.description) {
                match resolve(&reference, siblings, &by_id) {
                    Resolution::One(p) if p.id != t.id => dag
                        .prereqs
                        .entry(t.id.clone())
                        .or_default()
                        .push(p.id.clone()),
                    // self-reference: drop it, doctor reports as unresolved
                    Resolution::One(_) | Resolution::None => {
                        dag.unresolved.push((t.id.clone(), reference))
                    }
                    Resolution::Many => dag.ambiguous.push((t.id.clone(), reference)),
                }
            }
        }
        dag
    }

    /// Task ids that participate in a dependency cycle.
    pub fn cycle_members(&self) -> HashSet<String> {
        #[derive(Clone, Copy, PartialEq)]
        enum Color {
            White,
            Gray,
            Black,
        }
        let mut color: HashMap<&str, Color> = HashMap::new();
        let mut in_cycle: HashSet<String> = HashSet::new();

        fn visit<'a>(
            node: &'a str,
            prereqs: &'a HashMap<String, Vec<String>>,
            color: &mut HashMap<&'a str, Color>,
            stack: &mut Vec<&'a str>,
            in_cycle: &mut HashSet<String>,
        ) {
            color.insert(node, Color::Gray);
            stack.push(node);
            for next in prereqs.get(node).map(|v| v.as_slice()).unwrap_or(&[]) {
                match color.get(next.as_str()).copied().unwrap_or(Color::White) {
                    Color::White => visit(next, prereqs, color, stack, in_cycle),
                    Color::Gray => {
                        // everything from `next` to the top of the stack is on a cycle
                        let start = stack.iter().position(|n| *n == next.as_str()).unwrap();
                        for n in &stack[start..] {
                            in_cycle.insert(n.to_string());
                        }
                    }
                    Color::Black => {}
                }
            }
            stack.pop();
            color.insert(node, Color::Black);
        }

        for id in self.prereqs.keys() {
            if color.get(id.as_str()).copied().unwrap_or(Color::White) == Color::White {
                visit(
                    id,
                    &self.prereqs,
                    &mut color,
                    &mut Vec::new(),
                    &mut in_cycle,
                );
            }
        }
        in_cycle
    }

    /// Classify every active task as `next` (actionable now) or `blocked`.
    ///
    /// Frontier rule: a task is `next` when every prerequisite is complete
    /// (i.e. no longer active) — including tasks with no dependencies at
    /// all. Ambiguous references and cycle membership block a task.
    pub fn classify(&self, tasks: &[Task]) -> HashMap<String, &'static str> {
        let active: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        let cycles = self.cycle_members();
        let ambiguous: HashSet<&str> = self.ambiguous.iter().map(|(id, _)| id.as_str()).collect();

        tasks
            .iter()
            .map(|t| {
                let blocked = cycles.contains(&t.id)
                    || ambiguous.contains(t.id.as_str())
                    || self
                        .prereqs
                        .get(&t.id)
                        .is_some_and(|ps| ps.iter().any(|p| active.contains(p.as_str())));
                (
                    t.id.clone(),
                    if blocked { LABEL_BLOCKED } else { LABEL_NEXT },
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, content: &str, description: &str) -> Task {
        Task {
            id: id.into(),
            content: content.into(),
            description: description.into(),
            labels: vec![],
            project_id: "p1".into(),
            due: None,
        }
    }

    #[test]
    fn parses_refs() {
        assert_eq!(
            parse_refs("notes\nafter: buy lumber, sand boards\nAfter: paint"),
            vec!["buy lumber", "sand boards", "paint"]
        );
        assert!(parse_refs("no deps here").is_empty());
    }

    #[test]
    fn frontier_includes_dep_free_and_satisfied_tasks() {
        let tasks = vec![
            task("1", "buy lumber", ""),
            task("2", "sand boards", "after: buy lumber"),
            task("3", "stain shelves", "after: sand boards"),
            task("4", "unrelated errand", ""),
            // prerequisite already completed -> not in active set -> unresolved, but next
            task("5", "hang shelves", "after: order brackets"),
        ];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        assert_eq!(classes["1"], LABEL_NEXT);
        assert_eq!(classes["2"], LABEL_BLOCKED);
        assert_eq!(classes["3"], LABEL_BLOCKED);
        assert_eq!(classes["4"], LABEL_NEXT);
        assert_eq!(classes["5"], LABEL_NEXT);
        assert_eq!(
            dag.unresolved,
            vec![("5".to_string(), "order brackets".to_string())]
        );
    }

    #[test]
    fn resolves_by_unique_substring_and_id() {
        let tasks = vec![
            task("10", "Write launch blog post", ""),
            task("11", "Publish site", "after: blog"),
            task("12", "Tweet about it", "after: 10"),
        ];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        assert_eq!(classes["11"], LABEL_BLOCKED);
        assert_eq!(classes["12"], LABEL_BLOCKED);
        assert!(dag.unresolved.is_empty());
    }

    #[test]
    fn resolves_id_across_projects() {
        let mut prereq = task("40", "install cabinets", "");
        prereq.project_id = "p2".into();
        let tasks = vec![prereq, task("41", "game hutch", "after: 40")];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        assert_eq!(classes["41"], LABEL_BLOCKED);
        assert!(dag.unresolved.is_empty());
    }

    #[test]
    fn name_reference_stays_project_scoped() {
        let mut other = task("50", "paint fence", "");
        other.project_id = "p2".into();
        let tasks = vec![other, task("51", "hang gate", "after: paint fence")];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        assert_eq!(classes["51"], LABEL_NEXT);
        assert_eq!(dag.unresolved.len(), 1);
    }

    #[test]
    fn ambiguous_reference_blocks() {
        let tasks = vec![
            task("20", "Call plumber", ""),
            task("21", "Call electrician", ""),
            task("22", "Move in", "after: call"),
        ];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        assert_eq!(classes["22"], LABEL_BLOCKED);
        assert_eq!(dag.ambiguous.len(), 1);
    }

    #[test]
    fn cycles_block_their_members_only() {
        let tasks = vec![
            task("30", "a", "after: b"),
            task("31", "b", "after: a"),
            task("32", "c", ""),
        ];
        let dag = Dag::build(&tasks);
        let classes = dag.classify(&tasks);
        assert_eq!(classes["30"], LABEL_BLOCKED);
        assert_eq!(classes["31"], LABEL_BLOCKED);
        assert_eq!(classes["32"], LABEL_NEXT);
        assert_eq!(dag.cycle_members().len(), 2);
    }
}
