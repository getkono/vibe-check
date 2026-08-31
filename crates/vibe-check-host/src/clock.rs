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
//! measures durations for the report. Every field it produces is on the
//! digest's exclusion list, so it cannot affect a verdict or make two identical
//! evaluations compare unequal.
//!
//! `clippy.toml` bans `SystemTime::now` and `Instant::now` workspace-wide. This
//! module is the sanctioned exception, which is why it is this small.

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
