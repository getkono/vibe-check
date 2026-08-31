//! Command-line surface.
//!
//! The same binary is the GitHub Action and the local tool, so "it runs locally
//! with one command" holds by construction rather than by anyone remembering to
//! keep two paths in step.
//!
//! Global options are deliberately few. Every knob added here becomes an
//! `action.yml` input, and an input list that grows means every new option needs
//! an Action release — so configuration belongs in the policy file, which is
//! versioned, reviewable, and read from the merge base.

use clap::{Parser, Subcommand, ValueEnum};
use vibe_check_model::LeafId;

/// How to render output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Human-readable, for a terminal.
    Human,
    /// `vibe-check.json`, for machines.
    Json,
}

/// Where planned work runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum SchedulerKind {
    /// Choose from the environment: a job matrix under GitHub Actions,
    /// otherwise local.
    Auto,
    /// Run everything in this process, however long it takes.
    ///
    /// The escape hatch for anyone who would rather use one large runner than a
    /// fan-out of small ones.
    Local,
}

/// Decides which evidence a pull request needs, sources it from the CI you
/// already run, and adjudicates a verdict.
#[derive(Parser, Debug)]
#[command(name = "vibe-check", version, about, long_about = None)]
pub struct Cli {
    /// Revision to compare against.
    ///
    /// Resolved as: this flag, then `VIBE_CHECK_BASE`, then the CI event
    /// payload, then `origin/HEAD`, then the upstream branch, then `master` or
    /// `main`. Whatever it resolves to, the comparison is always against the
    /// **merge base** with the head — never the base branch tip, which drifts as
    /// the base branch moves.
    #[arg(long, global = true, env = "VIBE_CHECK_BASE")]
    pub base: Option<String>,

    /// Path to the policy document.
    #[arg(long, global = true, default_value = ".vibe-check/policy.toml")]
    pub config: String,

    /// Output format.
    #[arg(long, global = true, value_enum, default_value_t = Format::Human)]
    pub format: Format,

    /// Where planned work runs.
    #[arg(long, global = true, value_enum, default_value_t = SchedulerKind::Auto)]
    pub scheduler: SchedulerKind,

    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The subcommands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Classify a diff into risk flags, without deciding anything.
    ///
    /// Pure, fast, and read-only. Useful on its own — knowing a change touches
    /// `unsafe` in a particular crate is worth something before any policy
    /// exists to act on it.
    Classify,

    /// Show which capabilities this change requires and how each would be
    /// answered.
    ///
    /// A dry run. Because capabilities only ever *describe* work rather than
    /// perform it, this costs nothing beyond classification.
    Plan,

    /// Answer one capability.
    ///
    /// Invoked per job when work has been fanned out across a matrix.
    Run {
        /// Which leaf of the plan to run.
        ///
        /// Parsed into a [`LeafId`] here rather than downstream, so a malformed
        /// identifier fails at argument parsing and names the offending
        /// character — instead of surfacing three steps later as an artifact
        /// nobody uploaded.
        ///
        /// A closure rather than `LeafId::new_checked` directly: the
        /// constructor is generic over `impl AsRef<str>`, so as a function item
        /// it is bound to one lifetime and clap needs a parser that works for
        /// any.
        #[arg(long, value_parser = |raw: &str| LeafId::new_checked(raw))]
        id: LeafId,
    },

    /// Combine evidence into a verdict and write the bundle.
    Adjudicate,

    /// Re-adjudicate a recorded bundle.
    ///
    /// Answers "why did CI say human?" without pushing a commit. Also the
    /// regression test for the adjudicator: a change that alters a historical
    /// verdict has to be deliberate.
    Replay {
        /// Path to a previously written bundle.
        bundle: String,
    },

    /// Probe the repository and write a draft policy.
    ///
    /// Adoption-first: reports what can be answered by CI that already exists,
    /// what would have to be run, and what is being declared not applicable and
    /// why. Prints the changes it wants to make rather than making them.
    Init,

    /// Record a defect against the merge that introduced it.
    ///
    /// Turns tier boundaries from a guess into something measured.
    Escape {
        /// The merge commit.
        merge_sha: String,
        /// Which risk category the defect belongs to.
        #[arg(long)]
        category: String,
    },

    /// Print a JSON Schema.
    Schema {
        /// Which document: `bundle` or `evidence`.
        document: String,
    },
}

impl Command {
    /// A stable name for logs and metrics.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Classify => "classify",
            Self::Plan => "plan",
            Self::Run { .. } => "run",
            Self::Adjudicate => "adjudicate",
            Self::Replay { .. } => "replay",
            Self::Init => "init",
            Self::Escape { .. } => "escape",
            Self::Schema { .. } => "schema",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_definition_is_valid() {
        // clap validates argument definitions at runtime, so without this an
        // inconsistency would only surface when a user hit that subcommand.
        Cli::command().debug_assert();
    }

    #[test]
    fn defaults_match_the_documented_behaviour() {
        let cli = Cli::try_parse_from(["vibe-check", "classify"]).expect("parses");
        assert_eq!(cli.format, Format::Human);
        assert_eq!(cli.scheduler, SchedulerKind::Auto);
        assert_eq!(cli.config, ".vibe-check/policy.toml");
        assert!(cli.base.is_none());
    }

    #[test]
    fn global_options_are_accepted_after_the_subcommand() {
        // `vibe-check classify --base master` is what people actually type;
        // requiring the flag before the subcommand would be a papercut on every
        // single invocation.
        let cli =
            Cli::try_parse_from(["vibe-check", "classify", "--base", "master"]).expect("parses");
        assert_eq!(cli.base.as_deref(), Some("master"));
    }

    #[test]
    fn scheduler_can_be_forced_local() {
        let cli =
            Cli::try_parse_from(["vibe-check", "plan", "--scheduler", "local"]).expect("parses");
        assert_eq!(cli.scheduler, SchedulerKind::Local);
    }

    #[test]
    fn run_requires_a_leaf_identifier() {
        // Running "whatever" is not a meaningful request: each leaf is one
        // capability against one scope.
        assert!(Cli::try_parse_from(["vibe-check", "run"]).is_err());
        let cli =
            Cli::try_parse_from(["vibe-check", "run", "--id", "miri-core-0"]).expect("parses");
        assert!(matches!(cli.command, Command::Run { id } if id.as_str() == "miri-core-0"));
    }

    #[test]
    fn a_malformed_leaf_identifier_is_rejected_at_the_boundary() {
        // The id is interpolated into an artifact name and a shell command, so
        // the last useful place to reject one is before the process does any
        // work with it.
        for bad in ["../../etc/passwd", "Miri-Core", "a b", "$(id)"] {
            assert!(
                Cli::try_parse_from(["vibe-check", "run", "--id", bad]).is_err(),
                "{bad:?} must not parse as a leaf id"
            );
        }
    }

    #[test]
    fn escape_requires_a_category() {
        // A defect with no category cannot be attributed to a risk flag, and an
        // unattributed defect tells the tier statistics nothing.
        assert!(Cli::try_parse_from(["vibe-check", "escape", "9f3c"]).is_err());
    }

    #[test]
    fn command_names_are_stable() {
        let cli = Cli::try_parse_from(["vibe-check", "adjudicate"]).expect("parses");
        assert_eq!(cli.command.name(), "adjudicate");
    }
}
