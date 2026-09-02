//! The command-line surface: clap definitions, the per-invocation context, and dispatch to handlers.

use crate::util::canonical_or_absolute;
use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::commands;
use crate::error::{AcreError, Result};
use crate::shell::SupportedShell;
use crate::ui::errors::render_failure;

#[derive(Debug, Clone)]
pub struct GlobalOptions {
    pub directory: Option<PathBuf>,
    pub json: bool,
    pub no_color: bool,
    pub plain: bool,
    pub verbose: bool,
}
#[derive(Debug, Clone)]
pub struct ShellBridge {
    pub active: bool,
    pub directive_file: Option<PathBuf>,
    pub session_id: Option<String>,
    pub pid: Option<u32>,
}
#[derive(Debug, Clone)]
pub struct CommandContext {
    pub cwd: PathBuf,
    pub interactive: bool,
    pub global: GlobalOptions,
    pub shell: ShellBridge,
}

const AFTER_HELP: &str = "\
Daily use:
  acre                  choose existing work
  acre <target>         open an existing worktree, branch, remote branch, or PR
  acre new <branch>     create a branch in a warm workspace
  acre -                return to the previous exact location
  acre done             finish with the current workspace
  acre <target> -- CMD  run a command in an existing target

Acre never runs repository setup scripts automatically and never recycles dirty or unverified work.";

#[derive(Debug, Parser)]
#[command(
    name = "acre",
    version = crate::VERSION,
    about = "Warm, reusable Git workspaces. Every branch, already ready.",
    after_help = AFTER_HELP,
)]
struct Cli {
    #[arg(short = 'C', long, global = true, value_name = "PATH")]
    directory: Option<PathBuf>,

    #[arg(long, global = true)]
    json: bool,

    #[arg(long, global = true)]
    no_color: bool,

    #[arg(long, global = true)]
    plain: bool,

    #[arg(long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Option<Command>,

    #[arg(value_name = "TARGET", allow_hyphen_values = true)]
    target: Option<String>,

    #[arg(last = true, value_name = "COMMAND")]
    program: Vec<OsString>,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(about = "Create a branch and open it in a warm workspace")]
    New(NewArgs),

    #[command(about = "Finish with a workspace and return Acre-owned infrastructure to the warm pool")]
    Done(DoneArgs),

    #[command(about = "Install shell integration and create the user configuration")]
    Setup(SetupArgs),

    #[command(hide = true)]
    Acquire(AcquireArgs),

    #[command(hide = true)]
    Release(ReleaseArgs),

    #[command(about = "Inspect and maintain Acre's local workspace runtime")]
    System(SystemArgs),

    #[command(about = "View and edit Acre configuration")]
    Config(ConfigArgs),

    #[command(hide = true)]
    Shell(ShellArgs),

    #[command(hide = true)]
    Completion(CompletionArgs),

    #[command(name = "__session-id", hide = true)]
    SessionId,

    #[command(name = "__resume", hide = true)]
    Resume { token: String },

    #[command(name = "__replenish", hide = true)]
    Replenish { common_dir: PathBuf },

    #[command(name = "__complete", hide = true)]
    Complete { token: Option<String> },
}

#[derive(Debug, Args)]
struct NewArgs {
    branch: String,
    #[arg(long, value_name = "REF")]
    from: Option<String>,
    #[arg(long)]
    fresh: bool,
    #[arg(long)]
    stay: bool,
}

#[derive(Debug, Args)]
struct DoneArgs {
    target: Option<String>,
}

#[derive(Debug, Args)]
struct SetupArgs {
    #[arg(long, value_enum)]
    shell: Option<SupportedShell>,
    #[arg(short = 'y', long)]
    yes: bool,
}

#[derive(Debug, Args)]
struct AcquireArgs {
    target: String,
    #[arg(long)]
    holder: String,
    #[arg(long)]
    pid: Option<u32>,
    #[arg(long)]
    new: bool,
    #[arg(long)]
    from: Option<String>,
    #[arg(long)]
    fresh: bool,
}

#[derive(Debug, Args)]
struct ReleaseArgs {
    #[arg(long = "lease-id")]
    lease_id: String,
    #[arg(long)]
    keep_active: bool,
}

#[derive(Debug, Args)]
struct SystemArgs {
    #[command(subcommand)]
    command: SystemCommand,
}

#[derive(Debug, Subcommand)]
enum SystemCommand {
    Warm {
        #[arg(long)]
        slots: Option<usize>,
    },
    Inspect,
    Doctor,
    Repair,
    Gc,
}

#[derive(Debug, Args)]
struct ConfigArgs {
    #[command(subcommand)]
    command: Option<ConfigCommand>,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    Show,
    Path,
    Init {
        #[arg(long)]
        force: bool,
    },
    Set {
        key: String,
        value: String,
    },
    Edit,
    RepoInit {
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Args)]
struct ShellArgs {
    #[command(subcommand)]
    command: ShellCommand,
}

#[derive(Debug, Subcommand)]
enum ShellCommand {
    Init { shell: SupportedShell },
}

#[derive(Debug, Args)]
struct CompletionArgs {
    #[arg(value_enum)]
    shell: SupportedShell,
}

pub fn run() -> i32 {
    let cli = Cli::parse();
    if cli.target.is_some() && cli.command.is_some() {
        use clap::CommandFactory;
        Cli::command()
            .error(
                clap::error::ErrorKind::ArgumentConflict,
                "a target cannot be combined with a subcommand",
            )
            .exit();
    }
    let cwd = match &cli.directory {
        Some(directory) => canonical_or_absolute(directory),
        None => match std::env::current_dir() {
            Ok(cwd) => cwd,
            Err(error) => {
                let context = create_context(&cli, PathBuf::from("."));
                let error = AcreError::io("could not read current directory", error);
                return render_failure(&context, &error);
            }
        },
    };
    let context = create_context(&cli, cwd);
    match dispatch(&context, cli) {
        Ok(code) => code,
        Err(error) => render_failure(&context, &error),
    }
}

fn dispatch(context: &CommandContext, cli: Cli) -> Result<i32> {
    match cli.command {
        None => commands::open::run(context, cli.target.as_deref(), &cli.program),
        Some(Command::New(args)) => {
            commands::new::run(context, &args.branch, args.from.as_deref(), args.fresh, args.stay)
        }
        Some(Command::Done(args)) => commands::done::run(context, args.target.as_deref()),
        Some(Command::Setup(args)) => commands::setup::run(context, args.shell, args.yes),
        Some(Command::Acquire(args)) => commands::machine::acquire(
            context,
            &args.target,
            &args.holder,
            args.pid,
            args.new,
            args.from.as_deref(),
            args.fresh,
        ),
        Some(Command::Release(args)) => commands::machine::release(context, &args.lease_id, args.keep_active),
        Some(Command::System(args)) => match args.command {
            SystemCommand::Warm { slots } => commands::system::warm(context, slots),
            SystemCommand::Inspect => commands::system::inspect(context),
            SystemCommand::Doctor => commands::system::doctor(context),
            SystemCommand::Repair => commands::system::repair(context),
            SystemCommand::Gc => commands::system::gc(context),
        },
        Some(Command::Config(args)) => match args.command.unwrap_or(ConfigCommand::Show) {
            ConfigCommand::Show => commands::config::show(context),
            ConfigCommand::Path => commands::config::path(context),
            ConfigCommand::Init { force } => commands::config::init(context, force),
            ConfigCommand::Set { key, value } => commands::config::set(context, &key, &value),
            ConfigCommand::Edit => commands::config::edit(context),
            ConfigCommand::RepoInit { force } => commands::config::repo_init(context, force),
        },
        Some(Command::Shell(args)) => match args.command {
            ShellCommand::Init { shell } => commands::shell::init(context, shell),
        },
        Some(Command::Completion(args)) => commands::shell::completion(context, args.shell),
        Some(Command::SessionId) => commands::internal::session_id(context),
        Some(Command::Resume { token }) => commands::internal::resume(context, &token),
        Some(Command::Replenish { common_dir }) => commands::internal::replenish(&common_dir),
        Some(Command::Complete { token }) => {
            commands::internal::complete(context, token.as_deref().unwrap_or(""))
        }
    }
}

fn create_context(cli: &Cli, cwd: PathBuf) -> CommandContext {
    let session_id = std::env::var("ACRE_SHELL_SESSION_ID")
        .ok()
        .filter(|value| !value.is_empty());
    let directive_file = std::env::var_os("ACRE_DIRECTIVE_FILE").map(PathBuf::from);
    let pid = std::env::var("ACRE_SHELL_PID")
        .ok()
        .and_then(|value| value.parse().ok());
    CommandContext {
        cwd,
        interactive: !cli.json
            && std::io::IsTerminal::is_terminal(&std::io::stdin())
            && std::io::IsTerminal::is_terminal(&std::io::stdout()),
        global: GlobalOptions {
            directory: cli.directory.clone(),
            json: cli.json,
            no_color: cli.no_color,
            plain: cli.plain,
            verbose: cli.verbose,
        },
        shell: ShellBridge {
            active: session_id.is_some() && directive_file.is_some(),
            directive_file,
            session_id,
            pid,
        },
    }
}
