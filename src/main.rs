use anyhow::Result;
use clap::Parser;

use localgpt::cli::{Cli, Commands};

fn main() -> Result<()> {
    // argv[0] dispatch: if re-exec'd as "localgpt-sandbox", enter sandbox child path
    // immediately — before Tokio, Clap, or any other initialization.
    #[cfg(unix)]
    if let Some(arg0) = std::env::args_os().next()
        && arg0.to_string_lossy().ends_with("localgpt-sandbox")
    {
        localgpt::sandbox::sandbox_child_main();
    }

    let cli = Cli::parse();

    // Handle Gen mode specially — Bevy must own the main thread (no tokio runtime here)
    #[cfg(feature = "gen")]
    if let Commands::Gen(args) = cli.command {
        // Initialize logging before handing off to Bevy
        let log_level = if cli.verbose { "debug" } else { "info" };
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
            )
            .init();
        return localgpt::cli::gen3d::run(args, &cli.agent);
    }

    // Handle daemon start/restart specially - must fork BEFORE starting Tokio runtime
    #[cfg(unix)]
    if let Commands::Daemon(ref args) = cli.command {
        match args.command {
            localgpt::cli::daemon::DaemonCommands::Start { foreground: false } => {
                // Do the fork synchronously, then start Tokio in the child
                return localgpt::cli::daemon::daemonize_and_run(&cli.agent);
            }
            localgpt::cli::daemon::DaemonCommands::Restart { foreground: false } => {
                // Stop first (synchronously), then fork and start
                localgpt::cli::daemon::stop_sync()?;
                return localgpt::cli::daemon::daemonize_and_run(&cli.agent);
            }
            _ => {}
        }
    }

    // For all other commands, start the async runtime normally
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(cli))
}

async fn async_main(cli: Cli) -> Result<()> {
    // Initialize logging
    let log_level = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
        )
        .init();

    match cli.command {
        Commands::Chat(args) => localgpt::cli::chat::run(args, &cli.agent).await,
        Commands::Ask(args) => localgpt::cli::ask::run(args, &cli.agent).await,
        #[cfg(feature = "desktop")]
        Commands::Desktop(args) => localgpt::cli::desktop::run(args, &cli.agent),
        #[cfg(feature = "gen")]
        Commands::Gen(_) => unreachable!("Gen is handled before tokio runtime starts"),
        Commands::Daemon(args) => localgpt::cli::daemon::run(args, &cli.agent).await,
        Commands::Memory(args) => localgpt::cli::memory::run(args, &cli.agent).await,
        Commands::Config(args) => localgpt::cli::config::run(args).await,
        Commands::Paths => localgpt::cli::paths::run(),
        Commands::Md(args) => localgpt::cli::md::run(args).await,
        Commands::Sandbox(args) => localgpt::cli::sandbox::run(args).await,
    }
}
