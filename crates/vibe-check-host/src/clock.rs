//! Reading the wall clock, in the one place that is allowed to.
//!
//! # Two clocks, and only one of them decides anything
//!
//! **Decision time** is the committer date of the head commit, read via
//! [`Vcs::committer_date`](crate::vcs::Vcs::committer_date). Waiver expiry and
//! artifact freshness compare against it, so re-evaluating a pull request from
//! last month yields the verdict it had rather than a fresh one. A rule that
//! changes its answer depending on when you ask is not a rule anybody can
//! reason about.
//!
//! **Display time** is this module. It stamps "generated at" into the bundle and
//! measures durations for the report. It cannot affect a verdict, and it does
//! not do so by being remembered on an exclusion list: `verdict_digest` names
//! the paths it covers and covers nothing else
//! (`vibe_check_model::digest::VERDICT_DIGEST_PATHS`), so a field this module
//! produces is outside the digest by construction — a new display field is out
//! by default rather than out until somebody forgets.
//!
//! The one list a display field must be *added* to is
//! `vibe_check_model::digest::BUNDLE_ID_EXCLUDED_PATHS`. `bundle_id` is a
//! content address and therefore covers everything by default, so a timestamp
//! left off it would give two identical evaluations different identifiers. No
//! such field is on the bundle yet.
//!
//! `clippy.toml` bans the wall clock workspace-wide: the types `SystemTime` and
//! `Instant`, which closes `elapsed` and `duration_since` along with `now`, and
//! the methods `Timestamp::now`, `Zoned::now`, and `TimeZone::system`, the last
//! because reading the runner's `TZ` can move a civil date by a day. This
//! module is the sanctioned exception, which is why it is this small: the
//! single `#[allow]` below is the whole of the workspace's access to the wall
//! clock.

use jiff::Timestamp;

/// Supplies display timestamps.
///
/// A trait rather than a direct call so tests can pin it and produce
/// byte-identical bundles.
pub trait Clock: Send + Sync {
    /// The current instant, for display fields only.
    fn now(&self) -> Timestamp;
}

/// The real clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    // The sanctioned wall-clock read. Everything it produces is display-only and
    // on the digest's exclusion list, per this module's own doc comment; the
    // decision clock is `Vcs::committer_date`, carried as `DecisionTime`.
    #[allow(
        clippy::disallowed_methods,
        reason = "the one sanctioned wall-clock read; see this module's doc comment"
    )]
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// A clock stopped at a fixed instant.
///
/// Used by tests and by `--frozen-time`, so snapshot comparisons do not have to
/// redact the one field that always differs.
#[derive(Clone, Copy, Debug)]
pub struct FixedClock(pub Timestamp);

impl Default for FixedClock {
    fn default() -> Self {
        Self(Timestamp::UNIX_EPOCH)
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_clock_does_not_move() {
        let clock = FixedClock::default();
        assert_eq!(clock.now(), clock.now());
        assert_eq!(clock.now(), Timestamp::UNIX_EPOCH);
    }
}
