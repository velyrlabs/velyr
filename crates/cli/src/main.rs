mod commands;
mod config;
mod metrics_server;
#[cfg(feature = "node")]
mod node;
mod permissions;
mod profiles;
mod run;
mod telemetry;
mod transforms;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use config::Workers;
use permissions::Permissions;
use telemetry::LogFormat;

#[derive(Parser)]
#[command(name = "velyr", version)]
struct Cli {
    /// Log output format written to stderr.
    #[arg(long, global = true, value_enum, default_value_t)]
    log_format: LogFormat,

    /// Serve Prometheus metrics at http://ADDR/metrics (for example 127.0.0.1:9464). The
    /// exporter belongs to the operator, not to the script: it needs no net-listen permission.
    #[arg(long, global = true, value_name = "ADDR")]
    metrics_addr: Option<SocketAddr>,

    /// Experimental (ADR-016 pending): isolates of this process share module sources and V8 code
    /// caches, so workers after the first skip parsing the graph.
    #[arg(long, global = true)]
    experimental_shared_code_cache: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run a JavaScript file as a classic script.
    Run {
        /// Path to the script.
        file: PathBuf,

        /// JS engine to run on.
        #[arg(long, value_enum, default_value_t)]
        engine: run::EngineKind,

        #[command(flatten)]
        permissions: Permissions,
    },
    /// Serve the module's `export default { fetch }` over HTTP.
    Serve {
        /// Path to the module.
        file: PathBuf,

        /// Address to listen on (an IP address).
        #[arg(long, default_value = "0.0.0.0")]
        hostname: String,

        /// Port to listen on; 0 picks a free one.
        #[arg(long, default_value_t = 8000)]
        port: u16,

        /// Isolates to serve with: a count or `auto` (the CPUs of the cgroup, capped by the memory
        /// budget: `quotas.memoryMb`, else the cgroup limit, else 75% of the RAM, at 128 MiB per
        /// worker). Overrides `workers` in velyr.json; the default is 1.
        #[arg(long, value_name = "N|auto")]
        workers: Option<Workers>,

        #[command(flatten)]
        permissions: Permissions,
    },
    /// Add dependencies to package.json and install them.
    Add {
        /// Packages as `name`, `name@range` or `@scope/name@range`.
        #[arg(required = true)]
        packages: Vec<String>,

        /// Save to devDependencies.
        #[arg(short = 'D', long)]
        dev: bool,

        /// Save the resolved version without a `^` prefix.
        #[arg(long)]
        exact: bool,
    },
    /// Remove dependencies from package.json and node_modules.
    Remove {
        #[arg(required = true)]
        packages: Vec<String>,
    },
    /// Install the dependencies of package.json into node_modules.
    Install {
        /// Fail when velyr.lock does not match package.json (CI).
        #[arg(long)]
        frozen: bool,

        /// Skip devDependencies.
        #[arg(long)]
        production: bool,

        /// Run the lifecycle scripts of these packages (comma separated; `.` is the project).
        #[arg(long, value_delimiter = ',')]
        allow_scripts: Vec<String>,

        /// Run the lifecycle scripts of every package.
        #[arg(long)]
        allow_all_scripts: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    telemetry::init(cli.log_format);
    if cli.experimental_shared_code_cache {
        velyr_runtime_modules::SharedCache::enable_global();
    }
    if let Some(addr) = cli.metrics_addr {
        match metrics_server::start(addr) {
            Ok(bound) => eprintln!("metrics: http://{bound}/metrics"),
            Err(err) => {
                eprintln!("error: cannot serve metrics on {addr}: {err}");
                return ExitCode::FAILURE;
            }
        }
    }
    match cli.command {
        Some(Command::Run {
            file,
            engine,
            permissions,
        }) => run::run(&file, permissions, engine),
        Some(Command::Serve {
            file,
            hostname,
            port,
            workers,
            permissions,
        }) => serve(&file, &hostname, port, workers, permissions),
        Some(Command::Add {
            packages,
            dev,
            exact,
        }) => add(&packages, dev, exact),
        Some(Command::Remove { packages }) => remove(&packages),
        Some(Command::Install {
            frozen,
            production,
            allow_scripts,
            allow_all_scripts,
        }) => install(frozen, production, allow_scripts, allow_all_scripts),
        None => ExitCode::SUCCESS,
    }
}

/// The profile a binary lacking `command` was built for, for the "not available" error.
#[cfg(not(all(feature = "web", feature = "npm")))]
fn unavailable(command: &str) -> ExitCode {
    let profile = if cfg!(feature = "web") {
        "web"
    } else {
        "minimal"
    };
    eprintln!("error: `velyr {command}` is not available in profile {profile}");
    ExitCode::FAILURE
}

#[cfg(feature = "web")]
use commands::serve::serve;
#[cfg(not(feature = "web"))]
fn serve(_: &std::path::Path, _: &str, _: u16, _: Option<Workers>, _: Permissions) -> ExitCode {
    unavailable("serve")
}

#[cfg(feature = "npm")]
use commands::{add::add, install::install, remove::remove};
#[cfg(not(feature = "npm"))]
fn add(_: &[String], _: bool, _: bool) -> ExitCode {
    unavailable("add")
}
#[cfg(not(feature = "npm"))]
fn remove(_: &[String]) -> ExitCode {
    unavailable("remove")
}
#[cfg(not(feature = "npm"))]
fn install(_: bool, _: bool, _: Vec<String>, _: bool) -> ExitCode {
    unavailable("install")
}
