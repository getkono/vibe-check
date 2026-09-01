//! Process exit codes.
//!
//! These are a public interface. Scripts branch on them, CI gates on them, and
//! `mode: enforcing` is literally "do not swallow this number". Changing one is
//! a breaking change to every consumer, so the mapping lives next to the tier it
//! comes from rather than being spelled out at each call site.
//!
//! ```text
//!   0  auto              the change is sufficiently evidenced
//!  10  interface-review  a reviewer should look at the interface change
//!  20  human             a human must review
//!   1  failure           vibe-check itself could not produce a verdict
//!   2  usage             the command line was rejected before anything ran
//! ```
//!
//! The important one is `1`. A tool crash, an unreadable policy, an unreachable
//! merge base — none of these are `auto`. Reusing `0` for "we could not tell"
//! would make every outage look like a clean bill of health, which is the exact
//! failure mode vibe-check exists to prevent in other people's pipelines.
//!
//! `2` is clap's, not ours. It is documented because a consumer branching on
//! this table meets it the first time a workflow passes a flag this build does
//! not have, and an undocumented code is one a script cannot branch on. Nothing
//! in this crate produces it: argument parsing happens in the binaries, before
//! [`crate::run`] is reached.
//!
//! # A panic is `1`, and its bundle still says `human`
//!
//! A panic used to leave through Rust's default handler as `101`, a code this
//! table has never described. It is now caught — see [`crate::panic`] — and
//! reported as `1` with a minimal bundle whose single escalation is
//! `internal-panic` at `T2`.
//!
//! Precisely: a panic anywhere inside [`crate::run`], which is everything a
//! command actually does. The setup each binary performs first — building the
//! `color_eyre` hooks, initialising tracing, installing the panic hook, then
//! parsing arguments — runs outside the guard, and a panic there still exits
//! `101`. [`crate::panic::run_guarded`] enumerates that boundary. A consumer
//! branching on this table should read `101` the way it would read a signal:
//! vibe-check came apart before it started, and told you nothing.
//!
//! A *failed write* is not on that list. Every write the crash path performs —
//! the `color_eyre` crash report included — is fallible-and-ignored, so a full
//! disk or a closed log costs the diagnostic and nothing more. It does not
//! abort the process, which the shell would report as `134`, and which this
//! table does not describe either.
//!
//! Those two numbers are not in conflict, because they answer different
//! questions. **The exit code says whether vibe-check worked**; a crash did
//! not, so it is `1` and never `20`, or the pipeline cannot tell an outage from
//! a verdict. **The bundle says what should happen to the pull request**; a
//! crash means nothing was checked, so it is `human`, and that is what the
//! comment and the check run — which are what actually gate a merge in
//! `mode: enforcing` — carry. Reading either number as an answer to the other
//! question is the mistake this section exists to prevent.

// Every line of the panic path above is dead code under `panic = "abort"`: the
// process is gone before any handler runs, and `101` returns as the exit code
// with no bundle and no message. No profile in this workspace sets it today, so
// this is a forward guard — failing at compile time rather than letting a
// one-line profile change silently retire a documented contract.
#[cfg(panic = "abort")]
compile_error!(
    "vibe-check must be built with unwinding panics: `panic = \"abort\"` kills the \
     process before the handler in `crate::panic` can turn a crash into exit 1 and a \
     `human` bundle, so a crash would silently exit 101 again."
);

use vibe_check_model::Tier;

/// vibe-check could not produce a verdict.
pub const FAILURE: u8 = 1;

/// The exit code for a tier.
#[must_use]
pub fn for_tier(tier: Tier) -> u8 {
    tier.exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_is_distinguishable_from_every_verdict() {
        // If a crash exited 0, an outage would read as "everything passed".
        for tier in [Tier::T0, Tier::T1, Tier::T2] {
            assert_ne!(for_tier(tier), FAILURE);
        }
    }

    #[test]
    fn the_documented_mapping_holds() {
        assert_eq!(for_tier(Tier::T0), 0);
        assert_eq!(for_tier(Tier::T1), 10);
        assert_eq!(for_tier(Tier::T2), 20);
    }

    #[test]
    fn no_verdict_collides_with_the_codes_this_table_does_not_own() {
        // `2` is clap's and `101` is the default panic handler's. Neither may
        // ever become a tier's code: a consumer that saw `2` would read a
        // verdict as a usage error, and one that saw `101` would read a verdict
        // as a crash.
        for tier in [Tier::T0, Tier::T1, Tier::T2] {
            assert_ne!(for_tier(tier), 2);
            assert_ne!(u32::from(for_tier(tier)), 101);
        }
    }
}
