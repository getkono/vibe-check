//! The `vibe-check` binary.
//!
//! Thin by design: argument parsing, diagnostics setup, and mapping the result
//! to an exit code. Everything else lives in the library so it can be tested
//! without spawning a process.

use clap::Parser;
use eyre::Result;
use vibe_check::{Cli, exit};

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

    match vibe_check::run(Cli::parse()).await {
        Ok(code) => std::process::exit(i32::from(code)),
        Err(report) => {
            // Report the failure, then exit with the reserved failure code.
            // Never 0, and never a verdict code: "we could not tell" is not a
            // verdict, and a pipeline must be able to tell the difference.
            eprintln!("{report:?}");
            std::process::exit(i32::from(exit::FAILURE));
        }
    }
}
