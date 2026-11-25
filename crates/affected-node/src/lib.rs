#![allow(unsafe_code, reason = "napi-derive expands to the FFI glue Node calls into")]

use std::path::PathBuf;

use affected_core::{Entry, Options, Report};
use napi::bindgen_prelude::{AsyncTask, Env, Error, Result, Status, Task};
use napi_derive::napi;

#[napi(object)]
#[derive(Debug)]
pub struct NativeEntry {
    pub name: String,
    pub paths: Vec<String>,
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
}

#[napi(object)]
#[derive(Debug)]
pub struct NativeEntryReport {
    pub name: String,
    pub files: Vec<String>,
    pub hits: Vec<String>,
    pub chain: Vec<String>,
}

#[napi(object)]
#[derive(Debug)]
pub struct NativeReport {
    pub changed: Vec<String>,
    pub entries: Vec<NativeEntryReport>,
}

impl From<NativeOptions> for Options {
    fn from(native: NativeOptions) -> Self {
        let mut options = Options::new(native.cwd)
            .uncommitted(native.uncommitted.unwrap_or(true))
            .types(native.types.unwrap_or(true));
        for entry in native.entries {
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

impl From<Report> for NativeReport {
    fn from(report: Report) -> Self {
        Self {
            changed: report.changed,
            entries: report
                .entries
                .into_iter()
                .map(|entry| NativeEntryReport {
                    name: entry.name,
                    files: entry.files,
                    hits: entry.hits,
                    chain: entry.chain,
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
    run(&options.into()).map(NativeReport::from)
}

#[derive(Debug)]
pub struct Analysis(Options);

impl Task for Analysis {
    type Output = Report;
    type JsValue = NativeReport;

    fn compute(&mut self) -> Result<Report> {
        run(&self.0)
    }

    fn resolve(&mut self, _env: Env, output: Report) -> Result<NativeReport> {
        Ok(output.into())
    }
}

#[napi(ts_return_type = "Promise<NativeReport>")]
pub fn analyze_async(options: NativeOptions) -> AsyncTask<Analysis> {
    AsyncTask::new(Analysis(options.into()))
}
