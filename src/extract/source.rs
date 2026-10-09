//! Where a run's files come from (U15).
//!
//! Until this module existed, "the source" and "a git repository" were the same
//! thing: [`Repo`](super::Repo) held a `git2::Repository` and every enumeration
//! went straight through it. That is still the default and still the best answer
//! when it is available — the git index is the shared denominator of all
//! developers (KTD10), and no filesystem walk reproduces it. But it is not the
//! *only* answer, and pretending otherwise cost real targets: a decompiled tree
//! with no commits could only be indexed after a `git init` the operator had no
//! reason to want.
//!
//! So the two questions are separated. [`Source`] is *what a run needs from a
//! source* — enumerate, read bytes, name the revision if there is one. `Repo`
//! keeps its name and its place as the first parameter of
//! [`Extractor::run`](super::Extractor::run), and holds a `Box<dyn Source>`
//! instead of a git handle. Nothing downstream of `Repo` knows which
//! implementation it got, which is exactly the property U21 needs when a
//! declared component stands in this seam.
//!
//! **Determinism lives in `Repo`, not here.** `Repo::tracked_files_with_paths`
//! sorts and dedups by graph key after calling the source, so an implementation
//! cannot forget to — see its doc comment. A `Source` returns what it found, in
//! whatever order it found it.

use super::{disk_path_from_git_bytes, path_key_from_bytes, ExtractError, TrackedFile};

/// What a run needs from whatever it is reading files out of.
///
/// Deliberately three methods, and the fourth is instructive. An earlier shape
/// had a separate keys-only enumeration on the theory that a source with a cheap
/// answer to the cheap question should be free to give one — test-pairing
/// classifies by filename stem and never touches the disk. That reasoning is
/// void now that [`Repo`](super::Repo) takes the enumeration exactly once per
/// run: asking a source twice is not a saving but a second snapshot of a tree
/// that may have moved between the two. `Repo` projects the keys out of the one
/// enumeration it already holds, and a source is never asked the same question
/// twice.
///
/// **Crate-private, deliberately.** This is the seam U21 stands a component in,
/// and calling it an extension point today would be a claim the code does not
/// support: [`Repo`](super::Repo) has no constructor that takes a `Source`, so a
/// downstream implementation would compile and then have nowhere to go. Widening
/// visibility later is a compatible change and narrowing it is not, so it starts
/// narrow and U21 widens it if and when it has a shape worth committing to.
///
/// No `Send`/`Sync` bound. `blob_bytes` is called from the serial read loop in
/// `walk_files` — only the already-read bytes cross into rayon — and
/// `git2::Repository` is `Send` but not `Sync`, so requiring `Sync` would rule
/// out the one implementation that has to work.
pub(crate) trait Source {
    /// Every file this source offers, as [`TrackedFile`]s (graph key + real
    /// relative path). Order does not matter; `Repo` imposes it.
    fn tracked_files_with_paths(&self) -> Result<Vec<TrackedFile>, ExtractError>;

    /// The bytes of one enumerated file. An implementation returns what it has
    /// now; [`Repo::blob_bytes`](super::Repo::blob_bytes) is what checks the
    /// result still hashes to `tf.blob_id`, so a source that reads a live
    /// filesystem does not have to remember to.
    fn blob_bytes(&self, tf: &TrackedFile) -> Result<Vec<u8>, ExtractError>;

    /// The revision this source is a snapshot of, when it is a snapshot of one.
    ///
    /// `None` is a legitimate answer, not a failure: a directory has no commits,
    /// and a freshly initialized repository has none yet. The coverage artifact
    /// reports it as `null` rather than omitting it (R6).
    fn head_revision(&self) -> Option<String>;
}

/// A git repository read through its **index** — the behavior that predates this
/// trait, moved rather than changed.
pub(crate) struct GitSource {
    /// Open read-only git handle (git2 / libgit2).
    ///
    /// `pub(crate)`, like [`Repo::source`](super::Repo::source) and for the same
    /// reason: only `Repo::open` constructs one, and a public handle would
    /// reopen the encapsulation making that field private just closed — a caller
    /// reaching past the trait for git-only behavior no other source can offer.
    pub(crate) git: git2::Repository,
}

impl Source for GitSource {
    /// Read from the git index — the shared denominator of all developers
    /// (KTD10). Git-ignored files were never added, so they cannot appear here,
    /// and the enumeration costs no file reads at all: the blob oid is already in
    /// the index entry.
    fn tracked_files_with_paths(&self) -> Result<Vec<TrackedFile>, ExtractError> {
        let index = self.git.index()?;
        Ok(index
            .iter()
            // Gitlinks (submodule pointers, mode 0o160000) are dropped here — the
            // single choke point. They reference another repository, hold no blob,
            // and no consumer wants them: filetree would mis-emit a File node,
            // content extractors have no bytes, and test-pairing would index a
            // bogus stem. Filtering once keeps every consumer gitlink-free.
            .filter(|entry| entry.mode != 0o160000)
            .map(|entry| TrackedFile {
                key: path_key_from_bytes(&entry.path),
                path: disk_path_from_git_bytes(&entry.path),
                mode: entry.mode,
                blob_id: entry.id,
                is_symlink: entry.mode == 0o120000,
            })
            .collect())
    }

    /// The bytes of a tracked file from its git **index blob** — not the working
    /// tree (KTD10 determinism). For a clean checkout at HEAD the blob equals the
    /// working-tree bytes, but on a dirty or autocrlf-differing checkout only the
    /// blob is stable for a given commit, so content extractors read from here.
    ///
    /// Gitlinks never reach here (filtered at enumeration). A symlink's blob is
    /// its target path text, not source content, so content extractors skip
    /// symlinks before calling this. If the oid is not a blob (corrupt index) this
    /// returns a readable error the caller degrades on (E4) — never a panic.
    fn blob_bytes(&self, tf: &TrackedFile) -> Result<Vec<u8>, ExtractError> {
        let blob = self.git.find_blob(tf.blob_id)?;
        Ok(blob.content().to_vec())
    }

    /// The commit `HEAD` points at, as a full hex id — what the coverage
    /// artifact records so a consumer can tell which checkout its claims describe
    /// (R6).
    ///
    /// `None` for a repository with no commits, which is a normal state for a
    /// fixture and for a freshly initialized project. Reported as absent rather
    /// than as an empty string, which would read like a revision.
    fn head_revision(&self) -> Option<String> {
        self.git.head().ok()?.target().map(|oid| oid.to_string())
    }
}
