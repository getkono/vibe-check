//! Where every builtin is registered.
//!
//! **This file exists to be appended to.** Adding a capability, parser,
//! analyzer, probe, or adoption source should touch exactly two places: a new
//! file in the crate that implements it, and one line in [`builtin`]. If a
//! change ever needs to touch a third, the seam is in the wrong place and that
//! is worth fixing before the change lands.
//!
//! # Why a function and not `inventory`
//!
//! Attribute-based registration crates would remove even that one line. They
//! are the wrong tool here, for three reasons that are specific to this system
//! rather than matters of taste:
//!
//! 1. **Two registries must be able to exist at once.** Gate-integrity evaluates
//!    a pull request under the merge base's rules *and* under its own, and
//!    reports the difference. A process-global registry cannot do that.
//! 2. **The registry is hashed into every bundle.** Link order is not specified,
//!    so a link-time-collected set would need sorting anyway — and the
//!    escape-rate loop depends on that digest to know which verdicts are
//!    comparable.
//! 3. **Some capabilities come from configuration.** A capability declared in
//!    `policy.toml` cannot be registered at link time, so a second mechanism
//!    would be needed regardless, and two registration paths is worse than one
//!    slightly more verbose one.
//!
//! This is the same pattern rustc uses for lints and cargo for subcommands, for
//! the same reasons.

use std::collections::BTreeSet;

use vibe_check_model::{AnalyzerId, CapabilityId, ParserId};

/// Everything this build knows how to do.
///
/// Passed explicitly rather than reachable through a global, so that two of
/// them can coexist.
#[derive(Clone, Debug, Default)]
pub struct Registrations {
    /// Capabilities with a bespoke implementation.
    ///
    /// Most capabilities should *not* end up here: a capability that only needs
    /// to adopt an artifact and compare a threshold is expressible in
    /// configuration, and requiring a Rust release for each one is how a
    /// registry ossifies at whatever shipped in the first version.
    pub capabilities: BTreeSet<CapabilityId>,
    /// Artifact parsers.
    pub parsers: BTreeSet<ParserId>,
    /// Risk analyzers.
    pub analyzers: BTreeSet<AnalyzerId>,
}

impl Registrations {
    /// A stable digest over what is registered.
    ///
    /// Recorded in every bundle. Without it the escape-rate loop compares
    /// verdicts produced under different rules as though they were one
    /// population, which makes any tier proposal it derives unjustifiable.
    ///
    /// Ordering comes from `BTreeSet`, so the digest depends on *what* is
    /// registered and never on the order it was registered in.
    #[must_use]
    pub fn digest(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        for (label, ids) in [
            (
                "capability",
                self.capabilities
                    .iter()
                    .map(CapabilityId::as_str)
                    .collect::<Vec<_>>(),
            ),
            (
                "parser",
                self.parsers.iter().map(ParserId::as_str).collect(),
            ),
            (
                "analyzer",
                self.analyzers.iter().map(AnalyzerId::as_str).collect(),
            ),
        ] {
            for id in ids {
                hasher.update(label.as_bytes());
                hasher.update(b":");
                hasher.update(id.as_bytes());
                hasher.update(b"\n");
            }
        }
        format!("blake3:{}", hasher.finalize().to_hex())
    }

    /// Whether anything is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty() && self.parsers.is_empty() && self.analyzers.is_empty()
    }
}

/// Every builtin, assembled.
///
/// Currently empty: the capability, parser, and analyzer crates arrive with the
/// milestones that need them. The seam is here first on purpose — it is far
/// cheaper to add the first registration to a function that exists than to
/// retrofit a registry once a dozen builtins have grown their own ad-hoc wiring.
#[must_use]
pub fn builtin() -> Registrations {
    Registrations::default()
    // Append registrations here, one line each:
    //   .with_capability(TestsPass)
    //   .with_parser(NextestJunit)
    //   .with_analyzer(PublicApi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digest_is_stable_across_calls() {
        assert_eq!(builtin().digest(), builtin().digest());
    }

    #[test]
    fn the_digest_ignores_registration_order() {
        // Two builds that register the same things in different orders must
        // produce comparable verdicts, so their digests must match.
        let mut forward = Registrations::default();
        forward.capabilities.insert(CapabilityId::new("tests-pass"));
        forward
            .capabilities
            .insert(CapabilityId::new("api-diff-empty"));

        let mut backward = Registrations::default();
        backward
            .capabilities
            .insert(CapabilityId::new("api-diff-empty"));
        backward
            .capabilities
            .insert(CapabilityId::new("tests-pass"));

        assert_eq!(forward.digest(), backward.digest());
    }

    #[test]
    fn the_digest_changes_when_a_registration_changes() {
        // The whole point: a build that gained a capability must not be mistaken
        // for the build before it when attributing historical verdicts.
        let base = Registrations::default();
        let mut extended = base.clone();
        extended
            .capabilities
            .insert(CapabilityId::new("tests-pass"));
        assert_ne!(base.digest(), extended.digest());
    }

    #[test]
    fn the_digest_does_not_confuse_kinds() {
        // A parser named `x` and an analyzer named `x` are different things, and
        // a digest that conflated them would call two different builds equal.
        let mut as_parser = Registrations::default();
        as_parser.parsers.insert(ParserId::new("x"));
        let mut as_analyzer = Registrations::default();
        as_analyzer.analyzers.insert(AnalyzerId::new("x"));
        assert_ne!(as_parser.digest(), as_analyzer.digest());
    }

    #[test]
    fn nothing_is_registered_yet() {
        // Deliberate, and a reminder: this assertion should be deleted by the
        // first milestone that registers a builtin.
        assert!(builtin().is_empty());
    }
}
