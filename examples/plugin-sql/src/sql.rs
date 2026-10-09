//! A small SQL reader: enough of the language to find what a script creates
//! and what it touches, and nothing that needs a grammar.
//!
//! SQL has a dozen dialects and no two agree on procedure bodies, so this does
//! not parse statements. It tokenizes — comments, strings and quoted names
//! handled properly, because that is where a naive scan goes wrong — and then
//! looks for the few shapes every dialect shares:
//!
//! - `CREATE [OR REPLACE | OR ALTER] TABLE | VIEW | PROCEDURE | FUNCTION | TRIGGER name`
//!   declares `name`; a table's column list declares its columns;
//! - `FROM`, `JOIN`, `INTO`, `UPDATE`, `REFERENCES` and a trigger's `ON` name a
//!   table or view;
//! - `EXEC`, `EXECUTE`, `CALL` and `name(` call a procedure or function.
//!
//! A name that matches nothing in the repository simply resolves to nothing —
//! the host drops it — so treating every `name(` as a possible call costs no
//! false edges for built-ins like `count(` or `isnull(`.
//!
//! Unquoted identifiers are folded to lower case, as SQL compares them; a
//! quoted one (`"Name"`, `[Name]`, `` `Name` ``) keeps its spelling.

/// What a script declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decl {
    pub name: String,
    /// The qualifier before the name (`dbo`, `sales.archive`), if any.
    pub schema: Option<String>,
    /// `table`, `view`, `procedure`, `function` or `trigger`.
    pub kind: &'static str,
    pub start: u32,
    pub end: u32,
    /// The line the declaration starts on, trimmed.
    pub signature: String,
    /// The comment block directly above it: text and lines.
    pub doc: Option<(String, u32, u32)>,
    pub columns: Vec<Column>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub signature: String,
    pub line: u32,
}

/// A name the script uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Use {
    pub name: String,
    /// A call (`EXEC p`, `f(…)`) rather than a table or view reference.
    pub call: bool,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// An identifier or keyword, folded unless it was quoted.
    Word { text: String, quoted: bool },
    Punct(char),
    /// A string or number literal: its content is never a name.
    Literal,
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    line: u32,
}

struct Comment {
    start: u32,
    end: u32,
    text: String,
}

fn tokenize(src: &str) -> (Vec<Token>, Vec<Comment>) {
    let chars: Vec<char> = src.chars().collect();
    let mut tokens = Vec::new();
    let mut comments = Vec::new();
    let mut line = 1u32;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else if c == '-' && chars.get(i + 1) == Some(&'-') {
            let start = i + 2;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            comments.push(Comment {
                start: line,
                end: line,
                text: text.trim().to_string(),
            });
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            let first = line;
            let start = i + 2;
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                if chars[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            let text: String = chars[start..i.min(chars.len())].iter().collect();
            i = (i + 2).min(chars.len());
            comments.push(Comment {
                start: first,
                end: line,
                text: text
                    .lines()
                    .map(|l| l.trim().trim_start_matches('*').trim())
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n"),
            });
        } else if c == '\'' {
            let at = line;
            i += 1;
            while i < chars.len() {
                if chars[i] == '\n' {
                    line += 1;
                }
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i += 1;
            tokens.push(Token {
                tok: Tok::Literal,
                line: at,
            });
        } else if c == '"' || c == '[' || c == '`' {
            let close = match c {
                '[' => ']',
                other => other,
            };
            let at = line;
            let start = i + 1;
            i += 1;
            while i < chars.len() && chars[i] != close {
                if chars[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            let text: String = chars[start..i.min(chars.len())].iter().collect();
            i += 1;
            tokens.push(Token {
                tok: Tok::Word { text, quoted: true },
                line: at,
            });
        } else if c.is_ascii_digit() {
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                i += 1;
            }
            tokens.push(Token {
                tok: Tok::Literal,
                line,
            });
        } else if c.is_alphabetic() || c == '_' || c == '@' || c == '#' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '@' | '#' | '$'))
            {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            tokens.push(Token {
                tok: Tok::Word {
                    text: text.to_lowercase(),
                    quoted: false,
                },
                line,
            });
        } else {
            tokens.push(Token {
                tok: Tok::Punct(c),
                line,
            });
            i += 1;
        }
    }
    (tokens, comments)
}

fn word(t: Option<&Token>) -> Option<&str> {
    match t.map(|t| &t.tok) {
        Some(Tok::Word { text, .. }) => Some(text.as_str()),
        _ => None,
    }
}

/// An unquoted word equal to `kw`.
fn is_kw(t: Option<&Token>, kw: &str) -> bool {
    matches!(t.map(|t| &t.tok), Some(Tok::Word { text, quoted: false }) if text == kw)
}

fn is_punct(t: Option<&Token>, p: char) -> bool {
    matches!(t.map(|t| &t.tok), Some(Tok::Punct(c)) if *c == p)
}

/// A dotted name starting at `i`: its segments and the index after it.
fn qualified(tokens: &[Token], mut i: usize) -> Option<(Vec<String>, usize)> {
    let mut parts = vec![word(tokens.get(i))?.to_string()];
    i += 1;
    while is_punct(tokens.get(i), '.') {
        match word(tokens.get(i + 1)) {
            Some(w) => {
                parts.push(w.to_string());
                i += 2;
            }
            None => break,
        }
    }
    Some((parts, i))
}

const MODIFIERS: &[&str] = &[
    "or", "replace", "alter", "global", "local", "temporary", "temp", "unlogged",
    "materialized", "recursive", "definer", "secure", "force", "noforce", "editionable",
];

fn object_kind(w: &str) -> Option<&'static str> {
    Some(match w {
        "table" => "table",
        "view" => "view",
        "procedure" | "proc" => "procedure",
        "function" => "function",
        "trigger" => "trigger",
        _ => return None,
    })
}

/// A `CREATE …` at `i`: the kind, the name's segments, and where the name ends.
fn create_at(tokens: &[Token], i: usize) -> Option<(&'static str, Vec<String>, usize)> {
    if !is_kw(tokens.get(i), "create") {
        return None;
    }
    let mut j = i + 1;
    while word(tokens.get(j)).is_some_and(|w| MODIFIERS.contains(&w)) {
        j += 1;
    }
    let kind = object_kind(word(tokens.get(j))?)?;
    j += 1;
    if is_kw(tokens.get(j), "if") {
        j += 1;
        if is_kw(tokens.get(j), "not") {
            j += 1;
        }
        if is_kw(tokens.get(j), "exists") {
            j += 1;
        }
    }
    let (parts, after) = qualified(tokens, j)?;
    Some((kind, parts, after))
}

const NOT_A_COLUMN: &[&str] = &[
    "constraint", "primary", "foreign", "unique", "check", "index", "key", "period", "exclude",
    "like", "fulltext", "spatial",
];

/// Words that open a parenthesis without being a call.
const NOT_A_CALL: &[&str] = &[
    "in", "exists", "values", "as", "not", "and", "or", "on", "when", "then", "else", "case",
    "table", "into", "from", "where", "select", "key", "references", "check", "primary",
    "unique", "foreign", "index", "returns", "over", "using", "with", "if", "while", "return",
    "join", "union", "all", "any", "some", "by", "default", "set", "is", "between", "like",
    "procedure", "proc", "function", "view", "trigger", "for", "begin", "declare", "output",
];

fn columns(tokens: &[Token], open: usize, lines: &[&str]) -> (Vec<Column>, Vec<Use>) {
    let mut cols = Vec::new();
    let mut uses = Vec::new();
    let mut depth = 0usize;
    let mut item: Vec<&Token> = Vec::new();
    let flush = |item: &mut Vec<&Token>, cols: &mut Vec<Column>| {
        if let Some(first) = item.first() {
            if let Tok::Word { text, quoted } = &first.tok {
                if *quoted || !NOT_A_COLUMN.contains(&text.as_str()) {
                    let line = first.line;
                    let signature = lines
                        .get(line as usize - 1)
                        .map(|l| l.trim().trim_end_matches(',').to_string())
                        .unwrap_or_default();
                    cols.push(Column {
                        name: text.clone(),
                        signature: truncate(&signature),
                        line,
                    });
                }
            }
        }
        item.clear();
    };
    let mut i = open;
    while i < tokens.len() {
        let t = &tokens[i];
        match &t.tok {
            Tok::Punct('(') => {
                depth += 1;
                if depth > 1 {
                    item.push(t);
                }
            }
            Tok::Punct(')') => {
                depth -= 1;
                if depth == 0 {
                    flush(&mut item, &mut cols);
                    break;
                }
                item.push(t);
            }
            Tok::Punct(',') if depth == 1 => flush(&mut item, &mut cols),
            _ => {
                if is_kw(Some(t), "references") {
                    if let Some((parts, _)) = qualified(tokens, i + 1) {
                        uses.push(Use {
                            name: parts.last().cloned().unwrap_or_default(),
                            call: false,
                            line: t.line,
                        });
                    }
                }
                item.push(t);
            }
        }
        i += 1;
    }
    (cols, uses)
}

fn truncate(s: &str) -> String {
    const MAX: usize = 160;
    if s.chars().count() <= MAX {
        s.to_string()
    } else {
        let cut: String = s.chars().take(MAX).collect();
        format!("{cut}…")
    }
}

/// The comment run that ends on the line directly above `line`.
fn doc_above(comments: &[Comment], line: u32) -> Option<(String, u32, u32)> {
    let mut end = line.checked_sub(1)?;
    let mut picked: Vec<&Comment> = Vec::new();
    for c in comments.iter().rev() {
        if c.end == end {
            picked.push(c);
            end = c.start.checked_sub(1).unwrap_or(0);
        } else if c.end < end {
            break;
        }
    }
    if picked.is_empty() {
        return None;
    }
    picked.reverse();
    let text = picked
        .iter()
        .map(|c| c.text.as_str())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        return None;
    }
    Some((text, picked[0].start, picked[picked.len() - 1].end))
}

/// Everything a script declares and uses.
pub fn scan(src: &str) -> (Vec<Decl>, Vec<Use>) {
    // A byte-order mark is not part of the first line a signature quotes.
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let (tokens, comments) = tokenize(src);
    let lines: Vec<&str> = src.lines().collect();
    let mut decls: Vec<Decl> = Vec::new();
    let mut uses: Vec<Use> = Vec::new();
    // Tokens that are a declared name, so `create procedure p(` is not a call.
    let mut declared_at: Vec<usize> = Vec::new();

    let mut i = 0;
    while i < tokens.len() {
        if let Some((kind, mut parts, after)) = create_at(&tokens, i) {
            let line = tokens[i].line;
            let name = parts.pop().unwrap_or_default();
            let schema = (!parts.is_empty()).then(|| parts.join("."));
            declared_at.push(after - 1);
            let mut decl = Decl {
                name,
                schema,
                kind,
                start: line,
                end: line,
                signature: truncate(lines.get(line as usize - 1).map_or("", |l| l.trim())),
                doc: doc_above(&comments, line),
                columns: Vec::new(),
            };
            if kind == "table" && is_punct(tokens.get(after), '(') {
                let (cols, refs) = columns(&tokens, after, &lines);
                decl.columns = cols;
                uses.extend(refs);
            }
            if kind == "trigger" {
                // `CREATE TRIGGER t ON table …` — somewhere in the header.
                if let Some(on) = (after..tokens.len().min(after + 12))
                    .find(|&k| is_kw(tokens.get(k), "on"))
                {
                    if let Some((parts, _)) = qualified(&tokens, on + 1) {
                        uses.push(Use {
                            name: parts.last().cloned().unwrap_or_default(),
                            call: false,
                            line: tokens[on].line,
                        });
                    }
                }
            }
            decls.push(decl);
            i = after;
            continue;
        }
        i += 1;
    }

    // A declaration runs until the next one starts, or to the end of the script.
    let last_line = tokens.last().map_or(1, |t| t.line);
    let starts: Vec<u32> = decls.iter().map(|d| d.start).collect();
    for (k, decl) in decls.iter_mut().enumerate() {
        decl.end = match starts.get(k + 1) {
            Some(&next) if next > decl.start => next - 1,
            _ => last_line.max(decl.start),
        };
    }

    for (k, t) in tokens.iter().enumerate() {
        let Tok::Word { text, quoted } = &t.tok else {
            continue;
        };
        if !*quoted
            && matches!(
                text.as_str(),
                "from" | "join" | "into" | "update" | "exec" | "execute" | "call"
            )
        {
            if let Some((parts, _)) = qualified(&tokens, k + 1) {
                let name = parts.last().cloned().unwrap_or_default();
                if !object_kind(&name).is_some_and(|_| parts.len() == 1) {
                    uses.push(Use {
                        name,
                        call: matches!(text.as_str(), "exec" | "execute" | "call"),
                        line: t.line,
                    });
                }
            }
            continue;
        }
        if is_punct(tokens.get(k + 1), '(')
            && !declared_at.contains(&k)
            && (*quoted || !NOT_A_CALL.contains(&text.as_str()))
            && !is_punct(tokens.get(k.wrapping_sub(1)), '.')
        {
            uses.push(Use {
                name: text.clone(),
                call: true,
                line: t.line,
            });
        }
    }
    uses.sort_by_key(|u| u.line);
    (decls, uses)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = "\
-- Orders placed through the web shop.
-- One row per order.
CREATE TABLE sales.orders (
    id INT PRIMARY KEY,
    customer_id INT REFERENCES customers(id),
    note VARCHAR(200) DEFAULT 'it''s -- not a comment',
    CONSTRAINT ck_total CHECK (id > 0)
);

/* Every order, with its customer. */
create or replace view order_list as
  select o.id, c.name from sales.orders o join customers c on c.id = o.customer_id;

CREATE PROCEDURE archive_orders(@before DATE) AS
BEGIN
  INSERT INTO order_archive SELECT * FROM orders WHERE placed < @before;
  EXEC log_event 'archived';
  SELECT count(*) FROM orders;
END;

create trigger orders_audit on orders for insert as
  insert into audit_log values (1);
";

    #[test]
    fn every_created_object_is_declared_with_its_kind_and_lines() {
        let (decls, _) = scan(SCRIPT);
        let got: Vec<(&str, &str, u32, u32)> = decls
            .iter()
            .map(|d| (d.name.as_str(), d.kind, d.start, d.end))
            .collect();
        assert_eq!(
            got,
            [
                ("orders", "table", 3, 10),
                ("order_list", "view", 11, 13),
                ("archive_orders", "procedure", 14, 20),
                ("orders_audit", "trigger", 21, 22),
            ]
        );
        assert_eq!(decls[0].schema.as_deref(), Some("sales"));
        assert_eq!(decls[3].signature, "create trigger orders_audit on orders for insert as");
    }

    #[test]
    fn a_table_declares_its_columns_and_not_its_constraints() {
        let (decls, _) = scan(SCRIPT);
        let names: Vec<&str> = decls[0].columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["id", "customer_id", "note"]);
        assert_eq!(decls[0].columns[0].signature, "id INT PRIMARY KEY");
    }

    #[test]
    fn the_comment_block_above_a_declaration_is_its_doc() {
        let (decls, _) = scan(SCRIPT);
        assert_eq!(
            decls[0].doc,
            Some((
                "Orders placed through the web shop.\nOne row per order.".to_string(),
                1,
                2
            ))
        );
        assert_eq!(
            decls[1].doc.as_ref().map(|d| d.0.as_str()),
            Some("Every order, with its customer.")
        );
        assert_eq!(decls[2].doc, None);
    }

    #[test]
    fn uses_name_tables_and_calls_but_not_keywords_or_literals() {
        let (_, uses) = scan(SCRIPT);
        let got: Vec<(&str, bool, u32)> = uses
            .iter()
            .map(|u| (u.name.as_str(), u.call, u.line))
            .collect();
        assert!(got.contains(&("customers", false, 5)), "{got:?}");
        assert!(got.contains(&("orders", false, 12)), "{got:?}");
        assert!(got.contains(&("customers", false, 12)), "{got:?}");
        assert!(got.contains(&("order_archive", false, 16)), "{got:?}");
        assert!(got.contains(&("log_event", true, 17)), "{got:?}");
        assert!(got.contains(&("count", true, 18)), "{got:?}");
        assert!(got.contains(&("orders", false, 21)), "{got:?}");
        assert!(got.contains(&("audit_log", false, 22)), "{got:?}");
        // The declared procedure is not a call to itself, `values (` is not a
        // call, and nothing inside the string literal was read.
        assert!(!got.iter().any(|u| u.0 == "archive_orders"), "{got:?}");
        assert!(!got.iter().any(|u| u.0 == "values"), "{got:?}");
        assert!(!got.iter().any(|u| u.0 == "comment"), "{got:?}");
    }

    #[test]
    fn quoted_names_keep_their_spelling_and_unquoted_ones_fold() {
        let (decls, _) = scan("CREATE TABLE [dbo].[OrderLine] (Qty int)\nCREATE VIEW Totals AS SELECT 1");
        assert_eq!(decls[0].name, "OrderLine");
        assert_eq!(decls[0].schema.as_deref(), Some("dbo"));
        assert_eq!(decls[0].columns[0].name, "qty");
        assert_eq!(decls[1].name, "totals");
    }

    #[test]
    fn a_byte_order_mark_does_not_reach_the_signature() {
        let (decls, _) = scan("\u{feff}create procedure p as select 1");
        assert_eq!(decls[0].signature, "create procedure p as select 1");
    }
}
