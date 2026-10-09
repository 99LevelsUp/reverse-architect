//! Secret redaction shared by the host and every language plugin (R22).
//!
//! Text a reverse-architect artifact quotes — a doc comment, a signature, a
//! string literal — can carry a credential the repository leaked. This crate
//! recognizes the shapes it can name and replaces the whole line, so the value
//! never reaches a card, the manifest or any other export.
//!
//! It lives in its own crate because two builds need exactly the same filter:
//! the host, which scans literals and re-redacts everything a plugin returns,
//! and each plugin component, which redacts what it emits in the first place.
//! One copy of the pattern table means the two can never disagree about what a
//! secret looks like.

use regex::Regex;
use std::sync::OnceLock;

/// Token prefixes that identify a credential with no keyword anywhere near them.
///
/// A GitHub PAT or an AWS access key ID is recognizable by its own first
/// characters; nothing else in a source file starts that way, so this rule needs
/// no `password =` to fire and has essentially no false-positive surface. These
/// are also the credentials that actually get leaked and actually get abused —
/// GitHub and AWS scan for exactly these shapes themselves.
///
/// Case-sensitive, and the prefix must be followed by at least
/// [`MIN_TOKEN_TAIL`] token characters: prose mentioning `sk-` or `AKIA` in
/// passing must not cost a reader the whole line it sits on.
///
/// `--secret-prefix` adds to this list rather than replacing it, so a company
/// token shape can be recognized without a rebuild and without giving up the
/// public ones.
#[derive(Debug, Clone)]
pub struct SecretPrefixes {
    re: Regex,
    /// The operator's own prefixes, as given — so a host can hand the same
    /// list on to a plugin.
    extra: Vec<String>,
}

/// The public token prefixes recognized out of the box.
const BUILTIN_PREFIXES: &[&str] = &[
    // OpenAI and the many APIs that copied its shape
    "sk-",
    // GitHub: PAT (classic and fine-grained), OAuth, user, server, refresh
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    // GitLab personal access token
    "glpat-",
    // Slack: bot, app, user, refresh, service
    "xoxb-",
    "xoxa-",
    "xoxp-",
    "xoxr-",
    "xoxs-",
    // AWS access key id: long-term and temporary
    "AKIA",
    "ASIA",
    // Google API key, and an OAuth access token
    "AIza",
    "ya29.",
    // npm, DigitalOcean, Square, SendGrid, Hugging Face
    "npm_",
    "dop_v1_",
    "sq0atp-",
    "SG.",
    "hf_",
];

/// How many token characters must follow a prefix before it counts as a token.
///
/// Every real credential of these shapes is far longer. Sixteen is high enough
/// that a sentence naming a prefix stays untouched and low enough that no genuine
/// token slips under it.
const MIN_TOKEN_TAIL: usize = 16;

impl SecretPrefixes {
    /// The built-in list plus `extra`, compiled once.
    ///
    /// User prefixes are regex-escaped: a `--secret-prefix` is a literal, not a
    /// pattern, so a stray `.` or `+` in a company token shape cannot silently
    /// widen the filter.
    pub fn new(extra: &[String]) -> SecretPrefixes {
        let mut alternatives: Vec<String> =
            BUILTIN_PREFIXES.iter().map(|p| regex::escape(p)).collect();
        alternatives.extend(
            extra
                .iter()
                .filter(|p| !p.trim().is_empty())
                .map(|p| regex::escape(p.trim())),
        );
        let pattern = format!(
            r"(?:{})[A-Za-z0-9_\-.]{{{MIN_TOKEN_TAIL},}}",
            alternatives.join("|")
        );
        SecretPrefixes {
            re: Regex::new(&pattern).expect("every alternative is regex-escaped"),
            extra: extra
                .iter()
                .filter(|p| !p.trim().is_empty())
                .map(|p| p.trim().to_string())
                .collect(),
        }
    }

    /// The built-in list alone, built once per process.
    ///
    /// For the places that filter text a configured pass has ALREADY filtered —
    /// the card renderer's belt-and-braces sweep — where re-reading the run's
    /// configuration would buy nothing: anything a company prefix matched is
    /// already replaced and already carries the marker.
    pub fn builtin() -> &'static SecretPrefixes {
        static BUILTIN: OnceLock<SecretPrefixes> = OnceLock::new();
        BUILTIN.get_or_init(|| SecretPrefixes::new(&[]))
    }

    /// The prefixes added on top of the built-in list, trimmed, in order.
    pub fn extra(&self) -> &[String] {
        &self.extra
    }

    pub fn matches(&self, lit: &str) -> bool {
        self.re.is_match(lit)
    }
}

impl Default for SecretPrefixes {
    fn default() -> Self {
        SecretPrefixes::new(&[])
    }
}

/// Which secret pattern a literal's SHAPE matches, if any. Ordered: the most
/// specific name a reader can act on wins, deterministically.
///
/// ## What this does NOT catch, on purpose
///
/// Every rule below needs a SHAPE: an assignment separator, a named block, a URL
/// authority, or a known token prefix. A secret spelled out in prose with none of
/// those — `the default password is hunter2` — passes through. Recognizing it
/// would mean firing on the keyword alone, and because a match replaces the whole
/// line, `// Validates the password strength` would be replaced too. In a
/// codebase where that word appears in hundreds of ordinary comments, that trade
/// destroys more than it protects.
///
/// The honest boundary: RA reads a repository. A credential committed there is
/// already leaked, and R22's job is to stop RA making it worse by copying it into
/// an artifact that travels further. Being the repository's secret scanner is a
/// different job, with different tools (gitleaks, trufflehog) and a maintained
/// pattern set — run one in CI. This filter catches the shapes it can name.
pub fn secret_pattern(lit: &str, prefixes: &SecretPrefixes) -> Option<&'static str> {
    RULES
        .iter()
        .find_map(|rule| rule(lit, prefixes))
        .map(|(_, pattern)| pattern)
}

/// What every rule of [`secret_pattern`] finds in one text, not just the
/// first that fires — so a caller asking about many PREFIXES of one text can
/// tell where the answer stays the same without asking at every prefix.
///
/// Each rule's finding only ever grows as the text is read further: none of
/// them looks past the end of what it matched (no `$`, no trailing `\b`), so
/// a match inside a prefix is a match inside every longer prefix, and the
/// leftmost match a rule names can only move left. Two prefixes with equal
/// shapes therefore have the same shapes at every prefix between them — and
/// the same [`SecretShapes::pattern`], which is [`secret_pattern`] of each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretShapes([Option<(usize, &'static str)>; RULE_COUNT]);

impl SecretShapes {
    /// The one pattern [`secret_pattern`] names for the same text.
    pub fn pattern(&self) -> Option<&'static str> {
        self.0.iter().flatten().next().map(|(_, pattern)| *pattern)
    }
}

/// Every rule of [`secret_pattern`] read over `lit`.
pub fn secret_shapes(lit: &str, prefixes: &SecretPrefixes) -> SecretShapes {
    SecretShapes(RULES.map(|rule| rule(lit, prefixes)))
}

/// One rule of the table: the pattern it names, with where in the text its
/// finding starts when the name depends on WHICH match it read (`0` when it
/// does not), or `None`.
type Rule = fn(&str, &SecretPrefixes) -> Option<(usize, &'static str)>;

const RULE_COUNT: usize = 10;

/// The rules in the order [`secret_pattern`] tries them: the most specific
/// name a reader can act on first.
const RULES: [Rule; RULE_COUNT] = [
    private_key,
    url_credentials,
    provider_token,
    password,
    czech,
    sql_password,
    markup_password,
    token,
    markup_token,
    connection_string,
];

fn private_key(lit: &str, _: &SecretPrefixes) -> Option<(usize, &'static str)> {
    (lit.contains("-----BEGIN") && lit.contains("PRIVATE KEY")).then_some((0, "private key"))
}

/// `mongodb://app:hunter2@db01:27017/orbit` — a password carried in a URL's
/// userinfo, with no keyword anywhere on the line for the rules below to see.
/// The external extractor already calls this shape a credential, but only for string
/// literals; a doc comment reached the card through this function alone.
///
/// The colon is required. `git+ssh://git@github.com/org/repo` is a userinfo
/// too and appears in half the READMEs ever written, but it carries no secret
/// — redacting the line it sits on would cost real documentation for nothing.
fn url_credentials(lit: &str, _: &SecretPrefixes) -> Option<(usize, &'static str)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re =
        RE.get_or_init(|| Regex::new(r"(?i)\b[a-z][a-z0-9+.\-]*://[^\s/@]+:[^\s/@]+@").unwrap());
    re.is_match(lit).then_some((0, "credentials in a URL"))
}

/// A token recognizable by its own first characters, keyword or not. Checked
/// before the keyword rules because it names the credential more precisely:
/// "provider token" tells a reader which secret to rotate, "token" does not.
fn provider_token(lit: &str, prefixes: &SecretPrefixes) -> Option<(usize, &'static str)> {
    prefixes.matches(lit).then_some((0, "provider token"))
}

/// [`QUOTED`] may stand between the keyword and its separator: a key written
/// as JSON (`"password": "…"`), as JSON escaped inside a string literal
/// (`\"password\":\"…\"`) or in single quotes has its closing quote there,
/// and an assignment otherwise fully in shape was missed for it.
fn password(lit: &str, _: &SecretPrefixes) -> Option<(usize, &'static str)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(&format!(r"(?i)\b(?:password|passwd|pwd){QUOTED}\s*[=:]")).unwrap()
    });
    re.is_match(lit).then_some((0, "password"))
}

/// The closing quote of a quoted key, escaped or not: a run of quotes and
/// backslashes only. A `]` still stops it, so an indexer naming a message
/// (`Errors["Password"] = "Heslo je krátké"`) stays out of the rules that
/// use it.
const QUOTED: &str = r#"["'\\]*"#;

/// The Czech words, and SQL's own spelling (plan B, KTD11 rule 4). Only as
/// an assignment with a value on the same line: "Chybné heslo" is the
/// message a support desk searches for, and a bare label (`Heslo:`) is how a
/// form names the field — neither carries a value, and a match replaces
/// the whole line. The SQL form needs its quoted value for the same reason
/// (`WITH PASSWORD 'abc'`, `CREATE USER x PASSWORD 'abc'`); with `=` it is
/// the rule above.
///
/// A quoted key is read as in [`password`]; its value may open with an
/// escaped quote (`\"heslo\":\"…`), and an empty one (`\"heslo\":\"\"`) is
/// still no value.
///
/// The one rule whose name depends on the match it reads, so it says where
/// that match starts.
fn czech(lit: &str, _: &SecretPrefixes) -> Option<(usize, &'static str)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(&format!(
            r#"(?i)\b(?:hesl[oa]|klíč){QUOTED}\s*[=:]\s*\\?["']?[^\s"'\\]"#
        ))
        .unwrap()
    });
    let m = re.find(lit)?;
    Some((
        m.start(),
        if m.as_str().to_lowercase().starts_with("hesl") {
            "password"
        } else {
            "token"
        },
    ))
}

fn sql_password(lit: &str, _: &SecretPrefixes) -> Option<(usize, &'static str)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)\bpassword\s+'[^'\r\n]+'").unwrap());
    re.is_match(lit).then_some((0, "password"))
}

/// The separator inside each keyword allows a SPACE as well as `_` and `-`:
/// `api key = "…"` is how a person writes it in a comment, and requiring
/// `api_key` there missed an assignment that was otherwise fully in shape.
/// A space and a tab only — never `\s`, which would let a keyword straddle
/// two lines and match text the reader sees as unrelated.
///
/// A quoted key is read as in [`password`].
fn token(lit: &str, _: &SecretPrefixes) -> Option<(usize, &'static str)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(&format!(
            r"(?i)\b(?:access[ \t_-]?token|auth[ \t_-]?token|api[ \t_-]?key|client[ \t_-]?secret|secret[ \t_-]?key){QUOTED}\s*[=:]",
        ))
        .unwrap()
    });
    re.is_match(lit).then_some((0, "token"))
}

/// A password in markup: an element holding a value (`<Password>x`), or an
/// application setting whose key names one (`<add key="DbPassword"
/// value="x" />`, its quotes escaped where it sits in a string literal).
/// The element needs a character of value right after its tag, so an empty
/// element and a tag closing its line stay text; the setting needs the
/// `value=` after its key and reads no further — no rule looks past the end
/// of its match (see [`SecretShapes`]).
fn markup_password(lit: &str, _: &SecretPrefixes) -> Option<(usize, &'static str)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(&markup_pattern("password|passwd|pwd")).unwrap());
    re.is_match(lit).then_some((0, "password"))
}

/// [`markup_password`] for the names of a key or a token. Its own rule, not
/// a second name of one rule, so that neither depends on which match it read.
fn markup_token(lit: &str, _: &SecretPrefixes) -> Option<(usize, &'static str)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(&markup_pattern("secret|api[_-]?key|token")).unwrap());
    re.is_match(lit).then_some((0, "token"))
}

/// The element form and the setting form of a markup rule, for the keyword
/// alternatives `names`.
fn markup_pattern(names: &str) -> String {
    format!(
        r#"(?i)<\s*(?:{names})\s*>[^<\s]|\bkey\s*=\s*["'\\]+[^"'\\]*(?:{names})[^"'\\]*["'\\]+\s+value\s*="#
    )
}

/// A connection string: `;`-separated key=value pairs, at least TWO of them from
/// the canonical key set. One key alone is how a log-format string
/// (`"… Server={4} Sql={5} …"`) looks, and that is not a connection string.
fn connection_string(lit: &str, _: &SecretPrefixes) -> Option<(usize, &'static str)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    if !lit.contains(';') {
        return None;
    }
    let re = RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:server|data source|initial catalog|user id|uid|database|integrated security|trusted_connection|provider|driver|port|host)\s*=",
        )
        .unwrap()
    });
    (re.find_iter(lit).nth(1).is_some()).then_some((0, "connection string"))
}

/// Redact any secret-shaped content in a multi-line text block (R22).
///
/// Returns the text with every offending line replaced and whether anything was.
///
/// ## Why this is block-aware and not line-wise
///
/// [`secret_pattern`] recognizes a SHAPE, and two of those shapes span lines:
///
/// - a PEM block's `-----BEGIN … PRIVATE KEY-----` header matches, but the base64
///   key material on the lines below matches nothing at all — line-wise redaction
///   would replace the header and publish the key,
/// - a label on one line with its value on the next (`password:` then an indented
///   value) has the same asymmetry.
///
/// So a match opens a redaction that stays open until the block plausibly ends: an
/// explicit `-----END`, a blank line, or a line that closes an XML doc tag. Over-
/// redaction is the intended failure direction — a replaced line costs a reader one
/// lookup at the anchor, a published credential cannot be taken back.
///
/// Applied at FACT EMISSION rather than in a projection, so every consumer — the
/// cards, the machine manifest, any future export — inherits it from one place.
/// R22 names the retrieval artifact as well as the card, and a filter that lives in
/// one projection is a filter the other one does not have.
pub fn redact_secret_block(text: &str, prefixes: &SecretPrefixes) -> (String, bool) {
    let mut redacted = false;
    // Set while inside a PEM block; cleared by its `-----END` line.
    let mut in_pem = false;
    // Set when the previous line named a secret but carried no value, so the value
    // is on the line below.
    let mut value_follows = false;
    let mut out: Vec<String> = Vec::new();

    for line in text.lines() {
        if in_pem {
            // A key block that is never closed — quoted as an illustration, cut
            // short by whoever wrote the comment — must not swallow the rest of
            // the text to the end. Real key material contains no blank line and
            // no markup, so either one ends the block as surely as `-----END`,
            // and the boundary line itself is kept: it carries no key bytes.
            if ends_a_key_block(line) {
                in_pem = false;
                out.push(line.to_string());
                continue;
            }
            redacted = true;
            out.push(marker("private key"));
            if line.contains("-----END") {
                in_pem = false;
            }
            continue;
        }
        if value_follows {
            value_follows = false;
            if !line.trim().is_empty() {
                redacted = true;
                out.push(marker("secret value"));
                continue;
            }
        }
        if line.contains("-----BEGIN") && line.contains("PRIVATE KEY") {
            redacted = true;
            in_pem = true;
            out.push(marker("private key"));
            continue;
        }
        match secret_pattern(line, prefixes) {
            Some(pattern) => {
                redacted = true;
                // `password:` with nothing after the separator means the value sits
                // on the next line. Anything else is a complete assignment and the
                // one replaced line is enough — eating the lines below it would
                // swallow ordinary prose.
                value_follows = ends_with_bare_label(line);
                out.push(marker(pattern));
            }
            None => out.push(line.to_string()),
        }
    }
    (out.join("\n"), redacted)
}

/// Whether this line ends an unterminated key block.
///
/// Base64 key material is a solid run of `A-Za-z0-9+/=`, so anything that is
/// blank once its comment marker is stripped, or that opens or closes markup, is
/// past the end of the block whatever the missing `-----END` line would have
/// said. Erring here costs at most one redacted line too few in text that is
/// already malformed; erring the other way silences the rest of the document.
fn ends_a_key_block(line: &str) -> bool {
    let bare = line
        .trim()
        .trim_start_matches(['/', '!', '*', '#', '-'])
        .trim();
    bare.is_empty() || bare.starts_with('<')
}

/// The prefix every replacement carries. Public so a consumer that receives
/// already-filtered text can tell a redaction happened without re-running the
/// pattern table — the filter is idempotent, so a second pass finds nothing and
/// would otherwise report "nothing was removed" about text that lost a line.
pub const REDACTION_MARKER: &str = "[redacted: ";

fn marker(pattern: &str) -> String {
    format!("{REDACTION_MARKER}{pattern} — see the source anchor]")
}

/// Whether the line names a secret and then stops — the value is elsewhere.
///
/// Deliberately narrow: only whitespace and quote/tag punctuation may follow the
/// last separator. A line that carries any word character after it is a complete
/// assignment, and treating it as a continuation would redact the next line of
/// ordinary prose.
fn ends_with_bare_label(line: &str) -> bool {
    match line.rfind([':', '=']) {
        Some(at) => line[at + 1..]
            .chars()
            .all(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '/')),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(text: &str) -> Option<&'static str> {
        secret_pattern(text, &SecretPrefixes::default())
    }

    /// A key written in quotes — JSON, a JSON string escaped inside a
    /// literal — has its closing quote between the keyword and the separator.
    #[test]
    fn a_quoted_key_is_a_keyword_assignment() {
        assert_eq!(
            pattern(r#"{"user":"admin","password":"S3cret"}"#),
            Some("password")
        );
        assert_eq!(
            pattern(r#"{\"user\":\"admin\",\"password\":\"S3cret\"}"#),
            Some("password")
        );
        assert_eq!(pattern(r#"// {"password": "S3cret"}"#), Some("password"));
        assert_eq!(pattern(r#"{'api_key': 'abc'}"#), Some("token"));
        assert_eq!(pattern(r#"{"heslo": "pivo"}"#), Some("password"));
        assert_eq!(pattern(r#"{\"heslo\":\"pivo\"}"#), Some("password"));
        // A Czech key with no value is a label, quoted or not.
        assert_eq!(pattern(r#"{"heslo": ""}"#), None);
        assert_eq!(pattern(r#"{\"heslo\":\"\"}"#), None);
    }

    /// A credential in markup: an element with text, and an application
    /// setting named by its key — escaped or not.
    #[test]
    fn markup_and_settings_carry_a_named_value() {
        assert_eq!(pattern("<Password>x</Password>"), Some("password"));
        assert_eq!(pattern("< pwd >x"), Some("password"));
        assert_eq!(pattern("<ApiKey>x</ApiKey>"), Some("token"));
        assert_eq!(pattern("<secret>x</secret>"), Some("token"));
        assert_eq!(
            pattern(r#"<add key="DbPassword" value="x" />"#),
            Some("password")
        );
        assert_eq!(
            pattern(r#"<add key=\"DbPassword\" value=\"x\" />"#),
            Some("password")
        );
        assert_eq!(
            pattern(r#"<add key='Stripe.ApiKey' value='x' />"#),
            Some("token")
        );
        assert_eq!(
            pattern(r#"<add key="ClientSecret" value="x" />"#),
            Some("token")
        );
    }

    /// What the markup rules leave alone: an element with no value, a key
    /// that names no secret, and prose about a password.
    #[test]
    fn markup_without_a_value_or_a_secret_name_is_text() {
        assert_eq!(pattern("<Password></Password>"), None);
        assert_eq!(pattern("<Password>\n"), None);
        assert_eq!(pattern(r#"<add key="Timeout" value="30" />"#), None);
        assert_eq!(
            pattern("<summary>Checks the password strength.</summary>"),
            None
        );
        assert_eq!(pattern("Zadejte heslo"), None);
        assert_eq!(pattern("Chybné heslo"), None);
        assert_eq!(pattern("Validates the password strength"), None);
    }
}
