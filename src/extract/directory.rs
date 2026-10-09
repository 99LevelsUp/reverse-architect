//! A plain directory as a [`Source`] (U15, R15/R16).
//!
//! The source for a tree that has no commits: a decompiled project, an unpacked
//! archive, a vendored drop. Before this existed the only way to index one was to
//! `git init` it, which asked the operator to modify the thing they wanted
//! described.
//!
//! Two properties are load-bearing and neither is free the way they were free
//! from the git index.
//!
//! **The enumeration must not vary with the machine.** Git gave that for free:
//! the index is the same list on every developer's checkout. A filesystem walk
//! is not, so this one consults **in-tree `.gitignore` files only, rooted at the
//! source root**, and explicitly disables every per-machine ignore source —
//! `core.excludesFile`, `$XDG_CONFIG_HOME/git/ignore`, `.git/info/exclude`, and
//! ignore files in directories *above* the root. Each of those four is a
//! function of the operator's home directory, and a corpus that varied with it
//! would not be the deterministic artifact the whole project claims to produce.
//! The ripgrep-only `.ignore` convention is disabled too: it is not a git
//! concept, so honoring it would make the corpus depend on which tools the
//! operator happens to run.
//!
//! Respecting the in-tree files is not only about determinism. Ignored paths are
//! where secrets live — a `.env`, a vendored credential, a local settings file —
//! and a bare walk would carry them past [`SecretPrefixes`](super::external) and
//! into the corpus. A tree with no ignore file enumerates everything, which is
//! the honest default: nothing was declared, so nothing is hidden.
//!
//! One condition on the equivalence this buys, stated because it is easy to
//! over-read: a tree indexed as git and as a directory produces the same corpus
//! **when the index bytes equal the disk bytes**. They usually do; they do not
//! under end-of-line normalization — `core.autocrlf`, or an `eol` attribute in
//! `.gitattributes` — where the index holds LF and the working tree holds CRLF,
//! so the two sources hash one file differently. That is not a defect here but
//! the honest limit of the source: KTD10's independence from autocrlf comes from
//! reading an immutable index blob, and a tree with no index has none to read.
//!
//! **Dotfiles ARE enumerated.** The walker's default hides them, and taking that
//! default would silently drop `.github/`, `.editorconfig`, `.gitignore` itself —
//! files git tracks like any other and that the coverage artifact would then be
//! quietly missing.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::source::Source;
use super::{path_key_from_bytes, ExtractError, TrackedFile};

/// The mode recorded for a regular file from a non-git source.
///
/// A documented constant rather than the real filesystem mode, because there is
/// no portable one: `MetadataExt::mode()` does not exist on Windows, which is
/// this project's primary platform. Nothing reads `TrackedFile.mode` at runtime
/// (its own doc comment says so), so inventing a plausible per-file value would
/// buy nothing and vary by platform.
const MODE_FILE: u32 = 0o100644;
/// The mode recorded for a symlink from a non-git source — the same value git
/// uses, so `is_symlink` derives from `mode` here exactly as it does there.
const MODE_SYMLINK: u32 = 0o120000;

/// A directory tree read from the filesystem.
///
/// `pub(crate)`, like `GitSource`: only `Repo::open`
/// constructs one, and `Repo` exposes no way to be handed a
/// [`Source`](super::source::Source), so a public type here would be nameable
/// from outside the crate and unusable there.
pub(crate) struct DirectorySource {
    root: PathBuf,
}

impl DirectorySource {
    pub(crate) fn new(root: &Path) -> Self {
        DirectorySource {
            root: root.to_path_buf(),
        }
    }

    /// The bytes git would store for `abs`, which is what its blob id is
    /// computed over: a regular file's contents, or a symlink's **target path
    /// text** (never the pointed-to file's contents).
    fn stored_bytes(&self, abs: &Path, is_symlink: bool) -> Result<Vec<u8>, ExtractError> {
        if is_symlink {
            let target = std::fs::read_link(abs).map_err(|source| ExtractError::Io {
                path: abs.to_path_buf(),
                source,
            })?;
            Ok(os_bytes(target.as_os_str()))
        } else {
            std::fs::read(abs).map_err(|source| ExtractError::Io {
                path: abs.to_path_buf(),
                source,
            })
        }
    }
}

impl Source for DirectorySource {
    /// Walk the tree, hashing each file's stored bytes into the same git blob id
    /// the file would have had under git.
    ///
    /// The hash is why this enumeration costs a full read of the tree where the
    /// git one costs none — the index already knows every blob id, a directory
    /// does not. The identity is worth the read: identical content gets an
    /// identical id whether it arrived through git or through here, so
    /// `CacheStore` keys, cache hits and every `TrackedFile.blob_id` consumer
    /// work unchanged, and a tree indexed both ways lands on the same cache
    /// entries. Content is then read a second time by `blob_bytes` when an
    /// extractor actually wants it; holding every file in memory to save that
    /// second read is not a trade this makes.
    ///
    /// An unreadable file fails the run rather than being skipped. Enumeration is
    /// the wrong place to degrade: a silently dropped file makes the corpus a
    /// function of file permissions, and the coverage artifact would report a
    /// tree that does not exist. (`walk_files` still degrades at *read* time per
    /// E4 — that is a different moment, after the file is known.)
    fn tracked_files_with_paths(&self) -> Result<Vec<TrackedFile>, ExtractError> {
        let walk = ignore::WalkBuilder::new(&self.root)
            // Everything off, then exactly one thing back on. `standard_filters`
            // bundles six defaults; naming them individually is how this stays
            // reviewable, and how a future default added to the crate cannot
            // quietly start influencing the corpus.
            .standard_filters(false)
            // The one ignore source that is a property of the TREE.
            .git_ignore(true)
            // Without this, `.gitignore` is honored only in a directory that has
            // a `.git` — which is never, here. The default is the exact opposite
            // of what a non-git source needs.
            .require_git(false)
            // The four per-machine sources, off. See the module doc.
            .git_global(false)
            .git_exclude(false)
            .parents(false)
            .ignore(false)
            // Dotfiles are files. See the module doc.
            .hidden(false)
            // A directory symlink is enumerated as a link and never descended:
            // following one enumerates files from outside the source root under
            // in-root keys, and can loop. Git cannot follow one either.
            .follow_links(false)
            .filter_entry(|entry| {
                // `.git` itself never enters: its `config` and `logs/` hold
                // credentials in shapes `SecretPrefixes` does not match.
                if entry.file_name() == OsStr::new(".git") {
                    return false;
                }
                // A nested repository's contents never enter — the plain-directory
                // analogue of the gitlink filter in `GitSource`. The depth guard
                // spells out that the root is exempt: a root with a usable `.git`
                // would have opened as a `GitSource` instead of reaching here, so
                // pruning it would be pruning the whole run. `ignore` does not in
                // fact invoke this closure for the root (0.4.27 `walk.rs`), which
                // makes the guard belt-and-braces rather than load-bearing — kept
                // because the exemption is a decision, not an accident of the
                // crate's traversal order.
                let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
                !(entry.depth() > 0 && is_dir && entry.path().join(".git").exists())
            })
            .build();

        let mut files = Vec::new();
        for entry in walk {
            let entry = match entry {
                Ok(entry) => entry,
                // A failure that names something OUTSIDE the root is not this
                // tree's business. `parents(false)` stops an ancestor's
                // `.gitignore` patterns being APPLIED but not its file being
                // READ: the early return in `ignore` that skips ancestors
                // entirely requires `git_ignore` to be false too, and this walk
                // needs it true. So a malformed glob in a `.gitignore` somewhere
                // above the source root — a file the operator may not own and
                // this run deliberately ignores — arrived as a hard error that
                // aborted it. That is exactly the per-machine dependency the
                // module doc says cannot happen.
                Err(err) if names_something_outside(&self.root, &err) => continue,
                Err(err) => return Err(walk_error(&self.root, err)),
            };
            // A failure to parse an ignore file the walker could still traverse
            // past arrives attached to an OK entry rather than as an `Err`. Left
            // unread it means a `.gitignore` INSIDE the tree was half-applied —
            // some patterns took effect, some did not — and the file set is
            // neither the declared one nor a loud failure. Since the whole point
            // of consulting in-tree ignore files is that they are what keeps
            // secrets out of the corpus (R22), a partially applied one is the
            // case that must not pass quietly.
            if let Some(err) = entry.error() {
                if !names_something_outside(&self.root, err) {
                    return Err(soft_walk_error(entry.path(), err));
                }
            }
            // The walk's own root is not a file in the tree, and it must be
            // skipped explicitly rather than left to the type test below. When
            // the root is itself a directory symlink — or a Windows junction —
            // its entry is a symlink, passes that test, and `strip_prefix` then
            // yields an EMPTY relative path: a `TrackedFile` with key `""` and
            // mode `0o120000`, a file that does not exist. (`ignore` does not run
            // `filter_entry` at depth 0, so the closure above cannot catch it,
            // and `walkdir`'s `follow_root_links` defaults to true, so the root
            // link is resolved and descended even though `follow_links(false)`
            // stops every link below it.)
            if entry.depth() == 0 {
                continue;
            }
            // Only the stdin entry has no file type, and this walk has no stdin.
            let Some(file_type) = entry.file_type() else {
                continue;
            };
            let is_symlink = file_type.is_symlink();
            if !is_symlink && !file_type.is_file() {
                continue;
            }
            let abs = entry.path();
            let Ok(rel) = abs.strip_prefix(&self.root) else {
                continue;
            };
            let bytes = self.stored_bytes(abs, is_symlink)?;
            let blob_id = git2::Oid::hash_object(git2::ObjectType::Blob, &bytes)?;
            files.push(TrackedFile {
                key: path_key_from_bytes(&rel_path_bytes(rel)),
                path: rel.to_path_buf(),
                mode: if is_symlink { MODE_SYMLINK } else { MODE_FILE },
                blob_id,
                is_symlink,
            });
        }
        Ok(files)
    }

    /// Read the file back off disk.
    ///
    /// Unlike the git source there is no immutable object to read from, so this
    /// is the working tree by necessity, not by choice — the determinism KTD10
    /// buys by reading index blobs is simply not available for a tree with no
    /// index. Two runs over an unchanging tree still agree; a run racing an
    /// editor does not, and no source that reads a live directory can promise
    /// otherwise.
    ///
    /// What it can promise is that the disagreement is never silent:
    /// [`Repo::blob_bytes`](super::Repo::blob_bytes) hashes what this returns and
    /// refuses it when the file no longer matches the id enumeration recorded, so
    /// a file rewritten mid-run is dropped rather than parsed and cached under
    /// the wrong id.
    fn blob_bytes(&self, tf: &TrackedFile) -> Result<Vec<u8>, ExtractError> {
        self.stored_bytes(&self.root.join(&tf.path), tf.is_symlink)
    }

    /// `None`, always: a directory is not a snapshot of any commit. This is the
    /// established honest answer, the same one a repository with no commits gives
    /// (R6) — reported as `null` rather than omitted, so a consumer can tell
    /// "no revision" from "the field was not written".
    fn head_revision(&self) -> Option<String> {
        None
    }
}

/// Turn a walk failure into a readable [`ExtractError::Io`], keeping the failing
/// path when the walker named one and the root otherwise.
///
/// The original `Display` text is preserved for the cases that carry no
/// `io::Error` at all (a malformed glob in a `.gitignore`, a symlink loop), so
/// the operator is told what actually went wrong rather than "directory walk
/// failed".
/// Whether a walk failure is about a path that is not under `root`.
///
/// The only such failures this walk can produce come from ignore files in
/// ancestor directories, which `ignore` reads even with `parents(false)` because
/// `git_ignore(true)` keeps the ancestor pass alive. Their patterns are not
/// applied, so their errors must not fail the run either — otherwise a file in
/// the operator's home directory decides whether an unrelated tree can be
/// indexed.
///
/// Undecidable cases answer `false`, which fails loudly with the path in the
/// message. Dropping a failure is the choice that loses information, so it is the
/// one that has to be certain.
fn names_something_outside(root: &Path, err: &ignore::Error) -> bool {
    let Some(path) = error_path(err) else {
        return false;
    };
    if path.starts_with(root) {
        return false;
    }
    // `root` arrives from a command line and may be relative or `..`-laden while
    // the walker's ancestor paths are not, so a raw prefix test alone would call
    // an in-tree failure "outside" and silently drop it.
    match (std::fs::canonicalize(path), std::fs::canonicalize(root)) {
        (Ok(path), Ok(root)) => !path.starts_with(&root),
        _ => false,
    }
}

/// The path an `ignore::Error` names, through the wrappers it nests it in.
fn error_path(err: &ignore::Error) -> Option<&Path> {
    match err {
        ignore::Error::WithPath { path, .. } => Some(path),
        ignore::Error::WithLineNumber { err, .. } | ignore::Error::WithDepth { err, .. } => {
            error_path(err)
        }
        // A partial failure names one path only if every part agrees on it;
        // anything else is not attributable and must not be dropped.
        ignore::Error::Partial(errs) => {
            let mut paths = errs.iter().filter_map(error_path);
            let first = paths.next()?;
            paths.all(|p| p == first).then_some(first)
        }
        _ => None,
    }
}

/// The same readable [`ExtractError::Io`] as [`walk_error`], for a failure that
/// arrived by reference attached to an otherwise usable entry.
fn soft_walk_error(entry: &Path, err: &ignore::Error) -> ExtractError {
    ExtractError::Io {
        path: error_path(err).unwrap_or(entry).to_path_buf(),
        source: std::io::Error::other(err.to_string()),
    }
}

fn walk_error(root: &Path, err: ignore::Error) -> ExtractError {
    let path = match error_path(&err) {
        Some(path) => path.to_path_buf(),
        None => root.to_path_buf(),
    };
    let message = err.to_string();
    ExtractError::Io {
        path,
        source: err
            .into_io_error()
            .unwrap_or_else(|| std::io::Error::other(message)),
    }
}

/// A root-relative path as the byte string git would have stored: components
/// joined with `/`, never the platform separator.
///
/// The exact inverse of `disk_path_from_git_bytes`, and deliberately so — it is
/// what makes one file yield one graph key whether it was enumerated through git
/// or through here.
///
/// Components are joined rather than separators substituted, which is what git
/// itself does and is the reason the two agree byte for byte. It does **not**
/// protect a literal `\` in a unix filename: `path_key_from_bytes` folds `\` to
/// `/` immediately afterward, so `a\b` and `a/b` reach the same graph key
/// either way. That fold is the collision `Repo`'s total ordering makes
/// deterministic; it is not something this function can prevent, and an earlier
/// version of this comment claimed it did.
fn rel_path_bytes(rel: &Path) -> Vec<u8> {
    let mut out = Vec::new();
    for component in rel.components() {
        if !out.is_empty() {
            out.push(b'/');
        }
        out.extend_from_slice(&os_bytes(component.as_os_str()));
    }
    out
}

/// The bytes of an `OsStr`: exact on unix, where any byte sequence is a legal
/// path; a lossy UTF-8 encode on Windows, where paths are UTF-16 and the encode
/// is exact for every real path and never panics.
#[cfg(unix)]
fn os_bytes(s: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    s.as_bytes().to_vec()
}
#[cfg(not(unix))]
fn os_bytes(s: &OsStr) -> Vec<u8> {
    s.to_string_lossy().into_owned().into_bytes()
}
