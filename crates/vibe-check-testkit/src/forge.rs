//! An in-memory forge that records what it was asked to change.
//!
//! Most tests care about *what vibe-check decided to do*, not about how an HTTP
//! client behaves. [`FakeForge`] answers reads from values you hand it and
//! records writes into a list you can assert on or snapshot.
//!
//! It deliberately does not exercise authentication, pagination, retry, or
//! rate-limit handling — that is real client behaviour and needs a real client
//! pointed at a local HTTP server. Two layers, each testing what it can actually
//! see.

use std::sync::Mutex;

use async_trait::async_trait;
use vibe_check_host::forge::{
    Artifact, ArtifactMeta, CheckRequest, CheckRun, CommentId, CommentMarker, ForgeError,
    ForgeRead, ForgeResult, ForgeWrite, PullRequest, RunRef,
};

/// Something the code under test tried to change.
///
/// Recorded rather than performed, so a test can assert that a comment would be
/// upserted exactly once without anything leaving the process.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Mutation {
    /// A pull-request comment was created or updated.
    UpsertComment {
        /// Which pull request.
        number: u64,
        /// The marker identifying our comment.
        marker: String,
        /// The rendered body.
        body: String,
    },
    /// A check run was created or updated.
    UpsertCheckRun {
        /// Check name.
        name: String,
        /// Commit it applies to.
        head_sha: String,
        /// One-line summary.
        title: String,
    },
}

/// Canned responses plus a mutation log.
#[derive(Debug, Default)]
pub struct FakeForge {
    pull_request: Option<PullRequest>,
    check_runs: Vec<CheckRun>,
    runs: Vec<RunRef>,
    artifacts: Vec<(RunRef, ArtifactMeta)>,
    downloads: Vec<(u64, Vec<u8>)>,
    mutations: Mutex<Vec<Mutation>>,
}

impl FakeForge {
    /// An empty forge: reads find nothing, writes are recorded.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the pull request reads will return.
    #[must_use]
    pub fn with_pull_request(mut self, pr: PullRequest) -> Self {
        self.pull_request = Some(pr);
        self
    }

    /// Add a check run.
    #[must_use]
    pub fn with_check_run(mut self, run: CheckRun) -> Self {
        self.check_runs.push(run);
        self
    }

    /// Add an artifact belonging to a run, with the bytes it downloads to.
    #[must_use]
    pub fn with_artifact(mut self, run: RunRef, meta: ArtifactMeta, bytes: Vec<u8>) -> Self {
        if !self.runs.contains(&run) {
            self.runs.push(run);
        }
        self.downloads.push((meta.id, bytes));
        self.artifacts.push((run, meta));
        self
    }

    /// Everything the code under test tried to change, in order.
    ///
    /// # Panics
    /// Panics if the lock was poisoned by a panic in another test thread.
    #[must_use]
    pub fn mutations(&self) -> Vec<Mutation> {
        self.mutations.lock().expect("mutation log lock").clone()
    }

    fn record(&self, mutation: Mutation) {
        self.mutations
            .lock()
            .expect("mutation log lock")
            .push(mutation);
    }
}

#[async_trait]
impl ForgeRead for FakeForge {
    async fn pull_request(&self, number: u64) -> ForgeResult<PullRequest> {
        self.pull_request
            .clone()
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {number}")))
    }

    async fn check_runs(&self, _head_sha: &str) -> ForgeResult<Vec<CheckRun>> {
        Ok(self.check_runs.clone())
    }

    async fn workflow_runs(&self, _head_sha: &str) -> ForgeResult<Vec<RunRef>> {
        Ok(self.runs.clone())
    }

    async fn artifacts(&self, run: RunRef) -> ForgeResult<Vec<ArtifactMeta>> {
        Ok(self
            .artifacts
            .iter()
            .filter(|(r, _)| *r == run)
            .map(|(_, meta)| meta.clone())
            .collect())
    }

    async fn download(&self, meta: &ArtifactMeta) -> ForgeResult<Artifact> {
        let bytes = self
            .downloads
            .iter()
            .find(|(id, _)| *id == meta.id)
            .map(|(_, bytes)| bytes.clone())
            .ok_or_else(|| ForgeError::NotFound(format!("artifact {}", meta.id)))?;
        // Hash what we actually received, not what the metadata claimed. The
        // real implementation does the same, and tests should exercise the same
        // shape.
        let digest = format!("sha256:{:016x}", fnv1a(&bytes));
        Ok(Artifact::new(meta.clone(), bytes, digest))
    }
}

#[async_trait]
impl ForgeWrite for FakeForge {
    async fn upsert_comment(
        &self,
        number: u64,
        marker: &CommentMarker,
        body: &str,
    ) -> ForgeResult<CommentId> {
        self.record(Mutation::UpsertComment {
            number,
            marker: marker.0.clone(),
            body: body.to_owned(),
        });
        Ok(CommentId(1))
    }

    async fn upsert_check_run(&self, request: CheckRequest) -> ForgeResult<u64> {
        self.record(Mutation::UpsertCheckRun {
            name: request.name,
            head_sha: request.head_sha,
            title: request.title,
        });
        Ok(1)
    }
}

/// A small non-cryptographic hash, so the fake needs no digest dependency.
///
/// Only ever used for fake artifact digests; nothing security-relevant.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibe_check_host::forge::{CheckConclusion, NullForge};

    fn artifact_meta(id: u64) -> ArtifactMeta {
        ArtifactMeta {
            id,
            name: "vibe-check-evidence".into(),
            size_bytes: 3,
            expired: false,
            run: RunRef { id: 7, attempt: 1 },
            head_sha: "9f3c".into(),
            head_repo: None,
            created_at: jiff::Timestamp::UNIX_EPOCH,
            digest: None,
        }
    }

    #[tokio::test]
    async fn records_writes_without_performing_them() {
        let forge = FakeForge::new();
        let marker = CommentMarker::default();
        forge
            .upsert_comment(412, &marker, "T2 · human")
            .await
            .expect("upsert");
        assert_eq!(
            forge.mutations(),
            [Mutation::UpsertComment {
                number: 412,
                marker: marker.0,
                body: "T2 · human".into(),
            }]
        );
    }

    #[tokio::test]
    async fn a_comment_is_upserted_not_appended() {
        // Three pushes must leave one comment's worth of intent, all against the
        // same marker. A tool that appends trains people to mute the thread.
        let forge = FakeForge::new();
        let marker = CommentMarker::default();
        for body in ["first", "second", "third"] {
            forge
                .upsert_comment(412, &marker, body)
                .await
                .expect("upsert");
        }
        let markers: Vec<_> = forge
            .mutations()
            .into_iter()
            .map(|m| match m {
                Mutation::UpsertComment { marker, .. } => marker,
                Mutation::UpsertCheckRun { .. } => unreachable!("only comments here"),
            })
            .collect();
        assert_eq!(markers.len(), 3);
        assert!(markers.windows(2).all(|w| w[0] == w[1]));
    }

    #[tokio::test]
    async fn downloads_hash_the_bytes_actually_received() {
        let forge = FakeForge::new().with_artifact(
            RunRef { id: 7, attempt: 1 },
            artifact_meta(1),
            b"<testsuite/>".to_vec(),
        );
        let artifact = forge.download(&artifact_meta(1)).await.expect("download");
        assert_eq!(artifact.bytes(), b"<testsuite/>");
        assert!(artifact.sha256().starts_with("sha256:"));
    }

    #[tokio::test]
    async fn a_check_run_conclusion_carries_no_bytes() {
        // Documents the property that keeps adoption honest: you can read a
        // successful check run and still have nothing to parse. There is no
        // method here that turns one into an artifact.
        let forge = FakeForge::new().with_check_run(CheckRun {
            id: 1,
            name: "quality".into(),
            conclusion: CheckConclusion::Success,
            html_url: None,
        });
        let runs = forge.check_runs("9f3c").await.expect("check runs");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].conclusion, CheckConclusion::Success);
        assert!(
            forge
                .artifacts(RunRef { id: 7, attempt: 1 })
                .await
                .expect("artifacts")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn the_null_forge_reports_unavailable_rather_than_failing_loudly() {
        // Local mode. The adoption layer maps this to an unverified capability,
        // so there is one code path rather than two.
        let err = NullForge.pull_request(1).await.expect_err("unavailable");
        assert!(matches!(err, ForgeError::Unavailable(_)));
    }
}
