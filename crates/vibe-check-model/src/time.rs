//! The one instant a decision is allowed to depend on.
//!
//! Determinism has a clause that types cannot usually carry: *time-dependent
//! decisions read the head commit's committer date, never the wall clock*. Said
//! that way it is a rule someone has to remember, and the failure mode when
//! they do not is silent — a waiver that was live when the pull request was
//! opened is dead when CI re-runs it a month later, and nobody can reproduce
//! last week's verdict.
//!
//! [`DecisionTime`] makes it a property of the type instead. It is the only
//! shape a time-dependent decision's input may take, and there is no way to
//! build one out of the current moment.
//!
//! `clippy.toml` bans `Timestamp::now` and `Zoned::now` alongside their `std`
//! equivalents, and `vibe-check-host`'s `clock` module holds the workspace's
//! single sanctioned exception, for display fields that are on the digest's
//! exclusion list. This module is the other half of that argument: the lint
//! stops you *reading* the wall clock here, and the type stops a clock read
//! elsewhere from arriving here wearing the right name.

use jiff::{Timestamp, civil::Date, tz::TimeZone};

/// The instant every time-dependent decision is made against.
///
/// Constructible only from a commit's committer date, via
/// [`from_committer_date`](DecisionTime::from_committer_date).
///
/// # What is deliberately missing
///
/// The obvious conveniences are absent on purpose, and each of them is a way a
/// wall clock gets into a verdict:
///
/// - **No `now()`.** It would make "the current moment" a decision time, which
///   is precisely the thing that cannot be replayed. `clippy.toml`'s ban on
///   `Timestamp::now` stops one being written here today; the absence of the
///   method stops one being *reachable* from here at all.
/// - **No `Default`.** A default would be either the epoch or the current
///   moment. The first silently expires every waiver ever written; the second
///   is `now()` under another name. Neither is a defensible answer to "when is
///   this decision being made", and a caller who has no committer date has a
///   missing input, not a default one — it escalates instead.
/// - **No `From<Timestamp>`.** A blanket conversion accepts *any* timestamp,
///   including one that came from the clock a moment ago, and it does it
///   implicitly at a call site nobody reviews. The single named constructor
///   forces the provenance to be written down.
/// - **No `Serialize`/`Deserialize`.** Deserializing one would let a decision
///   time arrive from a document rather than from the repository's history.
///
/// If one of these appears useful, the thing that is actually needed is a
/// committer date threaded further down — not a shorter path to a clock.
///
/// # Why the accessor is UTC
///
/// [`utc_date`](DecisionTime::utc_date) pins the civil date to UTC rather than
/// to the runner's local zone, and `clippy.toml` bans `TimeZone::system` so
/// that the local zone cannot be reached in one hop from a raw timestamp
/// either. A policy's `expires: 2027-01-01` must mean the
/// same day on every machine that evaluates it; a runner whose `TZ` is
/// `Pacific/Kiritimati` would otherwise reach that date fourteen hours before
/// one in `UTC` — a head commit at `2026-12-31T11:30:00Z` is already on
/// `2027-01-01` there — and the same pull request would get two different
/// verdicts depending on which runner picked it up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DecisionTime(Timestamp);

impl DecisionTime {
    /// The decision time of an evaluation, taken from the head commit's
    /// committer date.
    ///
    /// Named for its one legitimate source rather than for its argument type,
    /// so that a call site passing anything else reads wrongly. The caller is
    /// `vibe-check-host`'s `Vcs::committer_date`; a caller that cannot obtain
    /// one has an unanswered question and must escalate rather than substitute
    /// a time of its own.
    #[must_use]
    pub const fn from_committer_date(at: Timestamp) -> Self {
        Self(at)
    }

    /// The UTC civil date of this decision time.
    ///
    /// Pinned to UTC, not local: a runner's `TZ` must not be able to move a
    /// waiver's expiry by a day. See the type's doc comment.
    #[must_use]
    pub fn utc_date(self) -> Date {
        self.0.to_zoned(TimeZone::UTC).date()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::{civil::date, tz::Offset};

    #[test]
    fn a_decision_time_keeps_the_committer_date_it_was_given() {
        let at: Timestamp = "2026-03-04T05:06:07Z".parse().unwrap();

        assert_eq!(
            DecisionTime::from_committer_date(at).utc_date(),
            date(2026, 3, 4)
        );
    }

    #[test]
    fn the_civil_date_is_utc_and_not_the_local_zone() {
        // 11:30 UTC is already the 5th at UTC+13, the offset a runner in New
        // Zealand reports. Whichever runner asks, the decision date is the UTC
        // one, because a waiver may not expire a day early on one machine.
        //
        // A fixed offset rather than a named zone, so the test does not depend
        // on the runner having a tzdb installed.
        let at: Timestamp = "2026-03-04T11:30:00Z".parse().unwrap();
        let decision = DecisionTime::from_committer_date(at);
        let far_east = TimeZone::fixed(Offset::constant(13));

        assert_eq!(decision.utc_date(), date(2026, 3, 4));
        assert_ne!(
            decision.utc_date(),
            at.to_zoned(far_east).date(),
            "the fixture must actually straddle a day boundary, or this test \
             proves nothing about the zone the accessor uses"
        );
    }

    #[test]
    fn two_decision_times_from_the_same_commit_are_equal() {
        // The replay property in miniature: the same input yields the same
        // decision time, because nothing about the current moment reaches it.
        let at: Timestamp = "2026-03-04T05:06:07Z".parse().unwrap();

        assert_eq!(
            DecisionTime::from_committer_date(at),
            DecisionTime::from_committer_date(at)
        );
    }
}
