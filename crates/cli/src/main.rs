mod commands;
mod config;
mod inspect;
mod metrics_server;
#[cfg(feature = "node")]
mod node;
#[cfg(feature = "node")]
mod node_shim;
mod permissions;
mod profiles;
mod run;
mod telemetry;
mod transforms;
mod watch_mode;

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

        /// Used by the `node` shim: permissions come from VELYR_NODE_SHIM_FLAGS (none when
        /// unset), the permission flags of this command line are ignored.
        #[arg(long, hide = true)]
        node_shim: bool,

        /// Restart the script when a module it imported changes; `--watch=a,b` also restarts on
        /// changes under those paths. Flags and permissions are kept across restarts.
        #[arg(long, num_args = 0..=1, require_equals = true, value_delimiter = ',', value_name = "PATHS")]
        watch: Option<Vec<String>>,

        #[command(flatten)]
        permissions: Permissions,

        #[command(flatten)]
        inspect: inspect::InspectFlags,

        /// Arguments for the script (`process.argv`), after `--`.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Run `node_modules/.bin/<bin>` with velyr's Node compat layer.
    // clap would take `--help` meant for the bin; `velyr --help` still documents this command.
    #[command(disable_help_flag = true)]
    X {
        #[command(flatten)]
        permissions: Permissions,

        /// Name of a bin in node_modules/.bin (not a path).
        bin: String,

        /// Arguments for the bin.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Run the tests (`velyr:test`, `node:test`), one isolate per file.
    Test(commands::test::TestArgs),
    /// Benchmark `Velyr.bench(name, fn)` calls: mean, p50, p99 and ops/s.
    Bench(commands::bench::BenchArgs),
    /// Type check with the project's own TypeScript (`tsc --noEmit`).
    Check(commands::check::CheckArgs),
    /// Report which Node APIs and packages the project uses and how Velyr supports them.
    Compat(commands::compat::CompatArgs),
    /// Bundle a module, its imports and extra files into a single executable.
    Compile(commands::compile::CompileArgs),
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

        /// JS engine to run on.
        #[arg(long, value_enum, default_value_t)]
        engine: run::EngineKind,

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
    // A binary produced by `velyr compile` runs its embedded program and ignores the velyr CLI.
    if let Some(code) = commands::compile::run_embedded() {
        return code;
    }
    let cli = Cli::parse();
    telemetry::init(cli.log_format);
    let code = real_main(cli);
    telemetry::shutdown();
    code
}

fn real_main(cli: Cli) -> ExitCode {
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
            node_shim,
            watch,
            permissions,
            inspect,
            args,
        }) => match watch {
            Some(paths) => watch_mode::supervise(&file, &paths),
            None => run_script(&file, &args, node_shim, permissions, inspect, engine),
        },
        Some(Command::X {
            permissions,
            bin,
            args,
        }) => x(&bin, &args, permissions),
        Some(Command::Test(args)) => commands::test::test(args),
        Some(Command::Bench(args)) => commands::bench::bench(args),
        Some(Command::Check(args)) => commands::check::check(args),
        Some(Command::Compat(args)) => commands::compat::compat(args),
        Some(Command::Compile(args)) => commands::compile::compile(args),
        Some(Command::Serve {
            file,
            hostname,
            port,
            workers,
            engine,
            permissions,
        }) => serve(&file, &hostname, port, workers, permissions, engine),
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

fn run_script(
    file: &std::path::Path,
    args: &[String],
    node_shim: bool,
    mut permissions: Permissions,
    inspect: inspect::InspectFlags,
    engine: run::EngineKind,
) -> ExitCode {
    let inspect = match inspect::from_flags(inspect, &mut permissions) {
        Ok(inspect) => inspect,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    if !node_shim && args.is_empty() {
        return run::run(file, permissions, engine, inspect);
    }
    if inspect.is_some() {
        eprintln!("error: the inspector does not support script arguments or --node-shim yet");
        return ExitCode::FAILURE;
    }
    #[cfg(feature = "node")]
    {
        if node_shim {
            node_shim::run_shim(file, args, engine)
        } else {
            node_shim::run_with_args(file, args, permissions, engine)
        }
    }
    #[cfg(not(feature = "node"))]
    {
        let _ = (args, permissions);
        eprintln!("error: script arguments and --node-shim need the node profile");
        ExitCode::FAILURE
    }
}

#[cfg(feature = "node")]
fn x(bin: &str, args: &[String], permissions: Permissions) -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let raw = node_shim::raw_flags_of_x(&argv, args.len());
    commands::x::x(bin, args, permissions, raw)
}
#[cfg(not(feature = "node"))]
fn x(_: &str, _: &[String], _: Permissions) -> ExitCode {
    eprintln!("error: `velyr x` needs the node profile");
    ExitCode::FAILURE
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
fn serve(
    _: &std::path::Path,
    _: &str,
    _: u16,
    _: Option<Workers>,
    _: Permissions,
    _: run::EngineKind,
) -> ExitCode {
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
