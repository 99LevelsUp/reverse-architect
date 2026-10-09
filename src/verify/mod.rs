//! Model verification of ambiguous usage sites (plan A, U12, KTD12, R13–R16).
//!
//! Where no index can judge a usage site, a cheap model may: `verify` shows it
//! the code around one ambiguous site and the site's candidates, and the model
//! names the candidate the site means, or none. The answers are a LAYER beside
//! the corpus, never part of it:
//!
//! - **Separate.** `verify` writes [`LAYER_FILENAME`] into the corpus
//!   directory and touches nothing else there. Generation reads the layer only
//!   when asked (`--model-verification`, R15), and then only re-orders and marks
//!   "maybe also" claims on the cards (R14). The manifest, the graph and every
//!   confidence rung stay as the rules left them, and nothing is deleted (R16):
//!   a model's "none" is a demotion inside "maybe also", not a right to delete,
//!   which a model earns only by the measurement a rule needs (R10).
//! - **Below the index.** A site an index judged — paired with a declaration of
//!   the repository or with an external symbol — is never selected, and a
//!   layer record for one is ignored. The model stands in for the index; it is
//!   not a second opinion beside it.
//! - **A closed answer.** The model sees the candidates under short labels and
//!   may answer one label or `none`. Anything else — prose, an identifier it
//!   made up — leaves the site unverified and is counted invalid. The enrich
//!   guardrail checks narrative against facts and does not fit a choice from a
//!   closed set (KTD12).
//! - **Redacted before it leaves.** The snippet is read from the git blob, not
//!   from extracted facts, so extraction's secret filter never saw it. The whole
//!   file has every string literal's value masked ([`mask_string_literals`])
//!   and then goes through [`redact_secret_block`] with the run's configured
//!   prefixes before the window is cut — a key block or a literal that opens
//!   above the window must still be recognized — and every other field of the
//!   request goes through [`redact_secret_block`] too.
//! - **Cached by content.** A request's key is the SHA-256 of the whole
//!   canonical request: the redacted snippet, the sorted candidate identifiers
//!   with their signatures, the prompt and redaction versions, and the backend
//!   settings. An unchanged site is never asked twice; a changed snippet, a
//!   changed candidate signature or another model is a new question. Each answer
//!   is cached as soon as it is validated, so a run the backend drops halfway
//!   resumes where it stopped.
//!
//! ## Where the cache lives
//!
//! In `<corpus>/.ra-verify-cache/`, beside the extraction cache rather than
//! inside it (the plan put it in `.ra-cache`). The extraction cache wipes its
//! whole directory whenever the run's fingerprint changes, and that
//! fingerprint covers the content of every tracked file — so any commit would
//! have thrown away every model answer, and the next `verify` would have paid
//! for, and sent out, every snippet again.

use crate::card::ModelVerdicts;
use crate::config::Config;
use crate::coverage::ModelVerification;
use crate::enrich::backend::{BackendError, LlmBackend};
use crate::extract::external::{redact_secret_block, SecretPrefixes};
use crate::extract::{ExtractError, Repo, TrackedFile};
use crate::graph::{
    CandidateOutcome, Confidence, FactKind, FactValue, Graph, IndexPairing, NodeKey, SiteId,
    SiteKind, SiteRecord,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

pub mod purpose;

/// The layer `verify` writes into the corpus directory.
pub const LAYER_FILENAME: &str = "model-verification.json";

/// The answer cache, under the corpus directory (see the module docs for why
/// not under `.ra-cache`).
pub const CACHE_DIR_NAME: &str = ".ra-verify-cache";

/// The layer file's shape. Bumped when [`Layer`] changes; generation refuses a
/// layer of another schema rather than guess at it.
pub const LAYER_SCHEMA: u32 = 1;

/// The prompt's wording and the request's shape. Part of every key: a
/// re-worded question is a new question, and its old answers are stale.
pub const PROMPT_VERSION: u32 = 1;

/// What a snippet is filtered by before it leaves: the secret filter's rules,
/// as `ra-redact` holds them, and [`mask_string_literals`]. Part of every key:
/// a snippet redacted under other rules is another request, and an answer
/// cached for a snippet that leaked what the new rules catch must not be
/// served as if it had not. Bump it with every change to `ra-redact`'s pattern
/// table or its block logic, and to the literal masking.
///
/// 2: the contents of every string literal are masked.
/// 3: one tokenizer reads comments, character literals, nested literals and
///    interpolation holes with their format part, closing the leaks the
///    review rounds found in the hand lexer of version 2.
/// 4: the masking reads the original text and the secret filter its result,
///    so a redacted line no longer deletes a literal's or a comment's
///    delimiter; a preprocessor directive's text opens nothing; CR, NEL, LS
///    and PS end a line like LF.
/// 5: the pattern table knows the Czech `heslo`, `hesla` and `klíč` as an
///    assignment with a value, and SQL's `PASSWORD '…'` (plan B, U3); and,
///    within the same unreleased version, a quoted key (`"password": "…"`,
///    escaped or not), a markup element (`<Password>x`) and an application
///    setting (`key="DbPassword" value=`).
pub const REDACTION_VERSION: u32 = 5;

/// Lines of context above the site's line (KTD12).
pub const LINES_ABOVE: usize = 12;

/// Lines of context below the site's line (KTD12).
pub const LINES_BELOW: usize = 6;

/// Most bytes a snippet may have (KTD12). Lines are dropped from the end
/// farther from the site until it fits.
pub const SNIPPET_CAP: usize = 4096;

/// How the dry run turns characters into tokens: a rough rule for English
/// and code, stated beside the estimate rather than hidden in it.
pub const CHARS_PER_TOKEN: usize = 4;

/// The marker in front of the site's line in a snippet.
const SITE_MARK: &str = ">> ";
/// The indent in front of every other line, so the code keeps its shape.
const CONTEXT_MARK: &str = "   ";

/// The backend settings that are part of every request: which backend, which
/// model, which endpoint. Another model is another question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendSettings {
    /// The backend's CLI word: `ollama`, `openai-compat`, `claude-cli`.
    pub backend: String,
    pub model: Option<String>,
    pub base_url: Option<String>,
}

/// What the model said about one site, once validated.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "answer")]
pub enum Answer {
    /// The site means this candidate (its node key).
    Candidate { target: String },
    /// The site means none of its candidates.
    None,
}

/// The layer file (KTD12): the validated answers of the last complete
/// `verify` run, one per site, sorted by the site's stable id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layer {
    pub schema: u32,
    /// The prompt version the answers were given under. Informational: the
    /// keys already cover it.
    pub prompt_version: u32,
    /// The redaction rules the snippets were redacted under. Informational,
    /// as `prompt_version`.
    pub redaction_version: u32,
    /// The backend that answered. Generation needs it to recompute a site's
    /// key, which is how it tells a current record from a stale one.
    pub settings: BackendSettings,
    pub records: Vec<LayerRecord>,
}

/// One answered site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerRecord {
    /// The site's stable id (`SiteRecord`'s first six fields).
    pub file: String,
    pub owner: String,
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    pub name: String,
    pub kind: SiteKind,
    /// SHA-256 of the canonical request the answer was given to, lowercase hex.
    pub key: String,
    /// The candidates the model chose among, sorted node keys.
    pub candidates: Vec<String>,
    #[serde(flatten)]
    pub answer: Answer,
}

/// What a `verify` run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerifyReport {
    /// Ambiguous sites selected for the model.
    pub sites: u64,
    /// …whose answer the cache already held.
    pub cached: u64,
    /// …sent to the backend. Zero on a dry run.
    pub asked: u64,
    /// …whose answer was neither a candidate's label nor `none`: left
    /// unverified, and not cached, so the next run asks again.
    pub invalid: u64,
    /// Valid answers naming a candidate, cached or fresh.
    pub promoted: u64,
    /// Valid answers naming none.
    pub none: u64,
    /// The prompts' size in tokens (see [`CHARS_PER_TOKEN`]): on a dry run,
    /// of the sites the cache does not hold; on a real run, of those asked.
    pub estimated_tokens: u64,
    /// Where the layer was written. `None` on a dry run.
    pub layer: Option<PathBuf>,
}

/// Why `verify` stopped.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error(transparent)]
    Generate(#[from] crate::GenError),
    #[error(transparent)]
    Extract(#[from] ExtractError),
    #[error(
        "the backend failed after {answered} of {sites} questions; every valid answer \
         among them is cached, so a new run continues from there: {source}"
    )]
    Backend {
        /// Questions done before the failing one, valid answers and invalid
        /// alike — sites, or with `--purpose` types (plan B, U13).
        answered: u64,
        /// The questions the run had: sites, or types.
        sites: u64,
        source: BackendError,
    },
    #[error("cannot write {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Why generation could not read the layer it was asked to read.
#[derive(Debug, thiserror::Error)]
pub enum LayerError {
    #[error(
        "--model-verification was given, but there is no layer at {0}; run \
         `reverse-architect verify` over this corpus first"
    )]
    Missing(PathBuf),
    // Read, Parse and Schema serve both layers (plan B, U13): the path names
    // which one.
    #[error("cannot read the model layer {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("the model layer {path} is not valid: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error(
        "the model layer {path} has schema {found}; this build reads {wanted}. Run \
         `reverse-architect verify` (with `--purpose` for the purpose layer) again"
    )]
    Schema {
        path: PathBuf,
        found: u32,
        wanted: u32,
    },
    #[error(
        "--model-purpose was given, but there is no layer at {0}; run \
         `reverse-architect verify --purpose` over this corpus first"
    )]
    PurposeMissing(PathBuf),
    #[error(transparent)]
    Extract(#[from] ExtractError),
}

/// Ask the model about every ambiguous site of the graph `config` builds, and
/// write the layer. With `backend` `None` this is the dry run: nothing is sent
/// and nothing written, and the report says how much would be.
///
/// The graph is built exactly as generation with `config` would build it
/// ([`crate::build_graph`]), so the sites asked about are the corpus's.
pub fn run(
    config: &Config,
    settings: &BackendSettings,
    backend: Option<&dyn LlmBackend>,
    progress: &dyn crate::progress::Progress,
) -> Result<VerifyReport, VerifyError> {
    // The records are what the model is asked about; keeping them changes
    // neither the graph nor the extraction cache's fingerprint.
    let mut config = config.clone();
    config.sites = true;
    let built = crate::build_graph(&config, progress)?;
    let prefixes = SecretPrefixes::new(&config.secret_prefixes);
    let sites = selected(&built.graph);

    progress.phase("verify", Some(sites.len()));
    let mut snippets = Snippets::new(&built.repo, &prefixes)?;
    let cache = Cache::new(config.corpus_dir.join(CACHE_DIR_NAME));
    let mut report = VerifyReport {
        sites: sites.len() as u64,
        ..VerifyReport::default()
    };
    let mut records: Vec<LayerRecord> = Vec::new();
    for (done, site) in sites.iter().enumerate() {
        let request = Request::new(site, &built.graph, settings, &mut snippets, &prefixes)?;
        let key = request.key();
        let answer = match cache.get::<Answer>(&key) {
            Some(answer) => {
                report.cached += 1;
                Some(answer)
            }
            None => {
                let prompt = request.prompt();
                let tokens = prompt.len().div_ceil(CHARS_PER_TOKEN) as u64;
                report.estimated_tokens += tokens;
                match backend {
                    // The dry run: counted, never sent.
                    None => None,
                    Some(backend) => {
                        report.asked += 1;
                        let reply =
                            backend
                                .narrate(&prompt)
                                .map_err(|source| VerifyError::Backend {
                                    answered: done as u64,
                                    sites: report.sites,
                                    source,
                                })?;
                        match request.validate(&reply) {
                            Some(answer) => {
                                cache.put(&key, &answer)?;
                                Some(answer)
                            }
                            None => {
                                report.invalid += 1;
                                None
                            }
                        }
                    }
                }
            }
        };
        if let Some(answer) = answer {
            match answer {
                Answer::Candidate { .. } => report.promoted += 1,
                Answer::None => report.none += 1,
            }
            records.push(LayerRecord {
                file: site.file.clone(),
                owner: site.owner.as_str().to_string(),
                line: site.line,
                column: site.column,
                name: site.name.clone(),
                kind: site.kind,
                key,
                candidates: request
                    .targets
                    .iter()
                    .map(|t| t.as_str().to_string())
                    .collect(),
                answer,
            });
        }
        progress.tick(done + 1);
    }
    progress.phase_done();

    if backend.is_some() {
        let layer = Layer {
            schema: LAYER_SCHEMA,
            prompt_version: PROMPT_VERSION,
            redaction_version: REDACTION_VERSION,
            settings: settings.clone(),
            records,
        };
        let path = config.corpus_dir.join(LAYER_FILENAME);
        write_atomically(&path, to_json(&layer).as_bytes()).map_err(|source| VerifyError::Io {
            path: path.clone(),
            source,
        })?;
        report.layer = Some(path);
    }
    Ok(report)
}

/// The layer as written: pretty JSON with every object's keys sorted and a
/// final newline, so two runs that got the same answers write the same bytes.
pub fn to_json(layer: &Layer) -> String {
    // Through a `Value`, whose maps are sorted (serde_json is built without
    // `preserve_order`), as `measure::to_json` does.
    let value = serde_json::to_value(layer).expect("a layer is plain serializable data");
    let mut json = serde_json::to_string_pretty(&value).expect("a JSON value always serializes");
    json.push('\n');
    json
}

/// Read the layer in `corpus_dir` and match it against the graph generation
/// just built (plan A, U12): the verdicts the cards apply, and what became of
/// every record. `graph` must have kept its site records.
pub fn apply_layer(
    corpus_dir: &Path,
    repo: &Repo,
    graph: &Graph,
    secret_prefixes: &[String],
) -> Result<(ModelVerdicts, ModelVerification), LayerError> {
    let path = corpus_dir.join(LAYER_FILENAME);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(LayerError::Missing(path))
        }
        Err(source) => return Err(LayerError::Read { path, source }),
    };
    let layer: Layer = serde_json::from_str(&text).map_err(|source| LayerError::Parse {
        path: path.clone(),
        source,
    })?;
    if layer.schema != LAYER_SCHEMA {
        return Err(LayerError::Schema {
            path,
            found: layer.schema,
            wanted: LAYER_SCHEMA,
        });
    }

    let prefixes = SecretPrefixes::new(secret_prefixes);
    // By stable id. Several records of one id are one site twice (two calls of
    // one name on a line without columns) and ask the same question.
    let mut by_id: HashMap<StableId, &SiteRecord> = HashMap::new();
    for record in graph.sites().records_sorted() {
        by_id.entry(StableId::of(record)).or_insert(record);
    }
    let mut snippets = Snippets::new(repo, &prefixes)?;
    let mut verdicts = ModelVerdicts::default();
    let mut counts = ModelVerification::default();
    for entry in &layer.records {
        let id = StableId {
            file: entry.file.clone(),
            owner: entry.owner.clone(),
            line: entry.line,
            column: entry.column,
            name: entry.name.clone(),
            kind: entry.kind,
        };
        let Some(site) = by_id.get(&id).copied().filter(|s| selectable(s)) else {
            counts.unmatched += 1;
            continue;
        };
        let request = Request::new(site, graph, &layer.settings, &mut snippets, &prefixes)?;
        if request.key() != entry.key {
            counts.stale += 1;
            continue;
        }
        counts.used += 1;
        verdicts.insert(
            site.owner.as_str(),
            site.kind.edge_kind(),
            SiteId {
                file: site.file.clone(),
                line: site.line,
                column: site.column,
            },
            request
                .targets
                .iter()
                .map(|t| t.as_str().to_string())
                .collect(),
            match &entry.answer {
                Answer::Candidate { target } => Some(target.clone()),
                Answer::None => None,
            },
        );
    }
    Ok((verdicts, counts))
}

/// A site's stable id, owned.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StableId {
    file: String,
    owner: String,
    line: u32,
    column: Option<u32>,
    name: String,
    kind: SiteKind,
}

impl StableId {
    fn of(record: &SiteRecord) -> StableId {
        StableId {
            file: record.file.clone(),
            owner: record.owner.as_str().to_string(),
            line: record.line,
            column: record.column,
            name: record.name.clone(),
            kind: record.kind,
        }
    }
}

/// The candidates a site left ambiguous: what the model chooses among.
fn ambiguous_targets(record: &SiteRecord) -> impl Iterator<Item = &NodeKey> {
    record.candidates.iter().filter_map(|c| match c.outcome {
        CandidateOutcome::Rated {
            rung: Confidence::Ambiguous,
            ..
        } => Some(&c.target),
        _ => None,
    })
}

/// Whether the model may be asked about a site: one with an ambiguous
/// candidate, which no index judged. A site the index paired with a
/// declaration of the repository has an index-resolved edge; one it paired
/// with an external symbol has the index's verdict that every candidate is a
/// guess. Either way the index has spoken, and the model stands in for an
/// index only where there is none (KTD12).
fn selectable(record: &SiteRecord) -> bool {
    !matches!(
        record.index,
        Some(IndexPairing::Resolved { .. } | IndexPairing::External)
    ) && ambiguous_targets(record).next().is_some()
}

/// The sites to ask about, in stable-id order, one per stable id.
fn selected(graph: &Graph) -> Vec<&SiteRecord> {
    let mut seen: BTreeSet<StableId> = BTreeSet::new();
    graph
        .sites()
        .records_sorted()
        .into_iter()
        .filter(|r| selectable(r))
        .filter(|r| seen.insert(StableId::of(r)))
        .collect()
}

impl PartialOrd for StableId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for StableId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (
            &self.file,
            &self.owner,
            self.line,
            self.column,
            &self.name,
            self.kind,
        )
            .cmp(&(
                &other.file,
                &other.owner,
                other.line,
                other.column,
                &other.name,
                other.kind,
            ))
    }
}

/// The redacted lines of every file a run reads a snippet from, read and
/// redacted once per file.
struct Snippets<'r> {
    repo: &'r Repo,
    prefixes: &'r SecretPrefixes,
    files: HashMap<String, TrackedFile>,
    redacted: HashMap<String, Vec<String>>,
}

impl<'r> Snippets<'r> {
    fn new(repo: &'r Repo, prefixes: &'r SecretPrefixes) -> Result<Self, ExtractError> {
        let files = repo
            .tracked_files_with_paths()?
            .into_iter()
            .map(|tf| (tf.key.clone(), tf))
            .collect();
        Ok(Snippets {
            repo,
            prefixes,
            files,
            redacted: HashMap::new(),
        })
    }

    /// The window around `line` (1-based) of `file`, the site's line marked,
    /// redacted, at most [`SNIPPET_CAP`] bytes. Empty when the file is not a
    /// tracked file — nothing produces such a site.
    fn around(&mut self, file: &str, line: u32) -> Result<String, ExtractError> {
        if !self.redacted.contains_key(file) {
            let lines = match self.files.get(file) {
                Some(tf) => {
                    let bytes = self.repo.blob_bytes(tf)?;
                    filtered_lines(&String::from_utf8_lossy(&bytes), self.prefixes)
                }
                None => Vec::new(),
            };
            self.redacted.insert(file.to_string(), lines);
        }
        Ok(window(&self.redacted[file], line))
    }
}

/// A whole file's text through both snippet filters, split into lines: the
/// whole file, not the window, because a key block whose `-----BEGIN` sits
/// above the window would leave its key lines in the window matching nothing,
/// and a literal opened above it would leave its text there unread.
///
/// The masking reads the original text and the secret filter its result. The
/// secret filter replaces whole lines, so run first it deleted the opening or
/// closing delimiter of a multi-line literal or of a block comment along with
/// the line that held a `password =`, and the masker paired what was left
/// with the wrong quote — the literal's next lines, or the next literal's
/// value, leaked. On masked text it loses nothing it is there for: an
/// unquoted secret in a comment or in code, a key block outside a literal and
/// a configured token prefix are untouched by the masking, and anything in a
/// literal is gone already. Neither filter adds or removes a line.
fn filtered_lines(text: &str, prefixes: &SecretPrefixes) -> Vec<String> {
    let (text, _) = redact_secret_block(&mask_string_literals(text), prefixes);
    text.lines().map(str::to_string).collect()
}

/// `text` with the contents of every double-quoted string literal replaced by
/// `…`, the quotes, prefixes and line terminators kept — the first filter on
/// a snippet's file, before the secret filter (see [`filtered_lines`] for
/// why in this order).
///
/// The secret filter knows a credential by a keyword at a word boundary
/// (`password =`), and C# rarely writes one that way: `_password = "…"`,
/// `const string SmtpPassword = "…"`, `var dbPwd = "…"`, `GoogleApiKey`,
/// `new NetworkCredential("user", "pw")` all pass it. Which overload a call
/// means depends on a literal's type, never on its value, so the values are
/// dropped wholesale rather than guessed at by name. An empty literal stays
/// `""`.
///
/// The text is read by one tokenizer for C# (to C# 12), the same in the code
/// around literals and in the code of an interpolation hole, so what a
/// literal's end depends on is decided by the token it is in, never by a
/// character searched for ahead:
///
/// - Comments are recognized before anything else, and nothing in one opens a
///   literal or a character: a `"`, `@"`, `$"{` or `"""` written about in a
///   comment never pairs with a quote of the code after it. A quoted span
///   inside a comment is masked as well; an unpaired quote masks the rest of
///   the comment and stops at its end.
/// - A preprocessor directive — a line whose first non-blank character is
///   `#` (`#region`, `#warning`, `#pragma`, `#if`…) — is read like a `//`
///   comment to the end of its line: a `@"`, `"""`, `$"{` or `/*` in a
///   region's name or a warning's message opens nothing.
/// - A line ends where C# ends one: at an LF, a CR, a NEL (U+0085), an LS
///   (U+2028) or a PS (U+2029). Each ends a `//` comment, a directive and the
///   text of a regular literal, and each is kept as it was.
/// - A character literal is read by its exact syntax (`'x'`, `'\''`,
///   `'"'`, `'\x41'`, `'\U0001F600'`); any other apostrophe is plain code
///   (a Rust lifetime `'a`, a quote in prose).
/// - Literals: regular (`\` escapes), verbatim (`@"…"`, `""` escapes, across
///   lines), interpolated (`$"…"`, `$@"…"`, `@$"…"`), raw (`"""…"""`, three or
///   more quotes, across lines; also Python's triple-quoted strings) and
///   interpolated raw (`$$"""…"""`: a hole opens with as many braces as there
///   are `$`, fewer are content). A `u8` suffix is code after the literal.
/// - A hole is code, read recursively: comments, character literals, nested
///   literals of any kind and bracket depth (`(`, `[`, `{`) are tokens of it,
///   so a brace or a quote inside one never closes the hole or the literal. A
///   `:` at depth 0 starts the format part, which is text by the outer
///   literal's rules — a `\"` in a regular literal's format, an apostrophe or
///   a `//` in any format is not code.
///
/// What is never closed costs at most what the language lets it span. A
/// regular or interpolated literal whose text (or format) a line ends inside
/// is masked to the end of that line and closed there — a hole may span lines
/// (C# 11), its text may not. A verbatim or raw literal, or a block comment,
/// runs to the end of the text: masking too much costs context, never a value.
///
/// Known limit: preprocessor conditions are not evaluated, so the text of an
/// inactive region (`#if false` … `#endif`) is read as code. Old code left
/// there with an unterminated verbatim or raw literal (`var old = @"`) opens
/// a literal that runs into the active code below: usually that only masks
/// too much, but the next quote of the active code closes it, and that
/// code's own literal is then read from its closing quote — its value can
/// leak. Telling active from inactive text needs the conditions' symbols,
/// which a snippet filter does not have.
pub(crate) fn mask_string_literals(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut lexer = Lexer {
        chars: &chars,
        at: 0,
    };
    while lexer.at < chars.len() {
        lexer.top_level_token(&mut out);
    }
    out
}

/// How a literal's text reads: its escapes, whether a line ends it, and how
/// it closes.
#[derive(Clone, Copy)]
enum Kind {
    /// `"…"`: `\` escapes, closed by a line break.
    Regular,
    /// `@"…"`: `""` escapes, across lines.
    Verbatim,
    /// `"""…"""` with this many quotes: no escapes, across lines, closed by
    /// at least as many quotes.
    Raw(usize),
}

/// A literal's opening: its kind and its `$` count — 0 when not interpolated;
/// a hole of a raw literal opens with that many braces.
#[derive(Clone, Copy)]
struct Opening {
    kind: Kind,
    dollars: usize,
}

impl Opening {
    /// The quotes that open and close it.
    fn quotes(self) -> usize {
        match self.kind {
            Kind::Raw(quotes) => quotes,
            Kind::Regular | Kind::Verbatim => 1,
        }
    }
}

/// What a literal's body was read as.
struct Body {
    /// The line terminators inside it, kept in the masked text as they were.
    breaks: String,
    /// No content at all — `""` stays `""`.
    empty: bool,
    /// Closed by its quotes; otherwise it ran to a line break (regular) or to
    /// the end of the text, and its closing quotes are not written.
    closed: bool,
}

/// How an interpolation hole ended.
enum HoleEnd {
    /// By its closing brace(s); the literal's text resumes.
    Closed,
    /// Its format part reached the literal's closing quote(s), consumed.
    LiteralClosed,
    /// Its format part (of a regular literal) reached a line break, left
    /// unread: the literal is closed there.
    LineEnd,
    /// The text ended inside it.
    TextEnd,
}

/// Whether `c` ends a line in C#: LF, CR, NEL, LS or PS. A CRLF pair is two
/// of them in a row, and every token that a line ends treats the second as
/// already past its end.
fn is_line_end(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

struct Lexer<'c> {
    chars: &'c [char],
    at: usize,
}

impl Lexer<'_> {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.at + ahead).copied()
    }

    /// The length of the run of `c` at the current position.
    fn run(&self, c: char) -> usize {
        self.chars[self.at..]
            .iter()
            .take_while(|&&x| x == c)
            .count()
    }

    fn starts_with(&self, token: &str) -> bool {
        token
            .chars()
            .enumerate()
            .all(|(k, c)| self.peek(k) == Some(c))
    }

    /// Read one token of top-level code and write it masked.
    fn top_level_token(&mut self, out: &mut String) {
        if self.starts_with("//") || self.starts_with("/*") {
            self.mask_comment(out);
        } else if self.peek(0) == Some('#') && self.at_line_start() {
            // A directive: its text is not code, whatever it spells.
            self.mask_quoted(self.line_end(), out);
        } else if let Some(len) = self.char_literal() {
            out.extend(&self.chars[self.at..self.at + len]);
            self.at += len;
        } else if let Some((prefix, opening)) = self.literal_opening() {
            let quotes = opening.quotes();
            out.extend(&self.chars[self.at..self.at + prefix + quotes]);
            self.at += prefix + quotes;
            let body = self.literal(opening);
            if !body.empty {
                out.push('…');
            }
            out.push_str(&body.breaks);
            if body.closed {
                out.extend(std::iter::repeat_n('"', quotes));
            }
        } else {
            out.push(self.chars[self.at]);
            self.at += 1;
        }
    }

    /// Whether only blanks precede the current position on its line — where a
    /// `#` starts a preprocessor directive. A byte-order mark counts as blank:
    /// `#nullable enable` is often a file's first line.
    fn at_line_start(&self) -> bool {
        self.chars[..self.at]
            .iter()
            .rev()
            .take_while(|&&c| !is_line_end(c))
            .all(|&c| c.is_whitespace() || c == '\u{feff}')
    }

    /// Where the current line ends: before its terminator, or at the end of
    /// the text.
    fn line_end(&self) -> usize {
        self.chars[self.at..]
            .iter()
            .position(|&c| is_line_end(c))
            .map_or(self.chars.len(), |at| self.at + at)
    }

    /// Where the comment at the current position ends: before its line
    /// terminator, or before its `*/` (and whether it has one).
    fn comment_end(&self) -> (usize, bool) {
        if self.starts_with("//") {
            return (self.line_end(), false);
        }
        let body = self.at + 2;
        self.chars[body..]
            .windows(2)
            .position(|w| w == ['*', '/'])
            .map_or((self.chars.len(), false), |at| (body + at, true))
    }

    /// Write the comment at the current position with every quoted span in
    /// it masked within its bounds.
    fn mask_comment(&mut self, out: &mut String) {
        let (end, terminated) = self.comment_end();
        self.mask_quoted(end, out);
        if terminated {
            out.push_str("*/");
            self.at += 2;
        }
    }

    /// Write the text from the current position to `end` — a comment's or a
    /// directive's — with every quoted span in it masked: an unpaired quote
    /// masks the rest of the text and stops at `end`.
    fn mask_quoted(&mut self, end: usize, out: &mut String) {
        let mut i = self.at;
        while i < end {
            let c = self.chars[i];
            out.push(c);
            i += 1;
            if c != '"' {
                continue;
            }
            let close = self.chars[i..end].iter().position(|&q| q == '"');
            let span_end = close.map_or(end, |at| i + at);
            if span_end > i {
                out.push('…');
            }
            let breaks = self.chars[i..span_end].iter().filter(|&&b| is_line_end(b));
            out.extend(breaks);
            if close.is_some() {
                out.push('"');
                i = span_end + 1;
            } else {
                i = end;
            }
        }
        self.at = i;
    }

    /// Skip the comment at the current position inside a hole, keeping its
    /// line terminators.
    fn skip_comment(&mut self, breaks: &mut String) {
        let (end, terminated) = self.comment_end();
        breaks.extend(self.chars[self.at..end].iter().filter(|&&c| is_line_end(c)));
        self.at = if terminated { end + 2 } else { end };
    }

    /// The length of a character literal at the current position, read by
    /// its exact syntax: an apostrophe, one character other than an
    /// apostrophe, a backslash or a line break — or an escape (`\'`, `\n`,
    /// `\x` with 1–4 hex digits, `\u` with 4, `\U` with 8) — and an
    /// apostrophe. `None` when the apostrophe opens none.
    fn char_literal(&self) -> Option<usize> {
        if self.peek(0) != Some('\'') {
            return None;
        }
        let hex = |from: usize, most: usize| {
            (0..most)
                .take_while(|&k| self.peek(from + k).is_some_and(|c| c.is_ascii_hexdigit()))
                .count()
        };
        let body = match self.peek(1)? {
            c if c == '\'' || is_line_end(c) => return None,
            '\\' => match self.peek(2)? {
                '\'' | '"' | '\\' | '0' | 'a' | 'b' | 'e' | 'f' | 'n' | 'r' | 't' | 'v' => 2,
                'x' => match hex(3, 4) {
                    0 => return None,
                    digits => 2 + digits,
                },
                'u' if hex(3, 4) == 4 => 6,
                'U' if hex(3, 8) == 8 => 10,
                _ => return None,
            },
            _ => 1,
        };
        (self.peek(1 + body) == Some('\'')).then_some(body + 2)
    }

    /// When a string literal opens at the current position: how many
    /// characters precede its first quote (`$`, `@`, `$@`, `@$`, `$$`…) and
    /// how it reads.
    fn literal_opening(&self) -> Option<(usize, Opening)> {
        let rest = &self.chars[self.at..];
        let prefix = rest.iter().take_while(|&&c| c == '$' || c == '@').count();
        if rest.get(prefix) != Some(&'"') {
            return None;
        }
        let ats = rest[..prefix].iter().filter(|&&c| c == '@').count();
        if ats > 1 {
            return None;
        }
        let quotes = rest[prefix..].iter().take_while(|&&q| q == '"').count();
        let kind = if ats == 1 {
            Kind::Verbatim
        } else if quotes >= 3 {
            Kind::Raw(quotes)
        } else {
            Kind::Regular
        };
        let dollars = prefix - ats;
        Some((prefix, Opening { kind, dollars }))
    }

    /// Read a literal's body, from just after its opening quotes to just
    /// after its closing ones — or to the line break that closes a regular
    /// one (left unread), or to the end of the text.
    fn literal(&mut self, opening: Opening) -> Body {
        let start = self.at;
        let mut breaks = String::new();
        // A raw literal is empty only without any non-break content: its
        // breaks are its layout, not its value.
        let mut content = false;
        let closed = loop {
            let Some(c) = self.peek(0) else {
                break false;
            };
            if opening.dollars > 0 && c == '{' {
                let braces = self.run('{');
                let opens = match opening.kind {
                    // Braces beyond the `$` count are content.
                    Kind::Raw(_) => braces >= opening.dollars,
                    // `{{` is an escaped brace.
                    Kind::Regular | Kind::Verbatim => braces % 2 == 1,
                };
                content = true;
                self.at += braces;
                if !opens {
                    continue;
                }
                match self.hole(opening, &mut breaks) {
                    HoleEnd::Closed => continue,
                    HoleEnd::LiteralClosed => break true,
                    HoleEnd::LineEnd | HoleEnd::TextEnd => break false,
                }
            }
            match (opening.kind, c) {
                (Kind::Raw(quotes), '"') => {
                    let run = self.run('"');
                    self.at += run;
                    if run >= quotes {
                        break true;
                    }
                    content = true;
                }
                (Kind::Verbatim, '"') if self.peek(1) == Some('"') => {
                    self.at += 2;
                    content = true;
                }
                (_, '"') => {
                    self.at += 1;
                    break true;
                }
                // An escape, but never past a line terminator: it is kept.
                (Kind::Regular, '\\') if self.peek(1).is_some_and(|n| !is_line_end(n)) => {
                    self.at += 2;
                    content = true;
                }
                (Kind::Regular, c) if is_line_end(c) => break false,
                (_, c) if is_line_end(c) => {
                    self.at += 1;
                    breaks.push(c);
                }
                _ => {
                    self.at += 1;
                    content = true;
                }
            }
        };
        let empty = match opening.kind {
            Kind::Raw(_) => !content,
            Kind::Regular | Kind::Verbatim => closed && self.at == start + 1,
        };
        Body {
            breaks,
            empty: empty && closed,
            closed,
        }
    }

    /// Read an interpolation hole of a literal opened by `opening`, from just
    /// after its opening brace(s): code, until the brace that closes it at
    /// depth 0, or its format part.
    fn hole(&mut self, opening: Opening, breaks: &mut String) -> HoleEnd {
        let mut depth = 0usize;
        loop {
            let Some(c) = self.peek(0) else {
                return HoleEnd::TextEnd;
            };
            if self.starts_with("//") || self.starts_with("/*") {
                self.skip_comment(breaks);
            } else if let Some(len) = self.char_literal() {
                self.at += len;
            } else if let Some((prefix, inner)) = self.literal_opening() {
                // Its masked text goes with the outer literal's; only its
                // line terminators are kept.
                self.at += prefix + inner.quotes();
                breaks.push_str(&self.literal(inner).breaks);
            } else {
                match c {
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' => depth = depth.saturating_sub(1),
                    '}' if depth > 0 => depth -= 1,
                    '}' => {
                        // The rest of a closing run belongs to it or is
                        // content; both are masked.
                        self.at += self.run('}');
                        return HoleEnd::Closed;
                    }
                    ':' if depth == 0 => {
                        self.at += 1;
                        return self.format(opening, breaks);
                    }
                    c if is_line_end(c) => breaks.push(c),
                    _ => {}
                }
                self.at += 1;
            }
        }
    }

    /// Read a hole's format part, from just after its `:`: text by the outer
    /// literal's rules, until the hole's closing brace(s).
    fn format(&mut self, opening: Opening, breaks: &mut String) -> HoleEnd {
        loop {
            let Some(c) = self.peek(0) else {
                return HoleEnd::TextEnd;
            };
            match (opening.kind, c) {
                (Kind::Raw(quotes), '"') => {
                    let run = self.run('"');
                    self.at += run;
                    if run >= quotes {
                        return HoleEnd::LiteralClosed;
                    }
                }
                (Kind::Verbatim, '"') if self.peek(1) == Some('"') => self.at += 2,
                (_, '"') => {
                    self.at += 1;
                    return HoleEnd::LiteralClosed;
                }
                (Kind::Raw(_), '}') => {
                    let run = self.run('}');
                    self.at += run;
                    if run >= opening.dollars {
                        return HoleEnd::Closed;
                    }
                }
                (_, '}') => {
                    self.at += self.run('}');
                    return HoleEnd::Closed;
                }
                (Kind::Regular, '\\') if self.peek(1).is_some_and(|n| !is_line_end(n)) => {
                    self.at += 2;
                }
                (Kind::Regular, c) if is_line_end(c) => return HoleEnd::LineEnd,
                (_, c) if is_line_end(c) => {
                    self.at += 1;
                    breaks.push(c);
                }
                _ => self.at += 1,
            }
        }
    }
}

/// Cut the snippet out of a file's (redacted) lines: [`LINES_ABOVE`] above the
/// site's line and [`LINES_BELOW`] below, each prefixed so the site's line is
/// marked, shrunk to [`SNIPPET_CAP`] by dropping the line farther from the site
/// — the top one on a tie — and as a last resort by cutting the site's own line.
fn window(lines: &[String], line: u32) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let at = (line.max(1) as usize - 1).min(lines.len() - 1);
    let mut start = at.saturating_sub(LINES_ABOVE);
    let mut end = (at + LINES_BELOW).min(lines.len() - 1);
    let render = |start: usize, end: usize| -> String {
        (start..=end)
            .map(|i| {
                let mark = if i == at { SITE_MARK } else { CONTEXT_MARK };
                format!("{mark}{}", lines[i])
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut snippet = render(start, end);
    while snippet.len() > SNIPPET_CAP && (start < at || end > at) {
        if at - start >= end - at {
            start += 1;
        } else {
            end -= 1;
        }
        snippet = render(start, end);
    }
    if snippet.len() > SNIPPET_CAP {
        let mut cut = SNIPPET_CAP;
        while !snippet.is_char_boundary(cut) {
            cut -= 1;
        }
        snippet.truncate(cut);
    }
    snippet
}

/// One candidate as the request states it.
#[derive(Debug, Serialize)]
struct Candidate {
    /// The node key, redacted.
    id: String,
    /// The declaration's signature, redacted; empty when it has none.
    signature: String,
}

/// The canonical request for one site: everything the prompt is made of,
/// plus what decides how it is answered. Its SHA-256 is the cache key.
///
/// Field order is the serialization order, so it is part of the key.
#[derive(Debug, Serialize)]
struct Request<'s> {
    prompt_version: u32,
    redaction_version: u32,
    settings: &'s BackendSettings,
    file: String,
    name: String,
    kind: SiteKind,
    column: Option<u32>,
    snippet: String,
    /// Sorted by the targets' keys — the order of the labels `c1`, `c2`, ….
    candidates: Vec<Candidate>,
    /// The candidates' real keys, in the order of `candidates`; what a label
    /// maps back to. Not part of the request: the redacted ids are.
    #[serde(skip)]
    targets: Vec<NodeKey>,
}

impl<'s> Request<'s> {
    fn new(
        site: &SiteRecord,
        graph: &Graph,
        settings: &'s BackendSettings,
        snippets: &mut Snippets,
        prefixes: &SecretPrefixes,
    ) -> Result<Self, ExtractError> {
        let redact = |text: &str| redact_secret_block(text, prefixes).0;
        let mut targets: Vec<NodeKey> = ambiguous_targets(site).cloned().collect();
        targets.sort();
        targets.dedup();
        let candidates = targets
            .iter()
            .map(|target| Candidate {
                id: redact(target.as_str()),
                signature: redact(&signature(graph, target)),
            })
            .collect();
        Ok(Request {
            prompt_version: PROMPT_VERSION,
            redaction_version: REDACTION_VERSION,
            settings,
            file: redact(&site.file),
            name: redact(&site.name),
            kind: site.kind,
            column: site.column,
            snippet: snippets.around(&site.file, site.line)?,
            candidates,
            targets,
        })
    }

    /// SHA-256 of the canonical request, lowercase hex.
    fn key(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("a request is plain serializable data");
        let digest = Sha256::digest(&bytes);
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The prompt, made of the request's fields alone.
    fn prompt(&self) -> String {
        let usage = match self.kind {
            SiteKind::Calls => "a call of",
            SiteKind::Instantiates => "a construction of",
            SiteKind::References => "a reference to",
        };
        let column = match self.column {
            Some(column) => format!(", column {column}"),
            None => String::new(),
        };
        let mut prompt = format!(
            "Which declaration does a name in this source code refer to?\n\n\
             File: {file}\n\
             Usage: {usage} `{name}` on the line marked `>>`{column}\n\n\
             ```\n{snippet}\n```\n\n\
             Candidates:\n",
            file = self.file,
            name = self.name,
            snippet = self.snippet,
        );
        for (i, candidate) in self.candidates.iter().enumerate() {
            prompt.push_str(&format!("c{}: {}", i + 1, candidate.id));
            if !candidate.signature.is_empty() {
                prompt.push_str(&format!(" — `{}`", candidate.signature));
            }
            prompt.push('\n');
        }
        prompt.push_str(
            "\nAnswer with exactly one label from the list (`c1`, `c2`, …), or with `none` if \
             the usage refers to none of the candidates. Answer with the label alone and \
             nothing else.",
        );
        prompt
    }

    /// The answer, when the reply is one candidate's label or `none` — the
    /// closed set (KTD12). Surrounding whitespace and case are forgiven,
    /// nothing else.
    fn validate(&self, reply: &str) -> Option<Answer> {
        let reply = reply.trim().to_ascii_lowercase();
        if reply == "none" {
            return Some(Answer::None);
        }
        let index: usize = reply.strip_prefix('c')?.parse().ok()?;
        let target = self.targets.get(index.checked_sub(1)?)?;
        // `c01` parses as 1 but is not a label the prompt offered.
        (reply == format!("c{index}")).then(|| Answer::Candidate {
            target: target.as_str().to_string(),
        })
    }
}

/// A candidate's signature, as the graph states it: its `SymbolSignature`
/// fact, or empty.
fn signature(graph: &Graph, target: &NodeKey) -> String {
    graph
        .node(target)
        .and_then(|node| {
            node.facts.iter().find_map(|f| match (&f.kind, &f.value) {
                (FactKind::SymbolSignature, FactValue::Text(text)) => Some(text.clone()),
                _ => None,
            })
        })
        .unwrap_or_default()
}

/// The answer cache: one file per key, `<root>/<first two hex digits>/<key>.json`.
///
/// Shared by both layers `verify` writes — the sites' and, since plan B's
/// U13, the purposes' ([`purpose`]) — each storing its own answer type. Their
/// keys cannot meet: a purpose request opens with the layer it is for, a
/// field no site request has, so no request of one serializes to the bytes
/// of a request of the other; and an entry read as the other's answer would
/// not parse and is a miss.
pub(crate) struct Cache {
    root: PathBuf,
}

/// One cached answer.
#[derive(Serialize, Deserialize)]
struct CachedAnswer<T> {
    key: String,
    #[serde(flatten)]
    answer: T,
}

impl Cache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Cache { root }
    }

    fn path(&self, key: &str) -> PathBuf {
        self.root.join(&key[..2]).join(format!("{key}.json"))
    }

    /// The cached answer for `key`. Anything unreadable — no file, a file
    /// that does not parse, one stored under another key — is a miss: the
    /// worst a miss costs is one more question.
    pub(crate) fn get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        let bytes = std::fs::read(self.path(key)).ok()?;
        let cached: CachedAnswer<T> = serde_json::from_slice(&bytes).ok()?;
        (cached.key == key).then_some(cached.answer)
    }

    pub(crate) fn put<T: Serialize + Clone>(
        &self,
        key: &str,
        answer: &T,
    ) -> Result<(), VerifyError> {
        let path = self.path(key);
        let cached = CachedAnswer {
            key: key.to_string(),
            answer: answer.clone(),
        };
        let bytes = serde_json::to_vec(&cached).expect("an answer is plain serializable data");
        write_atomically(&path, &bytes).map_err(|source| VerifyError::Io { path, source })
    }
}

/// Write through a temporary file in the same directory and rename it over
/// `path`, so a run killed mid-write leaves the old file or the new one, never
/// half of one.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::external::REDACTION_MARKER;

    fn lines(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("line {i}")).collect()
    }

    /// Every C# literal kind loses its value and keeps its shape.
    #[test]
    fn every_kind_of_string_literal_is_masked_and_keeps_its_shape() {
        for (code, masked) in [
            (r#"var a = "x\"y"; f(a);"#, r#"var a = "…"; f(a);"#),
            (r#"var e = ""; g();"#, r#"var e = ""; g();"#),
            (r#"var v = @"C:\x""y"; h();"#, r#"var v = @"…"; h();"#),
            (r#"var i = $"{a} and {{b}}"; k();"#, r#"var i = $"…"; k();"#),
            (
                r#"var n = $"{(ok ? "yes" : "no")} tail"; m();"#,
                r#"var n = $"…"; m();"#,
            ),
            (
                r#"var r = """say "hi" now"""; p();"#,
                r#"var r = """…"""; p();"#,
            ),
            (
                r#"var d = $$"""{{x}} "q" """; q();"#,
                r#"var d = $$"""…"""; q();"#,
            ),
            (r#"var u = "x"u8; s();"#, r#"var u = "…"u8; s();"#),
            // A brace in a character literal inside a hole closes nothing,
            // so the string after it stays inside the masked literal.
            (
                r#"var b = $"{(c == '}' ? "pw-1" : "pw-2")} t"; w();"#,
                r#"var b = $"…"; w();"#,
            ),
            (
                r#"var o = $"{(c == '{' ? "pw-3" : "x")}"; z();"#,
                r#"var o = $"…"; z();"#,
            ),
            // A nested interpolated literal is read by the same rules, so a
            // brace in a string inside it closes neither hole.
            (
                r#"var n = $"{f($"{g("}")}", "pw-6")} t"; y();"#,
                r#"var n = $"…"; y();"#,
            ),
            (
                r#"var m = $"{(ok ? $@"{a} ""}"" " : "pw-7")}"; x();"#,
                r#"var m = $"…"; x();"#,
            ),
            // An interpolated raw literal reads its holes as code, so a raw
            // literal nested in a hole is not its closing quotes.
            (
                r#"var q = $"""a {f("""pw-8""")} b"""; v();"#,
                r#"var q = $"""…"""; v();"#,
            ),
            (
                r#"var p = $$"""{{x}} {{g($"""{"pw-9"}""")}} c"""; u();"#,
                r#"var p = $$"""…"""; u();"#,
            ),
            // A comment inside a hole holds no brace that counts.
            (
                r#"var c = $"""a {f(/* } */ """pw-10""")} b"""; t();"#,
                r#"var c = $"""…"""; t();"#,
            ),
            (
                r#"var k = $"{f(/* } */ "pw-11")} b"; s();"#,
                r#"var k = $"…"; s();"#,
            ),
        ] {
            assert_eq!(mask_string_literals(code), masked, "{code}");
        }
    }

    /// A literal across lines keeps its line breaks, so every line of the
    /// file stays where the snippet's window looks for it.
    #[test]
    fn a_literal_across_lines_keeps_every_line_break() {
        let code = "var v = @\"one\ntwo \"\" three\nfour\";\nnext();\nvar r = \"\"\"\n  raw\n  \"\"\";\nlast();";
        let masked = mask_string_literals(code);
        assert_eq!(masked.lines().count(), code.lines().count(), "{masked}");
        assert_eq!(
            masked,
            "var v = @\"…\n\n\";\nnext();\nvar r = \"\"\"…\n\n\"\"\";\nlast();"
        );
        // A hole of a regular interpolated literal may span lines (C# 11):
        // the value after the hole is still masked and the break kept.
        let code = "Connect($\"{ResolveHost(\n    id)}:pw-12\");\nnext();";
        assert_eq!(mask_string_literals(code), "Connect($\"…\n\");\nnext();");
    }

    /// A quote a line never closes masks the rest of that line only, and a
    /// quote in a character literal opens nothing.
    #[test]
    fn a_stray_quote_costs_at_most_the_rest_of_its_line() {
        assert_eq!(
            mask_string_literals("// says \"hi\nvar k = \"key\";"),
            "// says \"…\nvar k = \"…\";"
        );
        assert_eq!(
            mask_string_literals("if (c == '\"' || c == '\\'') s = \"v\";"),
            "if (c == '\"' || c == '\\'') s = \"…\";"
        );
        assert_eq!(
            mask_string_literals("fn f<'a>(x: &'a str) { g(\"v\") }"),
            "fn f<'a>(x: &'a str) { g(\"…\") }"
        );
    }

    /// Mask `code` and check that `secret` is gone, that every piece of
    /// `visible` code survived, and that the line count is unchanged.
    fn assert_masked(code: &str, secret: &str, visible: &[&str]) {
        let masked = mask_string_literals(code);
        assert!(!masked.contains(secret), "{code:?} leaked: {masked:?}");
        for keep in visible {
            assert!(masked.contains(keep), "{code:?} lost {keep:?}: {masked:?}");
        }
        assert_eq!(
            masked.matches('\n').count(),
            code.matches('\n').count(),
            "{code:?} -> {masked:?}"
        );
    }

    /// A comment is bounded before anything in it is read: a quote, a
    /// literal opener or an apostrophe in its text never reaches past its end,
    /// and a quoted span inside it is masked within it.
    #[test]
    fn a_comment_holds_its_quotes_to_itself() {
        for (code, visible) in [
            (
                "/* strip a leading \" */ Send(\"LEAK\");",
                &["*/ Send(\"…\");"][..],
            ),
            (
                "/* strip a leading \\\" */ Send(\"LEAK\");",
                &["*/ Send(\"…\");"],
            ),
            (
                "// Verbatim strings start with @\"\nSend(\"LEAK\");",
                &["\nSend(\"…\");"],
            ),
            (
                "/* interpolation syntax: $\"{id} */ Send(\"LEAK\");",
                &["*/ Send(\"…\");"],
            ),
            (
                "// Raw delimiter: \"\"\"\nSend(\"\"\"LEAK\"\"\");",
                &["\nSend(\"\"\"…\"\"\");"],
            ),
            ("/* '\\' */F(\"O'Reilly\");", &["*/F(\"…\");"]),
            (
                "/* a \" b */ var v = @\"LEAK\"; var w = $\"LEAK\"; var r = \"\"\"LEAK\"\"\"; var u = \"LEAK\"u8;",
                &["var v = @\"…\";", "var w = $\"…\";", "var r = \"\"\"…\"\"\";", "var u = \"…\"u8;"],
            ),
            (
                "// Raw delimiter: \"\"\"\nvar s = \"LEAK\";\nnext();",
                &["\nvar s = \"…\";\nnext();"],
            ),
            (
                "// Interpolation starts with $\"{\nvar s = \"LEAK\";\nnext();",
                &["\nvar s = \"…\";\nnext();"],
            ),
        ] {
            assert_masked(code, "LEAK", visible);
            assert_masked(code, "Reilly", visible);
        }
        // The shape: a quoted span is masked inside the comment, an unpaired
        // quote masks the rest of the comment and stops at its end.
        assert_eq!(
            mask_string_literals("/* say \"pw\" or \"x */ f(\"y\"); // \"\" and \"z\""),
            "/* say \"…\" or \"…*/ f(\"…\"); // \"\" and \"…\""
        );
        assert_eq!(
            mask_string_literals("/* a \"one\ntwo\" b \"c\nd */ g();"),
            "/* a \"…\n\" b \"…\n*/ g();"
        );
    }

    /// A character literal is read by its exact C# syntax; an apostrophe that
    /// opens none is plain code, and a valid one holding a quote opens nothing.
    #[test]
    fn a_character_literal_is_read_by_its_exact_syntax() {
        for code in [
            "if (c == '\\u0022') s = \"LEAK\";",
            "if (c == '\\x22') s = \"LEAK\";",
            "if (c == '\\U0001F600') s = \"LEAK\";",
            "if (c == '\\\\') s = \"LEAK\";",
            "if (c == '\\'') s = \"LEAK\";",
            "if (c == '\"') s = \"LEAK\";",
            "x = 'a; s = \"LEAK\";",
        ] {
            assert_masked(code, "LEAK", &["s = \"…\";"]);
        }
    }

    /// The format part of a hole (`{x:format}`) is literal text by the outer
    /// literal's rules — never code, never a literal of its own — and a
    /// literal around a malformed hole never swallows the code after it.
    #[test]
    fn an_interpolation_format_is_text_of_the_literal_around_it() {
        for (code, secret, visible) in [
            (
                "if (ok) { var s = $\"{amount:0.00 \\\"USD\\\"}\"; } Send(\"LEAK\");",
                "LEAK",
                &["} Send(\"…\");"][..],
            ),
            (
                "if (ok) { var s = $\"\"\"{inches:0.00 \\\"}\"\"\"; } Send(\"\"\"LEAK\"\"\");",
                "LEAK",
                &["} Send(\"\"\"…\"\"\");"],
            ),
            (
                "var s = $\"{amount:0.00 \\\"USD\\\"}\";\nLog(s);\nSave();",
                "USD",
                &["\nLog(s);\nSave();"],
            ),
            (
                "var s = $\"\"\"{inches:0.00 \\\"}\"\"\";\nLog(s);\nSave();",
                "inches",
                &["\nLog(s);\nSave();"],
            ),
            (
                "var s = $\"Value: '{amount:0.00 'USD'}'\";\nLog(s);\nSave();",
                "USD",
                &["var s = $\"…\";\nLog(s);\nSave();"],
            ),
            (
                "var s = $@\"Value: '{amount:0.00 'USD'}'\";\nLog(s);\nSave();",
                "USD",
                &["var s = $@\"…\";\nLog(s);\nSave();"],
            ),
            (
                "var s = $\"\"\"Value: '{amount:0.00 'USD'}'\"\"\";\nLog(s);\nSave();",
                "USD",
                &["var s = $\"\"\"…\"\"\";\nLog(s);\nSave();"],
            ),
            (
                "var s = $\"{orderId:'https://host/orders/'0}\";\nLog(s);\nSave();",
                "host",
                &["var s = $\"…\";\nLog(s);\nSave();"],
            ),
            (
                "var s = $\"{x,-12:N2} LEAK\"; f();",
                "LEAK",
                &["var s = $\"…\"; f();"],
            ),
            // A colon nested in the hole's code is not its format.
            (
                "var s = $\"{new { A = c ? 1 : 2 }} LEAK {(ok ? \"a\" : \"b\")}\"; f();",
                "LEAK",
                &["var s = $\"…\"; f();"],
            ),
        ] {
            assert_masked(code, secret, visible);
        }
    }

    /// The leaks earlier rounds fixed one at a time, kept as one set.
    #[test]
    fn a_hole_is_read_as_code_to_any_depth() {
        for (code, visible) in [
            ("var s = $\"{Format('}', \"LEAK\")}\"; f();", "; f();"),
            (
                "var s = $\"{(c == '}' ? \"LEAK\" : \"x\")}\"; f();",
                "; f();",
            ),
            (
                "var s = $\"{Format($\"{Foo(\"}\")} LEAK\")}\"; f();",
                "; f();",
            ),
            (
                "var s = $$\"\"\"{{Format(\"\"\"LEAK\"\"\")}}\"\"\"; f();",
                "; f();",
            ),
            (
                "var s = $$\"\"\"{{Format(/* closing brace: } */ \"\"\"LEAK\"\"\")}}\"\"\"; f();",
                "; f();",
            ),
            (
                "var s = $\"{f(// a } here\n  \"LEAK\")} t\";\nf();",
                "\";\nf();",
            ),
            (
                "Connect($\"{ResolveHost(\n    id)}:LEAK\");\nf();",
                "\");\nf();",
            ),
            ("var s = $@\"{f(\n  @\"LEAK\")}\n LEAK\"; f();", "\"; f();"),
            (
                "var s = $$\"\"\"{{{x}}} LEAK {{ $\"{y:N2}\" }}\"\"\"; f();",
                "\"\"\"; f();",
            ),
        ] {
            assert_masked(code, "LEAK", &[visible]);
        }
    }

    /// Realistic C# lines, each carrying `SECRETVALUE` in a literal: alone and
    /// all together (so one line's misreading would show in the next), no
    /// value survives, no line is lost and no code line after them is
    /// swallowed.
    #[test]
    fn no_value_survives_a_corpus_of_realistic_lines() {
        let corpus = [
            r#"var conn = "Server=db;User Id=sa;Password=SECRETVALUE;";"#,
            r#"private const string ApiKey = "SECRETVALUE";"#,
            r#"var path = @"C:\secrets\SECRETVALUE.txt";"#,
            r#"var quoted = @"say ""SECRETVALUE"" loud";"#,
            r#"var msg = $"User {user.Name} has key SECRETVALUE";"#,
            r#"var both = $@"C:\{dir}\SECRETVALUE";"#,
            r#"var both2 = @$"C:\{dir}\SECRETVALUE";"#,
            r#"var json = """{"token": "SECRETVALUE"}""";"#,
            r#"var tpl = $$"""{"id": {{id}}, "pw": "SECRETVALUE"}""";"#,
            r#"var raw4 = """"has """ inside SECRETVALUE"""";"#,
            r#"ReadOnlySpan<byte> b = "SECRETVALUE"u8;"#,
            r#"var esc = "a\"SECRETVALUE\"b\\";"#,
            r#"var fmt = $"{amount:C2} SECRETVALUE {date:yyyy-MM-dd}";"#,
            r#"var fmt2 = $"{amount,10:0.00 \"SECRETVALUE\"}";"#,
            r#"var tern = $"{(ok ? "SECRETVALUE" : "no")}";"#,
            r#"var nest = $"{Wrap($"{Inner("SECRETVALUE")}")}";"#,
            r#"var ch = c == '"' ? "SECRETVALUE" : "";"#,
            r#"var ch2 = c == '\'' ? "SECRETVALUE" : "";"#,
            r#"var ch3 = c == '\u0022' ? "SECRETVALUE" : "";"#,
            r#"var cred = new NetworkCredential("admin", "SECRETVALUE");"#,
            r#"// Default password is "SECRETVALUE" until rotated"#,
            r#"/* the token "SECRETVALUE" is test-only */ Init();"#,
            r#"/// <param name="pw">e.g. "SECRETVALUE"</param>"#,
            r#"config["Password"] = "SECRETVALUE"; // don't log"#,
            r#"var url = "https://user:SECRETVALUE@host/path"; // a "url""#,
            r#"Log($"{x}}} SECRETVALUE {{y}}");"#,
            r#"var lambda = $"{items.Select(i => $"{i}:SECRETVALUE")}";"#,
            r#"var sw = $"{x switch { 1 => "SECRETVALUE", _ => "b" }}";"#,
            r#"var arr = $"{new[] { "SECRETVALUE" }[0]}";"#,
            r#"var cmt = $"{f(/* " */ "SECRETVALUE")}";"#,
            r#"if (s == "") return "SECRETVALUE";"#,
            r#"var multi = @"first line"#,
        ];
        for line in corpus {
            assert_masked(line, "SECRETVALUE", &[]);
        }
        let mut joined = String::new();
        for line in corpus {
            joined.push_str(line);
            joined.push_str("\nKeep();\n");
        }
        joined.push_str("SECRETVALUE\"; Keep();");
        let masked = mask_string_literals(&joined);
        assert!(!masked.contains("SECRETVALUE"), "{masked}");
        assert_eq!(masked.lines().count(), joined.lines().count(), "{masked}");
        // The unclosed verbatim line runs on by design; every `Keep();`
        // before it survives.
        assert_eq!(
            masked.matches("\nKeep();\n").count(),
            corpus.len() - 1,
            "{masked}"
        );
    }

    /// Filter `code` the way a snippet's file is filtered and check that no
    /// `secret` survives, that every piece of `visible` code does, and that
    /// the line count is unchanged.
    fn assert_filtered(code: &str, prefixes: &SecretPrefixes, secrets: &[&str], visible: &[&str]) {
        let lines = filtered_lines(code, prefixes);
        let filtered = lines.join("\n");
        for secret in secrets {
            assert!(
                !filtered.contains(secret),
                "{code:?} leaked {secret:?}: {filtered:?}"
            );
        }
        for keep in visible {
            assert!(
                filtered.contains(keep),
                "{code:?} lost {keep:?}: {filtered:?}"
            );
        }
        assert_eq!(
            lines.len(),
            code.lines().count(),
            "{code:?} -> {filtered:?}"
        );
    }

    /// The secret filter replaces whole lines. Run before the masking it
    /// deleted a multi-line literal's or a block comment's opening or closing
    /// delimiter with the line, and the masker paired what was left with the
    /// wrong quote: the literal's later lines, or the next literal, leaked.
    /// The masking reads the original text, so a replaced line costs nothing
    /// but itself.
    #[test]
    fn a_redacted_line_never_costs_a_literal_or_comment_its_boundary() {
        let builtin = SecretPrefixes::new(&[]);
        for (code, secrets, visible) in [
            (
                "string password = @\"first\nLEAK\";",
                &["LEAK"][..],
                &[][..],
            ),
            ("string password = $@\"first {id}\nLEAK\";", &["LEAK"], &[]),
            ("string password = @$\"first {id}\nLEAK\";", &["LEAK"], &[]),
            ("string password = \"\"\"\nLEAK\n\"\"\";", &["LEAK"], &[]),
            (
                "string password = $$\"\"\"\nLEAK {{id}}\n\"\"\";",
                &["LEAK", "{{id}}"],
                &[],
            ),
            ("string password = $\"{GetId(\n42)} LEAK\";", &["LEAK"], &[]),
            (
                "var s = @\"first\nlast\"; // password=obsolete\nSend(\"LEAK\");",
                &["LEAK"],
                &["Send(\"…\");"],
            ),
            (
                "var s = $@\"first\n{x}last\"; // password=obsolete\nSend(\"LEAK\");",
                &["LEAK"],
                &["Send(\"…\");"],
            ),
            (
                "var s = \"\"\"\nfirst\n\"\"\"; // password=obsolete\nSend(\"\"\"LEAK\"\"\");",
                &["LEAK"],
                &["Send(\"\"\"…\"\"\");"],
            ),
            (
                "/* password=example\nVerbatim prefix: @\"\n*/ Send(\"LEAK\");",
                &["LEAK"],
                &["*/ Send(\"…\");"],
            ),
            (
                "/* documentation\npassword=example */\nSend(\"prefix\\\"LEAK\");",
                &["LEAK"],
                &["Send(\"…\");"],
            ),
            (
                "/* documentation\npassword=example */\nSend(\"\"\"{\"name\":\"LEAK\"}\"\"\");",
                &["LEAK", "name"],
                &["Send(\"\"\"…\"\"\");"],
            ),
        ] {
            let code = format!("{code}\nLog();\nSave();");
            let mut keep = vec!["\nLog();\nSave();"];
            keep.extend(visible);
            assert_filtered(&code, &builtin, secrets, &keep);
        }
    }

    /// A preprocessor directive's text is not code: a `@"`, `"""`, `$"{` or
    /// `/*` in a `#region` name or a `#warning` message never opens a literal
    /// or a comment that pairs with the code below it. A quoted span in it is
    /// masked within its line, as in a `//` comment.
    #[test]
    fn a_directive_line_never_opens_a_literal_or_comment() {
        let builtin = SecretPrefixes::new(&[]);
        for (code, visible) in [
            (
                "#warning Verbatim string prefix: @\"\nSend(\"LEAK\");",
                &["\nSend(\"…\");"][..],
            ),
            (
                "#warning prefixes $@\" and @$\"\nSend(\"LEAK\");",
                &["\nSend(\"…\");"],
            ),
            (
                "#region SQL using \"\"\" raw strings\nSend(\"\"\"LEAK\"\"\");\n#endregion",
                &["\nSend(\"\"\"…\"\"\");\n#endregion"],
            ),
            (
                "#region SQL using $$\"\"\"\" raw strings\nSend(\"\"\"LEAK\"\"\");\n#endregion",
                &["\nSend(\"\"\"…\"\"\");\n#endregion"],
            ),
            (
                "#region Interpolation syntax $\"{\nSend(\"LEAK\");\n#endregion",
                &["\nSend(\"…\");\n#endregion"],
            ),
            (
                "    #region a /* b\nSend(\"a\\\"LEAK\");\n    #endregion",
                &["\nSend(\"…\");\n    #endregion"],
            ),
            (
                "class C {\n\t#pragma warning disable CS0618 // \"\"\"\n\tvoid F() => Send(\"LEAK\");\n}",
                &["\tvoid F() => Send(\"…\");\n}"],
            ),
        ] {
            let code = format!("{code}\nLog();");
            assert_filtered(&code, &builtin, &["LEAK"], &[visible[0], "\nLog();"]);
        }
        assert_eq!(
            mask_string_literals("#error \"LEAK\" or \"x\nSend(\"y\");"),
            "#error \"…\" or \"…\nSend(\"…\");"
        );
    }

    /// C# ends a line at a CR, a NEL, an LS and a PS as well as at an LF: a
    /// `//` comment, a directive and a regular literal all end there, and the
    /// character is kept where it was.
    #[test]
    fn every_csharp_line_terminator_ends_a_line_comment_and_a_regular_literal() {
        let builtin = SecretPrefixes::new(&[]);
        for t in ["\r", "\u{85}", "\u{2028}", "\u{2029}", "\r\n"] {
            // The secret filter splits at an LF and drops a CR before it.
            let kept = if t == "\r\n" { "\n" } else { t };
            for (code, visible) in [
                (
                    format!("// Strip '\"'{t}Send(\"LEAK\");"),
                    format!("{kept}Send(\"…\");"),
                ),
                (
                    format!("// note{t}Send(\"prefix\\\"LEAK\");"),
                    format!("{kept}Send(\"…\");"),
                ),
                (
                    format!("// note{t}Send(\"\"\"{{\"name\":\"LEAK\"}}\"\"\");"),
                    format!("{kept}Send(\"\"\"…\"\"\");"),
                ),
                (
                    format!("#region raw \"\"\"{t}Send(\"\"\"LEAK\"\"\");"),
                    format!("{kept}Send(\"\"\"…\"\"\");"),
                ),
                (
                    format!("var s = \"open{t}Send(\"LEAK\");"),
                    format!("\"…{kept}Send(\"…\");"),
                ),
                (
                    format!("var s = \"open\\{t}Send(\"LEAK\");"),
                    format!("\"…{kept}Send(\"…\");"),
                ),
                (
                    format!("var s = $\"{{x:open{t}Send(\"LEAK\");"),
                    format!("$\"…{kept}Send(\"…\");"),
                ),
            ] {
                let code = format!("{code}\nLog();");
                assert_filtered(&code, &builtin, &["LEAK", "name"], &[&visible, "\nLog();"]);
            }
        }
    }

    /// Masking first leaves the secret filter its own job: an unquoted secret
    /// in a comment or in code, a key block outside a literal and a configured
    /// token prefix are still replaced.
    #[test]
    fn the_secret_filter_still_reads_the_masked_text() {
        let configured = SecretPrefixes::new(&["ACMEX_".to_string()]);
        for (code, secret) in [
            ("// token ACMEX_abcdefghijklmnopqrstuv\nSend();", "abcdefghijklmnop"),
            ("var x = Build(); // password = hunter2\nSend();", "hunter2"),
            (
                "// -----BEGIN RSA PRIVATE KEY-----\n// MIIEpAIBAAKCAQEAsecretkeybytes\n// -----END RSA PRIVATE KEY-----\nSend();",
                "MIIEpAIBAAKCAQEA",
            ),
        ] {
            assert_filtered(code, &configured, &[secret], &[REDACTION_MARKER, "\nSend();"]);
        }
    }

    #[test]
    fn the_window_is_twelve_above_and_six_below_with_the_site_marked() {
        let snippet = window(&lines(40), 20);
        let rows: Vec<&str> = snippet.lines().collect();
        assert_eq!(rows.len(), LINES_ABOVE + 1 + LINES_BELOW);
        assert_eq!(rows[0], "   line 8");
        assert_eq!(rows[LINES_ABOVE], ">> line 20");
        assert_eq!(rows.last(), Some(&"   line 26"));
    }

    #[test]
    fn the_window_stops_at_the_file_edges() {
        let snippet = window(&lines(5), 2);
        assert_eq!(
            snippet,
            "   line 1\n>> line 2\n   line 3\n   line 4\n   line 5"
        );
    }

    #[test]
    fn an_oversized_window_drops_lines_far_from_the_site_first() {
        let long: Vec<String> = (1..=40)
            .map(|i| format!("{i:>3}{}", "x".repeat(500)))
            .collect();
        let snippet = window(&long, 20);
        assert!(snippet.len() <= SNIPPET_CAP, "{}", snippet.len());
        let rows: Vec<&str> = snippet.lines().collect();
        let site = rows.iter().position(|r| r.starts_with(SITE_MARK)).unwrap();
        // Balanced around the site: the farther side lost lines first.
        assert!(site.abs_diff(rows.len() - 1 - site) <= 1, "{rows:?}");
    }

    #[test]
    fn a_site_line_over_the_cap_is_cut_on_a_char_boundary() {
        let long = vec!["é".repeat(SNIPPET_CAP)];
        let snippet = window(&long, 1);
        assert!(snippet.len() <= SNIPPET_CAP);
        assert!(snippet.starts_with(SITE_MARK));
    }
}
