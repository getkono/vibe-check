//! Process exit codes.
//!
//! These are a public interface. Scripts branch on them, CI gates on them, and
//! `mode: enforcing` is literally "do not swallow this number". Changing one is
//! a breaking change to every consumer, so the mapping lives next to the tier it
//! comes from rather than being spelled out at each call site.
//!
//! ```text
//!  0  auto              the change is sufficiently evidenced
//! 10  interface-review  a reviewer should look at the interface change
//! 20  human             a human must review
//!  1  failure           vibe-check itself could not produce a verdict
//! ```
//!
//! The important one is `1`. A tool crash, an unreadable policy, an unreachable
//! merge base — none of these are `auto`. Reusing `0` for "we could not tell"
//! would make every outage look like a clean bill of health, which is the exact
//! failure mode vibe-check exists to prevent in other people's pipelines.

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
}
