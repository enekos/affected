#![allow(unsafe_code, reason = "napi-derive expands to the FFI glue Node calls into")]

use std::path::PathBuf;

use affected_core::{Entry, Options, Report, TraceEdge};
use napi::bindgen_prelude::{AsyncTask, Env, Error, Result, Status, Task};
use napi_derive::napi;

#[napi(object)]
#[derive(Debug)]
pub struct NativeEntry {
    pub name: String,
    pub paths: Vec<String>,
    pub inputs: Option<Vec<String>>,
}

#[napi(object)]
#[derive(Debug)]
pub struct NativeTraceEdge {
    pub from: String,
    pub to: String,
    pub revision: Option<String>,
}

#[napi(object)]
#[derive(Debug)]
pub struct NativeDiagnostic {
    pub file: String,
    pub expression: String,
    pub revision: String,
}

#[napi(object)]
#[derive(Debug)]
pub struct NativeOptions {
    pub cwd: String,
    pub entries: Vec<NativeEntry>,
    pub base: Option<String>,
    pub changed: Option<Vec<String>>,
    pub uncommitted: Option<bool>,
    pub types: Option<bool>,
    pub globals: Option<Vec<String>>,
    pub strict: Option<bool>,
    pub mermaid: Option<bool>,
    pub trace: Option<Vec<NativeTraceEdge>>,
}

#[napi(object)]
#[derive(Debug)]
pub struct NativeEntryReport {
    pub name: String,
    pub files: Vec<String>,
    pub hits: Vec<String>,
    pub chain: Vec<String>,
    pub diagnostics: Vec<NativeDiagnostic>,
    pub selected: bool,
    pub reason: Option<String>,
    pub revision: Option<String>,
    pub trace_steps: Vec<String>,
}

#[napi(object)]
#[derive(Debug)]
pub struct NativeReport {
    pub mermaid: Option<String>,
    pub changed: Vec<String>,
    pub entries: Vec<NativeEntryReport>,
}

impl From<NativeOptions> for Options {
    fn from(native: NativeOptions) -> Self {
        let mut options = Options::new(native.cwd)
            .uncommitted(native.uncommitted.unwrap_or(true))
            .types(native.types.unwrap_or(true))
            .globals(native.globals.unwrap_or_default());
        options.strict = native.strict.unwrap_or(false);
        options.trace = native
            .trace
            .unwrap_or_default()
            .into_iter()
            .map(|e| TraceEdge { from: e.from.into(), to: e.to.into(), revision: e.revision })
            .collect();
        for entry in native.entries {
            if let Some(inputs) = entry.inputs {
                options = options.inputs(entry.name.clone(), inputs);
            }
            options =
                options.entry(entry.name, Entry::from_paths(entry.paths.into_iter().map(PathBuf::from).collect()));
        }
        if let Some(base) = native.base {
            options = options.base(base);
        }
        if let Some(changed) = native.changed {
            options = options.changed(changed.into_iter().map(PathBuf::from).collect());
        }
        options
    }
}

impl NativeReport {
    fn from_report(report: Report, mermaid: bool) -> Self {
        Self {
            mermaid: mermaid.then(|| report.mermaid()),
            changed: report.changed,
            entries: report
                .entries
                .into_iter()
                .map(|entry| NativeEntryReport {
                    name: entry.name,
                    files: entry.files,
                    hits: entry.hits,
                    chain: entry.chain,
                    selected: entry.selected,
                    reason: entry.reason,
                    revision: entry.revision,
                    trace_steps: entry.trace_steps,
                    diagnostics: entry
                        .diagnostics
                        .into_iter()
                        .map(|d| NativeDiagnostic { file: d.file, expression: d.expression, revision: d.revision })
                        .collect(),
                })
                .collect(),
        }
    }
}

fn run(options: &Options) -> Result<Report> {
    affected_core::analyze(options).map_err(|error| Error::new(Status::GenericFailure, error.to_string()))
}

#[napi]
pub fn analyze(options: NativeOptions) -> Result<NativeReport> {
    let mermaid = options.mermaid.unwrap_or(false);
    run(&options.into()).map(|report| NativeReport::from_report(report, mermaid))
}

/// Files matching any `include` glob and no `exclude` glob, relative to `cwd`, sorted.
#[napi]
pub fn expand(cwd: String, include: Vec<String>, exclude: Vec<String>) -> Result<Vec<String>> {
    affected_core::expand(cwd.as_ref(), &include, &exclude)
        .map_err(|error| Error::new(Status::GenericFailure, error.to_string()))
}

#[derive(Debug)]
pub struct Analysis(Options, bool);

impl Task for Analysis {
    type Output = Report;
    type JsValue = NativeReport;

    fn compute(&mut self) -> Result<Report> {
        run(&self.0)
    }

    fn resolve(&mut self, _env: Env, output: Report) -> Result<NativeReport> {
        Ok(NativeReport::from_report(output, self.1))
    }
}

#[napi(ts_return_type = "Promise<NativeReport>")]
pub fn analyze_async(options: NativeOptions) -> AsyncTask<Analysis> {
    let mermaid = options.mermaid.unwrap_or(false);
    AsyncTask::new(Analysis(options.into(), mermaid))
}
