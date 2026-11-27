//! Report output, byte-compatible with the Node CLI (`Affected.toString()` and `JSON.stringify`).

use affected_core::{Entry, EntryReport, Options, Report};
use serde::Serialize;

/// `affected`, `unknown` or `unaffected`, as the Node package reports it.
fn status(entry: &EntryReport) -> &'static str {
    if !entry.hits.is_empty() {
        "affected"
    } else if !entry.diagnostics.is_empty() {
        "unknown"
    } else {
        "unaffected"
    }
}

/// One line per entry: `name  ← changed file (+N more)  via a → b`.
#[must_use]
pub fn text(report: &Report) -> String {
    report.entries.iter().map(describe).collect::<Vec<_>>().join("\n")
}

fn describe(entry: &EntryReport) -> String {
    let name = &entry.name;
    if status(entry) == "unknown" {
        let selected = if entry.is_hit() { " (selected)" } else { "" };
        return match entry.diagnostics.first() {
            Some(d) => format!("{name}  unknown{selected}  {}: {} [{}]", d.file, d.expression, d.revision),
            None => format!("{name}  unknown{selected}"),
        };
    }
    if !entry.is_hit() {
        return format!("{name}  unaffected");
    }
    let Some((changed, via)) = entry.chain.split_last() else {
        return format!("{name}  ←");
    };
    let others = match entry.hits.len() {
        0 | 1 => String::new(),
        n => format!(" (+{} more)", n - 1),
    };
    let via = if via.is_empty() { String::new() } else { format!("  via {}", via.join(" → ")) };
    format!("{name}  ← {changed}{others}{via}")
}

/// The names of the hit entries, in declaration order.
pub fn hit_names(report: &Report) -> impl Iterator<Item = &str> {
    report.entries.iter().filter(|entry| entry.is_hit()).map(|entry| entry.name.as_str())
}

/// The `$GITHUB_OUTPUT` lines, as the Node CLI writes them: `affected` (JSON array of hit names),
/// `any`, `files` (the first root of each hit entry), `matrix`, `count` and `total`.
#[must_use]
pub fn github_output(report: &Report, options: &Options) -> String {
    let names: Vec<&str> = hit_names(report).collect();
    let mut files: Vec<String> = Vec::new();
    for name in &names {
        let root = options.entries.iter().find(|(entry, _)| entry == name).and_then(|(_, entry)| match entry {
            Entry::Tsconfig(path) => Some(path),
            Entry::Files(paths) => paths.first(),
            _ => None,
        });
        if let Some(root) = root.map(|path| path.to_string_lossy().replace('\\', "/"))
            && !files.contains(&root)
        {
            files.push(root);
        }
    }
    format!(
        "affected={}\nany={}\nfiles={}\nmatrix={{\"entry\":{}}}\ncount={}\ntotal={}\n",
        to_json(&names),
        !names.is_empty(),
        to_json(&files),
        to_json(&names),
        names.len(),
        report.entries.len()
    )
}

fn to_json<T: Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "[]".to_owned())
}

#[derive(Serialize)]
struct JsonReport<'a> {
    changed: &'a [String],
    entries: Vec<JsonEntry<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonEntry<'a> {
    name: &'a str,
    hit: bool,
    status: &'static str,
    diagnostics: Vec<JsonDiagnostic<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<&'a str>,
    trace_steps: &'a [String],
    hits: &'a [String],
    chain: &'a [String],
    files: &'a [String],
}

#[derive(Serialize)]
struct JsonDiagnostic<'a> {
    file: &'a str,
    expression: &'a str,
    revision: &'a str,
}

/// The report as the Node CLI's `--json` prints it: two-space indented, no trailing newline.
///
/// # Errors
///
/// Only if serialization fails, which plain strings and booleans never do.
pub fn json(report: &Report) -> serde_json::Result<String> {
    let entries = report
        .entries
        .iter()
        .map(|entry| JsonEntry {
            name: &entry.name,
            hit: entry.is_hit(),
            status: status(entry),
            diagnostics: entry
                .diagnostics
                .iter()
                .map(|d| JsonDiagnostic { file: &d.file, expression: &d.expression, revision: &d.revision })
                .collect(),
            reason: entry.reason.as_deref().filter(|reason| !reason.is_empty()),
            revision: entry.revision.as_deref().filter(|revision| !revision.is_empty()),
            trace_steps: &entry.trace_steps,
            hits: &entry.hits,
            chain: &entry.chain,
            files: &entry.files,
        })
        .collect();
    serde_json::to_string_pretty(&JsonReport { changed: &report.changed, entries })
}
