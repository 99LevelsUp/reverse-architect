//! The model layer for a type's purpose (plan B, U13: R7, R10, KTD12).
//!
//! The deterministic purpose ([`crate::card::purpose`]) says only what the code
//! says: a stereotype, the shape of the members, what the type builds on and
//! reaches, the first sentence of its comment. Where that is thin, an operator
//! may let a cheap model say in one sentence what the type is for. The answer
//! is a layer beside the corpus, kept apart exactly as the site verdicts are
//! ([`super`]):
//!
//! - **Separate.** `verify --purpose` writes [`LAYER_FILENAME`] into the corpus
//!   directory and nothing else there. Generation reads it only when asked
//!   (`--model-purpose`; off by default, R7) and shows a sentence as its own
//!   labelled line under the type card's `## Purpose`
//!   ([`crate::card::story::Story::model_purpose`]) — never in the header, the
//!   map row or the manifest, which read the deterministic purpose alone.
//! - **Facts in, not code.** The model is shown what the card already knows
//!   about the type: its identity and name, the kind as its signature states
//!   it, the signature, the deterministic purpose sentences, its bases, the
//!   types it uses and is used by, its database accesses and its anchors —
//!   the last four lists capped at [`NAMED_FACTS`]. No source text is read.
//!   The anchors are the texts extraction kept; a text its secret filter
//!   withheld is no anchor and is never sent. Every field still goes through
//!   [`redact_secret_block`] with the run's prefixes before it leaves, as a
//!   site request's fields do.
//! - **Checked answers.** A reply must be one line of at most
//!   [`SENTENCE_CAP`] bytes, and the enrich guardrail
//!   ([`crate::enrich::guardrail`]) must find in it no number, code-shaped
//!   identifier or backticked span the facts sent do not contain, and no
//!   Markdown heading. Anything else is invalid: counted, not cached, no
//!   record — the next run asks again.
//! - **Cached by content.** A request's key is the SHA-256 of the canonical
//!   request: the facts as sent, the prompt and redaction versions, the
//!   backend settings and the language asked for. The answers share
//!   [`super::CACHE_DIR_NAME`] with the site verdicts; the request names its
//!   layer, a field no site request has, so the two key spaces cannot meet.
//!   Generation recomputes the key from the card it built: a record whose
//!   type's facts changed since is
//!   stale and not shown, one whose type is gone is unmatched, and coverage
//!   counts both ([`crate::coverage::ModelPurpose`]).

use super::{
    write_atomically, BackendSettings, Cache, LayerError, VerifyError, CACHE_DIR_NAME,
    CHARS_PER_TOKEN, REDACTION_VERSION,
};
use crate::card::detail::AnchorBody;
use crate::card::render::Lang;
use crate::card::{Card, CardKind};
use crate::config::Config;
use crate::coverage::ModelPurpose;
use crate::enrich::backend::LlmBackend;
use crate::enrich::bundle::{extract_identifiers, extract_numbers, LicensedSet};
use crate::enrich::guardrail::{self, Verdict};
use crate::extract::external::{redact_secret_block, SecretPrefixes};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};

/// The layer `verify --purpose` writes into the corpus directory.
pub const LAYER_FILENAME: &str = "model-purpose.json";

/// The layer file's shape. Generation refuses a layer of another schema
/// rather than guess at it.
pub const LAYER_SCHEMA: u32 = 1;

/// The prompt's wording and the request's shape. Part of every key: a
/// re-worded question is a new question, and its old answers are stale.
pub const PROMPT_VERSION: u32 = 1;

/// Most entries of one list of facts the model is shown — the types used, the
/// users, the database accesses, the anchors. The first ones in the card's
/// own order: enough to say what a type is for, and a type with thousands of
/// texts costs no more to ask about than one with a dozen.
pub const NAMED_FACTS: usize = 12;

/// Most bytes an answer may have, after its whitespace is collapsed: one
/// sentence, the size of a long purpose sentence of the card's own.
pub const SENTENCE_CAP: usize = 400;

/// The layer file: the valid answers of the last complete `verify --purpose`
/// run, one per type, in identity order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurposeLayer {
    pub schema: u32,
    /// Informational, like the site layer's: the keys already cover it.
    pub prompt_version: u32,
    /// Informational, as `prompt_version`.
    pub redaction_version: u32,
    /// The backend that answered; generation needs it to recompute a key.
    pub settings: BackendSettings,
    /// The language the sentences were asked for (`en`, `cs`). Informational:
    /// generation recomputes every key in its own language, so a layer asked
    /// in another one is stale as a whole.
    pub lang: String,
    pub records: Vec<PurposeRecord>,
}

/// One answered type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurposeRecord {
    /// The type's node key.
    pub identity: String,
    /// SHA-256 of the canonical request the answer was given to, lowercase hex.
    pub key: String,
    /// The sentence, whitespace collapsed.
    pub sentence: String,
}

/// What a `verify --purpose` run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PurposeReport {
    /// Type cards the run would ask about — every one.
    pub types: u64,
    /// …whose answer the cache already held.
    pub cached: u64,
    /// …sent to the backend. Zero on a dry run.
    pub asked: u64,
    /// …whose answer failed the checks: left out, and not cached.
    pub invalid: u64,
    /// Valid answers, cached or fresh — the layer's records.
    pub written: u64,
    /// The prompts' size in tokens (see [`CHARS_PER_TOKEN`]): on a dry run, of
    /// the types the cache does not hold; on a real run, of those asked.
    pub estimated_tokens: u64,
    /// Where the layer was written. `None` on a dry run.
    pub layer: Option<PathBuf>,
}

/// One cached answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Answer {
    sentence: String,
}

/// Ask the model what every type of the graph `config` builds is for, and
/// write the layer. With `backend` `None` this is the dry run: nothing is sent
/// and nothing written, and the report says how much would be.
///
/// The cards are built exactly as generation builds them from that graph, so
/// the facts asked about — and therefore the keys — are the ones generation
/// recomputes.
pub fn run(
    config: &Config,
    settings: &BackendSettings,
    lang: Lang,
    backend: Option<&dyn LlmBackend>,
    progress: &dyn crate::progress::Progress,
) -> Result<PurposeReport, VerifyError> {
    let built = crate::build_graph(config, progress)?;
    progress.phase("build cards", None);
    let (cards, _, _) = crate::card::build_cards_with_insight(&built.graph);
    progress.phase_done();
    let prefixes = SecretPrefixes::new(&config.secret_prefixes);
    let types: Vec<&Card> = cards.iter().filter(|c| c.kind == CardKind::Type).collect();

    progress.phase("verify purpose", Some(types.len()));
    let cache = Cache::new(config.corpus_dir.join(CACHE_DIR_NAME));
    let mut report = PurposeReport {
        types: types.len() as u64,
        ..PurposeReport::default()
    };
    let mut records: Vec<PurposeRecord> = Vec::new();
    for (done, card) in types.iter().enumerate() {
        let request = Request::new(card, settings, lang, &prefixes);
        let key = request.key();
        let answer = match cache.get::<Answer>(&key) {
            Some(answer) => {
                report.cached += 1;
                Some(answer)
            }
            None => {
                let prompt = request.prompt();
                report.estimated_tokens += prompt.len().div_ceil(CHARS_PER_TOKEN) as u64;
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
                                    sites: report.types,
                                    source,
                                })?;
                        match request.validate(&reply) {
                            Some(sentence) => {
                                let answer = Answer { sentence };
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
            report.written += 1;
            records.push(PurposeRecord {
                identity: card.identity.clone(),
                key,
                sentence: answer.sentence,
            });
        }
        progress.tick(done + 1);
    }
    progress.phase_done();

    if backend.is_some() {
        let layer = PurposeLayer {
            schema: LAYER_SCHEMA,
            prompt_version: PROMPT_VERSION,
            redaction_version: REDACTION_VERSION,
            settings: settings.clone(),
            lang: lang_code(lang).to_string(),
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

/// The layer as written: pretty JSON with sorted keys and a final newline, so
/// two runs that got the same answers write the same bytes.
pub fn to_json(layer: &PurposeLayer) -> String {
    let value = serde_json::to_value(layer).expect("a layer is plain serializable data");
    let mut json = serde_json::to_string_pretty(&value).expect("a JSON value always serializes");
    json.push('\n');
    json
}

/// Read the purpose layer in `corpus_dir` and put every current record's
/// sentence on its type's card ([`crate::card::story::Story::model_purpose`]),
/// counting what became of each record. `cards` must not have been rendered
/// yet; nothing but that one field changes.
pub fn apply_layer(
    corpus_dir: &Path,
    cards: &mut [Card],
    lang: Lang,
    secret_prefixes: &[String],
) -> Result<ModelPurpose, LayerError> {
    let path = corpus_dir.join(LAYER_FILENAME);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(LayerError::PurposeMissing(path))
        }
        Err(source) => return Err(LayerError::Read { path, source }),
    };
    let layer: PurposeLayer = serde_json::from_str(&text).map_err(|source| LayerError::Parse {
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
    let types: HashMap<&str, usize> = cards
        .iter()
        .enumerate()
        .filter(|(_, c)| c.kind == CardKind::Type)
        .map(|(i, c)| (c.identity.as_str(), i))
        .collect();
    let mut counts = ModelPurpose::default();
    let mut current: Vec<(usize, String)> = Vec::new();
    for record in &layer.records {
        let Some(&i) = types.get(record.identity.as_str()) else {
            counts.unmatched += 1;
            continue;
        };
        if Request::new(&cards[i], &layer.settings, lang, &prefixes).key() != record.key {
            counts.stale += 1;
            continue;
        }
        counts.used += 1;
        current.push((i, record.sentence.clone()));
    }
    for (i, sentence) in current {
        cards[i].story.model_purpose = Some(sentence);
    }
    Ok(counts)
}

/// The language a sentence is asked in, as the layer and the request name it.
fn lang_code(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "en",
        Lang::Cs => "cs",
    }
}

/// The canonical request for one type: the facts the prompt shows, plus what
/// decides how they are answered. Its SHA-256 is the cache key; the field
/// order is the serialization order, so it is part of the key.
#[derive(Debug, Serialize)]
struct Request<'s> {
    /// Which layer the request is for — what keeps its keys apart from the
    /// site verdicts' in the shared cache.
    layer: &'static str,
    prompt_version: u32,
    redaction_version: u32,
    settings: &'s BackendSettings,
    lang: &'static str,
    identity: String,
    name: String,
    kind: String,
    signature: String,
    /// The deterministic purpose, in English whatever the card's language,
    /// each sentence with its origin token.
    purpose: Vec<String>,
    bases: Vec<String>,
    uses: Vec<String>,
    used_by: Vec<String>,
    database: Vec<String>,
    anchors: Vec<String>,
}

impl<'s> Request<'s> {
    fn new(
        card: &Card,
        settings: &'s BackendSettings,
        lang: Lang,
        prefixes: &SecretPrefixes,
    ) -> Request<'s> {
        let redact = |text: &str| redact_secret_block(text, prefixes).0;
        let story = &card.story;
        let labels = |peers: &[crate::card::story::Peer]| -> Vec<String> {
            peers
                .iter()
                .take(NAMED_FACTS)
                .map(|p| redact(&p.label))
                .collect()
        };
        // Distinct, by access then object: what the type does to the
        // database, not where — the detail says where.
        let database: BTreeSet<String> = card
            .navigators
            .db
            .iter()
            .map(|row| format!("{} {}", row.access.word(), row.object))
            .collect();
        // Distinct texts in the order the detail lists them. A withheld text
        // is no anchor and is never sent; only its reason exists.
        let mut seen: HashSet<&str> = HashSet::new();
        let anchors = card
            .navigators
            .anchors
            .iter()
            .filter_map(|row| match &row.body {
                AnchorBody::Text(text) => Some(text.text.as_str()),
                AnchorBody::Withheld(_) => None,
            })
            .filter(|text| seen.insert(text))
            .take(NAMED_FACTS)
            .map(&redact)
            .collect();
        Request {
            layer: "purpose",
            prompt_version: PROMPT_VERSION,
            redaction_version: REDACTION_VERSION,
            settings,
            lang: lang_code(lang),
            identity: redact(&card.identity),
            name: redact(&card.title),
            kind: redact(&card.kind_phrase()),
            signature: redact(card.signature.as_deref().unwrap_or("")),
            purpose: story
                .purpose
                .iter()
                .map(|s| redact(&format!("{} ({})", s.text(Lang::En), s.origin.token())))
                .collect(),
            bases: story
                .bases
                .iter()
                .map(|(peer, word)| redact(&format!("{word} {}", peer.label)))
                .collect(),
            uses: labels(&story.uses),
            used_by: labels(&story.users),
            database: database
                .iter()
                .take(NAMED_FACTS)
                .map(|access| redact(access))
                .collect(),
            anchors,
        }
    }

    /// SHA-256 of the canonical request, lowercase hex.
    fn key(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("a request is plain serializable data");
        let digest = Sha256::digest(&bytes);
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The facts as the prompt shows them — and as the answer is checked
    /// against: the guardrail licenses exactly what is written here.
    fn facts(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "Type: `{}`", self.identity);
        let _ = writeln!(s, "Name: `{}`", self.name);
        let _ = writeln!(s, "Kind: {}", self.kind);
        if !self.signature.is_empty() {
            let _ = writeln!(s, "Signature: `{}`", self.signature);
        }
        let mut list = |title: &str, items: &[String], quoted: bool| {
            if items.is_empty() {
                return;
            }
            let _ = writeln!(s, "{title}:");
            for item in items {
                if quoted {
                    let _ = writeln!(s, "- \"{item}\"");
                } else {
                    let _ = writeln!(s, "- {item}");
                }
            }
        };
        list("What the code says it is for", &self.purpose, false);
        list("Builds on", &self.bases, false);
        list("Uses", &self.uses, false);
        list("Used by", &self.used_by, false);
        list("Database", &self.database, false);
        list("Texts in its code", &self.anchors, true);
        s
    }

    /// The prompt: the instruction, then the facts.
    fn prompt(&self) -> String {
        let language = match self.lang {
            "cs" => "Czech",
            _ => "English",
        };
        format!(
            "In one sentence of {language}, say what the type below is for. Use only the facts \
             given: name no type, member, table or number they do not contain. Answer with the \
             sentence alone — no heading, no list, no quotes around it.\n\n{}",
            self.facts()
        )
    }

    /// The sentence, when the reply passes the checks in the module docs:
    /// one line, at most [`SENTENCE_CAP`] bytes once its whitespace is
    /// collapsed, and nothing the guardrail rejects against the facts sent.
    fn validate(&self, reply: &str) -> Option<String> {
        if reply.lines().filter(|l| !l.trim().is_empty()).count() != 1 {
            return None;
        }
        let facts = self.facts();
        let licensed = LicensedSet {
            numbers: extract_numbers(&facts),
            identifiers: extract_identifiers(&facts),
        };
        if guardrail::check(reply, &licensed) != Verdict::Ok {
            return None;
        }
        let sentence = reply.split_whitespace().collect::<Vec<_>>().join(" ");
        (sentence.len() <= SENTENCE_CAP).then_some(sentence)
    }
}
