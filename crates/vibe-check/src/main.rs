//! The `vibe-check` binary.
//!
//! Thin by design: argument parsing, diagnostics setup, and mapping the result
//! to an exit code. Everything else lives in the library so it can be tested
//! without spawning a process.

use clap::Parser;
use eyre::Result;
use vibe_check::{Cli, panic};

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    // Diagnostics go to stderr so that `--format json` on stdout stays
    // machine-readable even with `RUST_LOG` turned up.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    // After `color_eyre::install`, which installs a hook of its own and would
    // otherwise replace this one.
    panic::install();

    // Every outcome, including a panic, comes back as an exit code from the
    // documented table. Argument parsing stays outside: clap exits `2` on a bad
    // command line itself, which is the one code this binary does not choose.
    std::process::exit(i32::from(panic::run_guarded(Cli::parse()).await));
}
