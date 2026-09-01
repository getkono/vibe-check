//! The `cargo vibe-check` shim.
//!
//! Cargo invokes `cargo-<name>` for an unknown subcommand and passes the
//! subcommand name back as `argv[1]`. Stripping it here means the argument
//! parser sees the same shape either way, so `cargo vibe-check plan` and
//! `vibe-check plan` are the same invocation rather than two code paths that
//! have to be kept in step.

use std::ffi::{OsStr, OsString};

use clap::Parser;
use color_eyre::config::HookBuilder;
use eyre::Result;
use vibe_check::{Cli, panic};

#[tokio::main]
async fn main() -> Result<()> {
    // Split rather than `color_eyre::install()`, for the reason given in
    // `main.rs`: the panic half must be rendered through a fallible write.
    let (panic_hook, eyre_hook) = HookBuilder::default().try_into_hooks()?;
    eyre_hook.install()?;
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    // The only panic hook this process installs.
    panic::install(panic_hook);

    // `args_os`, never `args`. The `String` iterator panics on a non-UTF-8
    // argument, and it is evaluated here — outside the guard — so that panic
    // would escape as 101 with no bundle, which is the exact hole this crate is
    // closing. `args_os` hands the bytes to clap, which reports a usage error
    // and exits 2 like every other malformed command line.
    let cli = Cli::parse_from(strip_cargo_subcommand(std::env::args_os()));
    std::process::exit(i32::from(panic::run_guarded(cli).await));
}

/// Drop the `vibe-check` cargo inserts as `argv[1]`.
///
/// Only when it is actually there: the binary is also runnable directly, and
/// removing the first argument unconditionally would eat a real one.
///
/// [`OsString`] rather than `String`, for the same reason `clippy.toml` bans
/// `std::path::PathBuf`: an argument that is not UTF-8 must be *rejected*, by
/// the parser, with a message — never lossily converted, and never turned into
/// a panic by the iterator that collected it. Comparing against a `&str` still
/// works, because a non-UTF-8 argument simply is not equal to one.
fn strip_cargo_subcommand(args: impl Iterator<Item = OsString>) -> Vec<OsString> {
    let mut args: Vec<OsString> = args.collect();
    if args
        .get(1)
        .is_some_and(|a| a.as_os_str() == OsStr::new("vibe-check"))
    {
        args.remove(1);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(args: &[&str]) -> Vec<OsString> {
        strip_cargo_subcommand(args.iter().map(OsString::from))
    }

    #[test]
    fn removes_the_subcommand_cargo_inserts() {
        assert_eq!(
            strip(&["cargo-vibe-check", "vibe-check", "plan"]),
            ["cargo-vibe-check", "plan"]
        );
    }

    #[test]
    fn leaves_a_direct_invocation_alone() {
        assert_eq!(
            strip(&["cargo-vibe-check", "plan"]),
            ["cargo-vibe-check", "plan"]
        );
    }

    #[test]
    fn does_not_eat_a_real_argument_that_happens_to_match() {
        // `replay vibe-check` names a file; only position 1 is cargo's.
        assert_eq!(
            strip(&["cargo-vibe-check", "replay", "vibe-check"]),
            ["cargo-vibe-check", "replay", "vibe-check"]
        );
    }

    #[test]
    fn handles_no_arguments() {
        assert_eq!(strip(&["cargo-vibe-check"]), ["cargo-vibe-check"]);
    }

    #[test]
    #[cfg(unix)]
    fn a_non_utf8_argument_passes_through_instead_of_panicking() {
        // The bug this signature exists to prevent. `std::env::args()` panics
        // on this input, and it panics *before* the guard in `vibe_check::panic`
        // can turn a crash into exit 1 and a bundle — so the byte sequence has
        // to survive as far as clap, which rejects it with a usage error.
        use std::os::unix::ffi::OsStringExt;

        let invalid = OsString::from_vec(vec![0xff]);
        let stripped = strip_cargo_subcommand(
            [
                OsString::from("cargo-vibe-check"),
                OsString::from("vibe-check"),
                invalid.clone(),
            ]
            .into_iter(),
        );

        assert_eq!(stripped, [OsString::from("cargo-vibe-check"), invalid]);
    }
}
