use std::io::{self, Read, Write};
use std::process::ExitCode;

use affected_cli::{Command, Config, Context, Error, USAGE, render};

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(Error::Usage(message)) => {
            eprintln!("affected: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Error::Failed(message)) => {
            eprintln!("affected: {message}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<u8, Error> {
    let args = match affected_cli::args::parse(std::env::args_os().skip(1))? {
        Command::Help => return print(&format!("{USAGE}\n")).map(|()| 0),
        Command::Version => return print(&format!("affected {}\n", env!("CARGO_PKG_VERSION"))).map(|()| 0),
        Command::Run(args) => args,
    };
    let cwd = std::env::current_dir().map_err(|error| Error::Failed(format!("working directory: {error}")))?;
    let config = Config::find(args.config.as_deref(), &cwd).map_err(Error::Failed)?;
    let env = |name: &str| std::env::var(name).ok();
    let mut stdin = || {
        let mut text = String::new();
        io::stdin().lock().read_to_string(&mut text).map(|_| text)
    };
    let options = affected_cli::options(&args, config, Context { cwd, env: &env, stdin: &mut stdin })?;
    let report = affected_core::analyze(&options).map_err(|error| Error::Failed(error.to_string()))?;

    let output = if args.json {
        render::json(&report).map_err(|error| Error::Failed(error.to_string()))? + "\n"
    } else if args.names {
        render::hit_names(&report).flat_map(|name| [name, "\n"]).collect()
    } else {
        render::text(&report) + "\n"
    };
    print(&output)?;

    if args.github_output {
        let file = std::env::var_os("GITHUB_OUTPUT")
            .filter(|file| !file.is_empty())
            .ok_or_else(|| Error::Usage("--github-output needs $GITHUB_OUTPUT".into()))?;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)
            .and_then(|mut out| out.write_all(render::github_output(&report).as_bytes()))
            .map_err(|error| Error::Failed(format!("{}: {error}", file.to_string_lossy())))?;
    }
    let any = render::hit_names(&report).next().is_some();
    Ok(if args.exit_code && !any { 1 } else { 0 })
}

/// Writes to stdout; a closed pipe (`affected … | head`) is not an error.
fn print(text: &str) -> Result<(), Error> {
    let mut stdout = io::stdout().lock();
    match stdout.write_all(text.as_bytes()).and_then(|()| stdout.flush()) {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => Err(Error::Failed(format!("stdout: {error}"))),
        _ => Ok(()),
    }
}
