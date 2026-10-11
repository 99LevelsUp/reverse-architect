use super::external::{
    is_doc_comment, last_segment, literal_secrets, literal_text, names_a_message,
    redact_secret_block, secret_name, secret_pattern, segments, Lang, LineIndex, SecretPrefixes,
    SegKind, Segment,
};
use crate::graph::{Fact, FactKind, FactValue};
use regex::Regex;
use std::sync::OnceLock;

// ---- The secret filter of the second layer (plan 2a, KTD16) ----

/// Which secret a value is by the name it is given, compared with or
/// written to (`SmtpPassword`, `token`, `Errors.Password`) — the rule
/// [`literal_secrets`] reads the assigned name with, for a name the second
/// layer's model gives instead of the code on the line. A value of several
/// words is a message more often than a secret, so a name with a word of a
/// message (`lblPassword`, `errorToken`) speaks for none of those.
pub(crate) fn named_secret(name: &str, value: &str) -> Option<&'static str> {
    let pattern = secret_name(last_segment(name))?;
    let words = value
        .trim_matches(|c: char| c == '"' || c == '@' || c == '$' || c == '\'')
        .trim()
        .contains(char::is_whitespace);
    (!words || !names_a_message(name)).then_some(pattern)
}

/// One string literal of a source file, as the second layer pairs it.
struct SourceLiteral {
    /// Where its content starts: 1-based line, 1-based column counted in
    /// characters — the units of the model's positions.
    at: (u32, u32),
    /// The pattern it is withheld under, when it is.
    pattern: Option<&'static str>,
    /// How it is written: its content with the quotes around it, when they
    /// are quotes; a text the model quotes from the source holds it so.
    spelling: String,
}

/// What a source file withholds, read for the second layer's model of its
/// bodies (plan 2a, KTD16) — the decisions the first layer makes about its
/// literals, extended by what only the second layer quotes.
///
/// Per literal, in source order: what [`literal_secrets`] decides, which
/// stays the first layer's own and unchanged; the literals of a key written
/// one per line through the one holding `-----END`, as [`emit_file`]
/// withholds them; a literal holding a key-shaped word ([`is_key_word`]),
/// which the first layer only keeps out of its anchors; and a literal
/// compared with a name that says what it is, on either side of `==` or
/// `!=` (`token == "…"`, `"…" != token`) — the partner the first layer reads
/// in one direction and for `==` alone.
///
/// Beside them, the spellings a text of the model may quote: each withheld
/// literal as written, and each comment that names a secret, which a
/// condition's text quotes with the code around it.
pub(crate) struct SourceSecrets {
    literals: Vec<SourceLiteral>,
    spellings: Vec<(String, &'static str)>,
}

impl SourceSecrets {
    /// The file at `path` read for the second layer, or `None` when the host
    /// has no lexer for its language.
    pub(crate) fn read(path: &str, src: &str, prefixes: &SecretPrefixes) -> Option<SourceSecrets> {
        let lang = Lang::from_path(path)?;
        let segs = segments(lang, src);
        let lines = LineIndex::new(src);
        let named = literal_secrets(src, &segs, &lines, prefixes);
        // Columns are counted on from the literal before on the same line, so
        // a minified line of many literals is counted once, not per literal.
        let mut last: (u32, usize, u32) = (0, 0, 0);
        let mut at = |offset: usize| {
            let line = lines.line(offset);
            let (from, column) = if last.0 == line && last.1 <= offset {
                (last.1, last.2)
            } else {
                (lines.start(line), 1)
            };
            let column = column + src[from..offset].chars().count() as u32;
            last = (line, offset, column);
            (line, column)
        };
        let mut literals = Vec::new();
        let mut spellings = Vec::new();
        let mut in_key = false;
        for (i, seg) in segs.iter().enumerate() {
            let text = &src[seg.start..seg.end];
            match seg.kind {
                SegKind::Code => {}
                SegKind::Comment => {
                    let (_, redacted) = redact_secret_block(text, prefixes);
                    let pattern = if redacted {
                        Some(secret_pattern(text, prefixes).unwrap_or("secret value"))
                    } else {
                        without_long_tokens(text).1.then_some("long token")
                    };
                    if let Some(pattern) = pattern {
                        spellings.push((text.to_string(), pattern));
                    }
                }
                SegKind::Str { .. } => {
                    let pattern = if in_key {
                        in_key = !text.contains("-----END");
                        Some("private key")
                    } else if let Some(pattern) = named[i] {
                        in_key = pattern == "private key"
                            && text.contains("-----BEGIN")
                            && !text.contains("-----END");
                        Some(pattern)
                    } else {
                        compared_name(src, &segs, i)
                            .and_then(|name| named_secret(name, text))
                            .or_else(|| {
                                without_long_tokens(&literal_text(src, seg))
                                    .1
                                    .then_some("long token")
                            })
                    };
                    let b = src.as_bytes();
                    let quoted = seg.start > 0
                        && seg.end < b.len()
                        && matches!(b[seg.start - 1], b'"' | b'\'' | b'`')
                        && b[seg.end] == b[seg.start - 1];
                    let spelling = if quoted {
                        &src[seg.start - 1..seg.end + 1]
                    } else {
                        text
                    };
                    if let Some(pattern) = pattern.filter(|_| !text.is_empty()) {
                        spellings.push((spelling.to_string(), pattern));
                    }
                    literals.push(SourceLiteral {
                        at: at(seg.start),
                        pattern,
                        spelling: spelling.to_string(),
                    });
                }
            }
        }
        Some(SourceSecrets {
            literals,
            spellings,
        })
    }

    /// The literal of the file whose content starts within `start..=end`
    /// (line, column in characters) — the place the model gives a literal,
    /// which starts at its opening token (`@`, `$`, the quotes) — as the
    /// pattern it is withheld under and its spelling; `None` when no literal
    /// of the file starts there.
    pub(crate) fn literal(
        &self,
        start: (u32, u32),
        end: (u32, u32),
    ) -> Option<(Option<&'static str>, &str)> {
        let first = self.literals.partition_point(|l| l.at < start);
        self.literals
            .get(first)
            .filter(|l| l.at <= end)
            .map(|l| (l.pattern, l.spelling.as_str()))
    }

    /// Every spelling the file withholds, with its pattern, in source order.
    pub(crate) fn spellings(&self) -> &[(String, &'static str)] {
        &self.spellings
    }
}

/// A block comment written right after a string literal, or right after the
/// call the literal closes the arguments of —
/// `GetResourceString("EditorView_checkErrors") /*"Kontrola chyb"*/` — as the
/// lexer split it (plan 2a, U7: R18). One lexer segment as it is: never
/// joined with the comments around it and never under the anchors' merging or
/// their cap ([`crate::graph::ANCHOR_CAP`]), which a type with many comments
/// reaches long before its menu is read.
pub(crate) struct CommentAfterLiteral {
    /// Where the literal's content starts: 1-based line, 1-based column in
    /// characters — the point [`SourceSecrets::literal`] pairs a model's
    /// literal by.
    pub(crate) literal: (u32, u32),
    /// Where the comment's `/*` is, in the same units.
    pub(crate) start: (u32, u32),
    /// Where the comment's last character is.
    pub(crate) end: (u32, u32),
    /// The comment without `/*` and `*/`, its whitespace collapsed, and
    /// without the quotes around it when it is quoted whole. Unredacted: the
    /// caller filters it.
    pub(crate) text: String,
}

/// Every [`CommentAfterLiteral`] of the file at `path`, in source order, or
/// `None` when the host has no lexer for its language.
///
/// Only what stands between the literal and the comment decides: the
/// literal's closing quotes, closing parentheses and spaces, on one line. A
/// comma, an operator or a line break between them makes the comment
/// something else, and so does a doc comment, a line comment or an empty
/// one. No other comment of the file is read.
pub(crate) fn comments_after_literals(path: &str, src: &str) -> Option<Vec<CommentAfterLiteral>> {
    let lang = Lang::from_path(path)?;
    let segs = segments(lang, src);
    let lines = LineIndex::new(src);
    let at = |offset: usize| {
        let line = lines.line(offset);
        let column = 1 + src[lines.start(line)..offset].chars().count() as u32;
        (line, column)
    };
    let mut out = Vec::new();
    for (i, seg) in segs.iter().enumerate() {
        if !matches!(seg.kind, SegKind::Str { .. }) {
            continue;
        }
        let Some(comment) = segs[i + 1..]
            .iter()
            .take(2)
            .take_while(|s| !matches!(s.kind, SegKind::Str { .. }))
            .find(|s| s.kind == SegKind::Comment)
        else {
            continue;
        };
        let text = &src[comment.start..comment.end];
        let gap = &src[seg.end..comment.start];
        if !text.starts_with("/*")
            || is_doc_comment(lang, text)
            || !gap
                .chars()
                .all(|c| matches!(c, '"' | '\'' | ')' | ' ' | '\t'))
        {
            continue;
        }
        let inner = text.strip_prefix("/*").unwrap_or(text);
        let inner = inner.strip_suffix("*/").unwrap_or(inner);
        let inner = inner.split_whitespace().collect::<Vec<_>>().join(" ");
        let inner = match inner.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            Some(quoted) => quoted.trim().to_string(),
            None => inner,
        };
        if inner.is_empty() {
            continue;
        }
        out.push(CommentAfterLiteral {
            literal: at(seg.start),
            start: at(comment.start),
            end: at(comment.end - 1),
            text: inner,
        });
    }
    Some(out)
}

/// The name a literal is compared with by `==` or `!=`: the path written
/// right before the operator, or right after it when the literal comes
/// first — not a call's name, which names no value.
fn compared_name<'s>(src: &'s str, segs: &[Segment], i: usize) -> Option<&'s str> {
    static AFTER: OnceLock<Regex> = OnceLock::new();
    static BEFORE: OnceLock<Regex> = OnceLock::new();
    let code = |at: usize| {
        segs.get(at)
            .filter(|seg| seg.kind == SegKind::Code)
            .map(|seg| &src[seg.start..seg.end])
    };
    let after = AFTER.get_or_init(|| {
        Regex::new(
            r"^\s*[!=]=\s*((?:[\p{L}_][\p{L}\p{N}_]*\s*\.\s*)*[\p{L}_][\p{L}\p{N}_]*)\s*(\()?",
        )
        .unwrap()
    });
    if let Some(caps) = code(i + 1).and_then(|c| after.captures(c)) {
        if caps.get(2).is_none() {
            return Some(caps.get(1)?.as_str());
        }
    }
    let before = BEFORE.get_or_init(|| {
        Regex::new(r"((?:[\p{L}_][\p{L}\p{N}_]*\s*\.\s*)*[\p{L}_][\p{L}\p{N}_]*)\s*[!=]=\s*$")
            .unwrap()
    });
    // The literal's own line of the code before it: a minified line is read
    // once per literal on it, never the whole file before it.
    code(i.wrapping_sub(1))
        .and_then(|c| c.rsplit('\n').next())
        .and_then(|c| before.captures(c))
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str())
}

/// Whether a word is one long token shaped like a key or a hash (plan B,
/// KTD11 rule 3): at least twenty characters with no space, every one of them
/// from the hex or base64 alphabet, a digit among them, and either nothing but
/// hex digits or both cases of letters. That last pair is what keeps a long
/// identifier out of it — `SetScheduledTasksInvalid` has no digit, and
/// `ERR_CODE_2024_INVALID_STATE` has no lower case letter.
pub(crate) fn is_long_token(word: &str) -> bool {
    let b = word.as_bytes();
    b.len() >= 20
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'=' | b'_' | b'-'))
        && b.iter().any(u8::is_ascii_digit)
        && (b.iter().all(u8::is_ascii_hexdigit)
            || (b.iter().any(u8::is_ascii_uppercase) && b.iter().any(u8::is_ascii_lowercase)))
}

/// A literal's text with every key-shaped word cut out (KTD11 rule 3), and
/// whether one was. A word is tested with its escape sequences removed — a
/// key line written as `"MIIE…\n"` carries `\` and `n` in its source text,
/// and no key alphabet holds a backslash — and trimmed of the punctuation
/// around it, as a comment's word is.
pub(crate) fn without_long_tokens(text: &str) -> (std::borrow::Cow<'_, str>, bool) {
    use std::borrow::Cow;
    let trimmed = text.trim();
    // One word — most literals — is tested whole, with no split and no copy.
    if !trimmed.contains(char::is_whitespace) {
        return if is_key_word(trimmed) {
            (Cow::Borrowed(""), true)
        } else {
            (Cow::Borrowed(text), false)
        };
    }
    let mut long = false;
    let kept: Vec<&str> = text
        .split_whitespace()
        .filter(|w| {
            let key = is_key_word(w);
            long |= key;
            !key
        })
        .collect();
    if long {
        (Cow::Owned(kept.join(" ")), true)
    } else {
        (Cow::Borrowed(text), false)
    }
}

/// Whether a word, with its escape sequences removed and, failing that, the
/// punctuation around it trimmed, is key-shaped ([`is_long_token`]).
fn is_key_word(word: &str) -> bool {
    let word = unescaped(word);
    is_long_token(&word) || is_long_token(word.trim_matches(|c: char| !c.is_ascii_alphanumeric()))
}

/// A doc comment's text with every key-shaped word cut out (KTD11 rule 3),
/// line by line — a line that loses one keeps its indentation and the words
/// around it — and the 0-based index of each line that lost one. `None` when
/// no word was cut.
///
/// A doc comment is no anchor, so [`comment_anchors`] never reads it; it is
/// cut where it enters the graph instead, so that every reader of
/// `DocComment` — the card's quote and purpose, the map row, the manifest
/// header, the model's input — inherits the cut.
pub(crate) fn doc_without_long_tokens(text: &str) -> Option<(String, Vec<usize>)> {
    let mut cut: Vec<usize> = Vec::new();
    let mut lines: Vec<std::borrow::Cow<'_, str>> = Vec::new();
    for (n, line) in text.split('\n').enumerate() {
        let (kept, long) = if line.contains(['<', '>']) {
            without_long_tokens_in_markup(line)
        } else {
            without_long_tokens(line)
        };
        if long {
            cut.push(n);
            let indent = &line[..line.len() - line.trim_start().len()];
            lines.push(format!("{indent}{}", kept.trim()).into());
        } else {
            lines.push(std::borrow::Cow::Borrowed(line));
        }
    }
    (!cut.is_empty()).then(|| (lines.join("\n"), cut))
}

/// [`without_long_tokens`] for a doc comment's line that holds markup. A
/// key written right between two tags (`<c>KEY</c>`, `<summary>KEY</summary>`)
/// is one whitespace word with the tags glued to it, and no key alphabet
/// holds `<` or `>` — so each word is also read run by run between them, and
/// a key-shaped run is cut out with the markup around it kept: `<c></c>`.
fn without_long_tokens_in_markup(line: &str) -> (std::borrow::Cow<'_, str>, bool) {
    let (whole, long) = without_long_tokens(line);
    let mut cut = long;
    let words: Vec<String> = whole
        .split_whitespace()
        .map(|word| {
            if !word.contains(['<', '>']) {
                return word.to_string();
            }
            let mut kept = String::with_capacity(word.len());
            for run in word.split_inclusive(['<', '>']) {
                let text = run.trim_end_matches(['<', '>']);
                if is_key_word(text) {
                    cut = true;
                    kept.push_str(&run[text.len()..]);
                } else {
                    kept.push_str(run);
                }
            }
            kept
        })
        .collect();
    if cut {
        (std::borrow::Cow::Owned(words.join(" ")), true)
    } else {
        (std::borrow::Cow::Borrowed(line), false)
    }
}

/// [`doc_without_long_tokens`] applied to a `DocComment` fact in place, and
/// the skips it leaves: one `AnchorSkipped("long token")` per line that lost
/// a word, at that line — the reason and the place, never the text, as
/// [`comment_anchors`] records a comment line's (R10). For the caller to put
/// on the same node; empty, and nothing allocated, for every other fact.
pub(crate) fn cut_doc_long_tokens(fact: &mut Fact) -> Vec<Fact> {
    if fact.kind != FactKind::DocComment {
        return Vec::new();
    }
    let FactValue::Text(text) = &fact.value else {
        return Vec::new();
    };
    let Some((kept, cut)) = doc_without_long_tokens(text) else {
        return Vec::new();
    };
    fact.value = FactValue::Text(kept);
    let (first, last) = (fact.provenance.span.start(), fact.provenance.span.end());
    cut.into_iter()
        .map(|n| {
            // A doc comment's text holds one line per source line; where the
            // two disagree the skip stays inside the comment's own range.
            let line = first.map(|f| {
                let at = f.saturating_add(u32::try_from(n).unwrap_or(u32::MAX));
                last.map_or(at, |l| at.min(l))
            });
            let mut prov = fact.provenance.clone();
            prov.span = crate::graph::Span::at(line);
            Fact::new(
                FactKind::AnchorSkipped,
                FactValue::Text("long token".to_string()),
                prov,
            )
        })
        .collect()
}

/// `word` without the escape sequences a key's line is written with in a
/// literal: `\n`, `\r`, `\t`, `\\` and `\"`.
fn unescaped(word: &str) -> std::borrow::Cow<'_, str> {
    if !word.contains('\\') {
        return std::borrow::Cow::Borrowed(word);
    }
    let mut out = String::with_capacity(word.len());
    let mut chars = word.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && matches!(chars.peek(), Some('n' | 'r' | 't' | '\\' | '"')) {
            chars.next();
            continue;
        }
        out.push(c);
    }
    std::borrow::Cow::Owned(out)
}
