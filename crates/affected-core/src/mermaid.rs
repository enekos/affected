use std::collections::{BTreeMap, BTreeSet};

use crate::{EntryReport, Report};

// Keep PR diagrams legible. JSON always contains every hit and full explanation.
const ENTRIES: usize = 20;
const EXTRA_FILES: usize = 12;
const CHAIN_FILES: usize = 8;

#[derive(Default)]
struct Diagram {
    nodes: BTreeMap<(&'static str, String), usize>,
    lines: Vec<String>,
    edges: BTreeSet<(usize, &'static str, usize)>,
}

impl Diagram {
    fn node(&mut self, kind: &'static str, label: &str) -> usize {
        let next = self.nodes.len();
        *self.nodes.entry((kind, label.to_owned())).or_insert_with(|| {
            let mut safe = String::new();
            for ch in label.chars() {
                match ch {
                    ch if ch.is_alphanumeric() || " /._-".contains(ch) => safe.push(ch),
                    ch if ch.is_control() => safe.push(' '),
                    ch => {
                        safe.push_str(&format!("#{code};", code = u32::from(ch)));
                    }
                }
            }
            self.lines.push(format!("    n{next}[\"{kind} · {safe}\"]"));
            next
        })
    }

    fn entry(&mut self, entry: &EntryReport, changed: &BTreeSet<&str>) {
        let target = self.node("Selected", &entry.name);
        let side = entry.revision.as_deref();
        let mut previous = None;
        for (i, path) in entry.chain.iter().rev().enumerate() {
            let kind = if changed.contains(path.as_str()) { "Changed" } else { "File" };
            let node = if i == CHAIN_FILES && entry.chain.len() > CHAIN_FILES + 1 {
                self.node("Path", &format!("{} more steps to {}", entry.chain.len() - i, entry.name))
            } else {
                self.node(kind, path)
            };
            if let Some(from) = previous {
                let traced = entry.chain.iter().rev().nth(i - 1).is_some_and(|p| entry.trace_steps.contains(p));
                let label = match (side, traced) {
                    (Some("base"), true) => "base trace",
                    (Some("head"), true) => "head trace",
                    (Some("base"), false) => "base",
                    (Some("head"), false) => "head",
                    _ => "dependency",
                };
                self.edges.insert((from, label, node));
            }
            previous = Some(node);
            if i == CHAIN_FILES && entry.chain.len() > CHAIN_FILES + 1 {
                break;
            }
        }
        let reason = match entry.reason.as_deref() {
            Some("input") => "configured input",
            Some("lockfile") => "lockfile",
            _ => "selects",
        };
        if let Some(root) = previous {
            self.edges.insert((root, reason, target));
        } else {
            let fallback = self.node("Fallback", "Incomplete analysis");
            self.edges.insert((fallback, "selects", target));
        }
        let other: Vec<_> = entry.hits.iter().filter(|p| Some(*p) != entry.chain.last()).collect();
        for path in other.iter().take(EXTRA_FILES) {
            let node = self.node("Changed", path);
            self.edges.insert((node, "also affects", target));
        }
        if other.len() > EXTRA_FILES {
            let node = self.node("Changes", &format!("{} more for {}", other.len() - EXTRA_FILES, entry.name));
            self.edges.insert((node, "also affects", target));
        }
    }
}

impl Report {
    /// Fenced Mermaid for a PR, showing selected entries and the evidence reported for them.
    #[must_use]
    pub fn mermaid(&self) -> String {
        let mut diagram = Diagram::default();
        let changed = self.changed.iter().map(String::as_str).collect();
        let mut selected: Vec<_> = self.entries.iter().filter(|e| e.selected).collect();
        selected.sort_by(|a, b| a.name.cmp(&b.name));
        for entry in selected.iter().take(ENTRIES) {
            diagram.entry(entry, &changed);
        }
        if selected.len() > ENTRIES {
            diagram.node("Selected", &format!("{} additional entries (see JSON)", selected.len() - ENTRIES));
        }
        let reached: BTreeSet<_> = self.entries.iter().flat_map(|e| &e.hits).collect();
        let unmatched: Vec<_> = self.changed.iter().filter(|p| !reached.contains(p)).collect();
        if !unmatched.is_empty() {
            let group = diagram.node("Changes", "No known consumer");
            for path in unmatched.iter().take(EXTRA_FILES) {
                let file = diagram.node("Changed", path);
                diagram.edges.insert((file, "unmatched", group));
            }
            if unmatched.len() > EXTRA_FILES {
                let more = diagram.node("Changes", &format!("{} more unmatched files", unmatched.len() - EXTRA_FILES));
                diagram.edges.insert((more, "unmatched", group));
            }
        }
        if diagram.lines.is_empty() {
            diagram.node("Changes", "Empty diff");
        }
        let mut lines = vec!["```mermaid".to_owned(), "flowchart LR".to_owned()];
        lines.extend(diagram.lines);
        lines.extend(diagram.edges.iter().map(|(a, label, b)| format!("    n{a} -->|{label}| n{b}")));
        lines.push("```".to_owned());
        lines.join("\n")
    }
}
