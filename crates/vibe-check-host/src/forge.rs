//! Forge access, split by authority.
//!
//! # The split is the point
//!
//! [`ForgeRead`] and [`ForgeWrite`] are separate traits, and the anti-gaming
//! probes are handed a `&dyn ForgeRead`.
//!
//! A probe therefore **cannot** post a comment, update a check run, or move a
//! label — not because a policy forbids it, and not because a reviewer would
//! catch it, but because the value it holds has no such method. That is the
//! strongest form of enforcement available in a type system, and it is why the
//! split exists rather than a single `Forge` trait with a `read_only: bool`.
//!
//! # No path from a green check to evidence
//!
//! [`CheckRun`] deliberately has no conversion into [`Artifact`] and none into
//! evidence. A check named `tests` may have run a subset, excluded a feature, or
//! skipped a target entirely; its name and its conclusion are not evidence about
//! the code. [`Artifact`] cannot be constructed without bytes, so the only route
//! to evidence runs through bytes and a parser.
//!
//! Adding `impl From<CheckRun> for Artifact` would be the single most damaging
//! change available in this workspace. There is no such impl, and there must
//! never be one.
//!
//! # Forks are not a trust boundary here
//!
//! A fork pull request is adoptable. Reading the `head_repo` fields below as
//! though it were not is the mistake this section exists to prevent, because
//! that reading makes every external contribution fully unverified — and
//! therefore escalated — for no security gain whatsoever.
//!
//! For a `pull_request` event the workflow definition comes from the **base
//! branch**, and the run executes **in the base repository**. The API says so
//! directly: a workflow run reports `repository`, which is the base repository
//! and the only authority, separately from `head_repository`, which is the fork
//! the head branch lives in. Artifacts belong to the base repository and are
//! readable with `actions: read`, fork pull requests included. The base
//! repository's own CI *is* the evidence producer for a fork pull request.
//!
//! So `head_repository` is a **consistency check** — does the run being adopted
//! from describe the same head branch as the pull request in front of us — and
//! never a trust anchor. It is compared against
//! [`PullRequest::head_repo`], never against the base repository.
//!
//! The residual weakness, stated in the same breath so none of the above reads
//! as "forks are fine now": the workflow *definition* is trusted, and the *code
//! it runs* is not. A fork still chooses much of what the base repository's CI
//! compiles and executes — through `build.rs`, `.cargo/config.toml`, the body of
//! a test, or a `[patch]` in a manifest. Adoption therefore establishes
//! **provenance**, not integrity: it says which run produced these bytes, not
//! that the run measured what its name suggests. Constraining what a run is
//! permitted to answer is the gate-integrity problem, decided by policy read
//! from the merge base — never by which repository the head branch happened to
//! live in.

use async_trait::async_trait;
use jiff::Timestamp;

/// Which repository.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RepoId {
    /// Owner or organization.
    pub owner: String,
    /// Repository name.
    pub name: String,
}

impl RepoId {
    /// Parse `owner/name`.
    #[must_use]
    pub fn parse(full_name: &str) -> Option<Self> {
        let (owner, name) = full_name.split_once('/')?;
        (!owner.is_empty() && !name.is_empty() && !name.contains('/')).then(|| Self {
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    }

    /// Render as `owner/name`.
    #[must_use]
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// Identifies a workflow run and attempt.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RunRef {
    /// Run identifier.
    pub id: u64,
    /// Which attempt. Re-runs produce a new attempt against the same id, and the
    /// highest attempt wins when the same evidence arrives twice.
    pub attempt: u32,
}

/// The pull request under evaluation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PullRequest {
    /// Number.
    pub number: u64,
    /// Head commit.
    ///
    /// This is the identity every staleness check keys on. Note it is *not*
    /// `GITHUB_SHA` during a `pull_request` event, which is the synthetic merge
    /// commit.
    pub head_sha: String,
    /// The repository the head branch lives in — the fork, for a fork pull
    /// request.
    ///
    /// This is the value a candidate workflow run's `head_repository` is
    /// compared **against**: a consistency check between two descriptions of the
    /// same head branch. It is never compared against the base repository, and
    /// differing from the base repository says nothing about adoptability. See
    /// the module documentation.
    pub head_repo: Option<RepoId>,
    /// Base branch name.
    pub base_ref: String,
    /// Base branch tip *at event time*.
    ///
    /// Deliberately not used as the merge base: it is the base branch tip at an
    /// earlier moment and drifts as the base branch moves. Compute
    /// `git merge-base` instead.
    pub base_sha_at_event: String,
    /// Whether the pull request is a draft. Drafts get the cheap lane only.
    pub draft: bool,
    /// Labels, for policy that keys on them.
    pub labels: Vec<String>,
}

/// How a check run concluded.
///
/// Carried for reporting only. It is never sufficient to adopt a capability —
/// see the module documentation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum CheckConclusion {
    /// Passed.
    Success,
    /// Failed.
    Failure,
    /// Neither.
    Neutral,
    /// Cancelled.
    Cancelled,
    /// Timed out.
    TimedOut,
    /// Skipped — renders as a non-blocking green tick in most branch-protection
    /// configurations, which is exactly the failure mode vibe-check exists to
    /// surface.
    Skipped,
    /// Still running.
    Pending,
}

/// A check run on a commit.
///
/// Intentionally inert: it names a check and reports how it concluded, and there
/// is nothing you can do with it that produces evidence.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CheckRun {
    /// Check identifier.
    pub id: u64,
    /// Display name, e.g. `quality`.
    pub name: String,
    /// How it concluded.
    pub conclusion: CheckConclusion,
    /// Link for humans.
    pub html_url: Option<String>,
}

/// Metadata about an uploaded artifact, without its bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ArtifactMeta {
    /// Artifact identifier.
    pub id: u64,
    /// Name, e.g. `vibe-check-evidence-miri-core-0`.
    pub name: String,
    /// Size in bytes, as reported before download.
    pub size_bytes: u64,
    /// Whether the retention window has passed.
    pub expired: bool,
    /// The run that produced it.
    pub run: RunRef,
    /// The commit that run was for.
    ///
    /// Cross-checked against the pull-request head. An artifact that cannot be
    /// tied to this commit is not adoptable, however green the check looked.
    pub head_sha: String,
    /// The repository the *head branch* of the producing run lived in — the
    /// fork, for a fork pull request.
    ///
    /// Not the repository the producing run belonged to, and not the repository
    /// this artifact belongs to: for a `pull_request` event both of those are the
    /// **base** repository. Cross-checked against [`PullRequest::head_repo`],
    /// never against the base repository. See the module documentation.
    pub head_repo: Option<RepoId>,
    /// When it was created.
    pub created_at: Timestamp,
    /// Digest the API reports, when it reports one.
    pub digest: Option<String>,
}

/// A downloaded artifact.
///
/// Cannot be constructed without bytes. That is the whole design: every route to
/// evidence runs through content somebody can parse.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Artifact {
    meta: ArtifactMeta,
    bytes: Vec<u8>,
    sha256: String,
}

impl Artifact {
    /// Wrap downloaded bytes together with the digest computed over them.
    ///
    /// `sha256` must be computed from `bytes` by the caller that did the
    /// downloading — not copied from what the API claimed, which is a different
    /// assertion by a different party.
    #[must_use]
    pub fn new(meta: ArtifactMeta, bytes: Vec<u8>, sha256: String) -> Self {
        Self {
            meta,
            bytes,
            sha256,
        }
    }

    /// Metadata.
    #[must_use]
    pub fn meta(&self) -> &ArtifactMeta {
        &self.meta
    }

    /// The bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Digest over the bytes we actually read.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// A marker that identifies vibe-check's own pull-request comment.
///
/// Comments are *upserted*, never appended. A tool that adds a comment on every
/// push trains people to mute the thread, and a muted thread is a tool nobody
/// reads.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CommentMarker(pub String);

impl Default for CommentMarker {
    fn default() -> Self {
        Self("<!-- vibe-check -->".into())
    }
}

/// A comment identifier.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct CommentId(pub u64);

/// A check run to create or update.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CheckRequest {
    /// Check name as it appears in the UI.
    pub name: String,
    /// The commit it applies to.
    pub head_sha: String,
    /// How it concluded.
    pub conclusion: CheckConclusion,
    /// One-line summary.
    pub title: String,
    /// Markdown body.
    pub summary: String,
    /// Opaque value round-tripped through the API.
    ///
    /// Used to cache our own comment identifier, so re-runs skip listing
    /// comments entirely.
    pub external_id: Option<String>,
}

/// Why a forge operation did not happen.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ForgeError {
    /// There is no forge in this context.
    ///
    /// Returned by the local-mode implementation for every method. **Not an
    /// error at call sites**: the adoption layer maps it to an unverified
    /// capability, so local runs degrade gracefully down one code path instead
    /// of needing a parallel one.
    #[error("no forge available: {0}")]
    Unavailable(&'static str),

    /// The forge asked us to slow down.
    #[error("rate limited; retry after {}s", retry_after_secs)]
    RateLimited {
        /// Seconds to wait.
        retry_after_secs: u64,
    },

    /// The thing does not exist, or we cannot see it.
    #[error("not found: {0}")]
    NotFound(String),

    /// Anything else.
    #[error("forge request failed: {0}")]
    Request(String),
}

/// Convenience alias.
pub type ForgeResult<T> = Result<T, ForgeError>;

/// Read-only forge access.
///
/// This is all an anti-gaming probe ever receives.
#[async_trait]
pub trait ForgeRead: Send + Sync {
    /// Fetch the pull request under evaluation.
    async fn pull_request(&self, number: u64) -> ForgeResult<PullRequest>;

    /// List check runs for a commit.
    ///
    /// For reporting and for discovering which jobs exist. A conclusion here can
    /// never satisfy a capability on its own.
    async fn check_runs(&self, head_sha: &str) -> ForgeResult<Vec<CheckRun>>;

    /// List workflow runs for a commit.
    async fn workflow_runs(&self, head_sha: &str) -> ForgeResult<Vec<RunRef>>;

    /// List artifacts produced by a run.
    async fn artifacts(&self, run: RunRef) -> ForgeResult<Vec<ArtifactMeta>>;

    /// Download an artifact.
    ///
    /// The implementation computes the digest over what it actually received.
    async fn download(&self, meta: &ArtifactMeta) -> ForgeResult<Artifact>;
}

/// Read-write forge access.
///
/// Held only by the publishing step, which runs in a separate workflow with no
/// checkout and never sees pull-request code. Nothing that compiles a pull
/// request ever holds one of these.
#[async_trait]
pub trait ForgeWrite: ForgeRead {
    /// Create or update our comment, identified by `marker`.
    ///
    /// Upsert, never append.
    async fn upsert_comment(
        &self,
        number: u64,
        marker: &CommentMarker,
        body: &str,
    ) -> ForgeResult<CommentId>;

    /// Create or update a check run.
    async fn upsert_check_run(&self, request: CheckRequest) -> ForgeResult<u64>;
}

/// A forge that is not there.
///
/// Every method returns [`ForgeError::Unavailable`]. This is what local runs
/// use, and it is the reason `cargo vibe-check` needs no separate code path:
/// the adoption layer already treats `Unavailable` as "this capability is
/// unverified" rather than as a failure, so running without a token degrades to
/// a more cautious verdict instead of an error.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullForge;

const NO_FORGE: &str = "running without forge access";

#[async_trait]
impl ForgeRead for NullForge {
    async fn pull_request(&self, _number: u64) -> ForgeResult<PullRequest> {
        Err(ForgeError::Unavailable(NO_FORGE))
    }

    async fn check_runs(&self, _head_sha: &str) -> ForgeResult<Vec<CheckRun>> {
        Err(ForgeError::Unavailable(NO_FORGE))
    }

    async fn workflow_runs(&self, _head_sha: &str) -> ForgeResult<Vec<RunRef>> {
        Err(ForgeError::Unavailable(NO_FORGE))
    }

    async fn artifacts(&self, _run: RunRef) -> ForgeResult<Vec<ArtifactMeta>> {
        Err(ForgeError::Unavailable(NO_FORGE))
    }

    async fn download(&self, _meta: &ArtifactMeta) -> ForgeResult<Artifact> {
        Err(ForgeError::Unavailable(NO_FORGE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_owner_and_name() {
        let id = RepoId::parse("getkono/vibe-check").expect("parses");
        assert_eq!(id.owner, "getkono");
        assert_eq!(id.name, "vibe-check");
        assert_eq!(id.full_name(), "getkono/vibe-check");
    }

    #[test]
    fn rejects_malformed_repository_names() {
        // A repository identifier ends up in artifact-provenance comparisons, so
        // a sloppy parse here becomes a way to confuse one repository for
        // another.
        for bad in ["", "no-slash", "/name", "owner/", "a/b/c"] {
            assert!(RepoId::parse(bad).is_none(), "{bad:?} must not parse");
        }
    }

    #[test]
    fn an_artifact_cannot_exist_without_bytes() {
        // Compile-time property, asserted here as documentation: the only
        // constructor takes `Vec<u8>`. If someone adds a bytes-free constructor,
        // adoption from a bare check-run conclusion becomes expressible.
        let meta = ArtifactMeta {
            id: 1,
            name: "vibe-check-evidence".into(),
            size_bytes: 3,
            expired: false,
            run: RunRef { id: 7, attempt: 1 },
            head_sha: "9f3c".into(),
            head_repo: None,
            created_at: Timestamp::UNIX_EPOCH,
            digest: None,
        };
        let artifact = Artifact::new(meta, b"xml".to_vec(), "sha".into());
        assert_eq!(artifact.bytes(), b"xml");
    }

    #[test]
    fn the_default_comment_marker_is_an_html_comment() {
        // Invisible in the rendered comment, findable when listing comments.
        let marker = CommentMarker::default();
        assert!(marker.0.starts_with("<!--"));
        assert!(marker.0.contains("vibe-check"));
    }

    #[test]
    fn unavailable_reads_as_a_condition_not_a_crash() {
        let err = ForgeError::Unavailable("running locally without a token");
        assert!(err.to_string().contains("no forge available"));
    }
}
