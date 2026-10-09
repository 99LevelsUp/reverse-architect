//! The six-grammar characterization fixture (U1, R18).
//!
//! Every other golden fixture in this repo parses at most one real source file:
//! `render.rs` / `manifest.rs` build from three files whose `.rs` bodies are a
//! single comment, and the card golden is C#-only. Between them five of the six
//! tree-sitter grammars have no coverage at all, so a per-language regression
//! could land without turning a single snapshot red.
//!
//! This fixture exists to be that instrument. It carries **two** files per
//! grammar variant — `Language` in `src/extract/treesitter.rs` has six: Rust,
//! TypeScript, Tsx, Python, CSharp, Go — a model declaring a type and a
//! function, and a consumer that calls into it. Two files rather than one so
//! every usage edge resolves *within* its own language: a single file per
//! grammar would have to call across languages to exercise usage at all, and a
//! Rust edge pointing at a Go declaration makes per-grammar attribution
//! ambiguous, which is the one thing this fixture is for.
//!
//! Names are language-prefixed for the same reason. Heuristic name resolution
//! is repo-wide, so a shared name like `compute_total` would collide across six
//! files and every snapshot would carry ambiguity that says nothing about the
//! grammar under it.
//!
//! `Cargo.toml`, `go.mod` and `package.json` bring `manifest_deps` and the
//! language tables into the run. Note what that does *not* buy: everything
//! those three produce anchors to their own paths, so none of it lands in a
//! per-grammar slice. `u1_manifests` is the snapshot that attributes it.
//!
//! # Two shapes here are load-bearing and easy to "tidy" into uselessness
//!
//! **Every doc comment carries diacritics and a line break.** `DocComment` is
//! the one fact holding verbatim prose that R10 says is never translated,
//! never reflowed and never stripped of diacritics — and U11 moves the code
//! producing it across a WASM boundary, where string marshaling is the
//! likeliest defect class there is. An all-ASCII fixture cannot see that
//! corruption. Do not "simplify" these to plain English.
//!
//! **The Rust pair lives under `rust/src/` and the Python consumer imports
//! `from .model`.** Both are what make internal import resolution actually
//! fire. `crate_root_dir` (`src/extract/treesitter.rs:3101-3109`) locates a
//! crate by finding `src` among the path components and returns `None`
//! otherwise, failing closed; `resolve_python_from` (`:2947-2950`) treats any
//! non-relative import as external. Flatten either and the snapshots silently
//! re-pin "this language emits no `Imports` edge" as the correct answer.

use git2::Repository;
use std::path::Path;

/// The six grammar variants, each with the path prefix its files live under.
/// The prefix is what slices the graph per grammar, so a whole-corpus diff
/// still points at one grammar (U11's execution note depends on this).
pub const GRAMMARS: &[(&str, &str)] = &[
    ("rust", "rust/"),
    ("typescript", "ts/"),
    ("tsx", "tsx/"),
    ("python", "py/"),
    ("csharp", "cs/"),
    ("go", "go/"),
    ("javascript", "js/"),
    ("java", "java/"),
];

/// The fixture files that carry no grammar. `manifest_deps` reads all three;
/// coverage reports each as unclassified, which is the artifact working rather
/// than a gap. Declared here so a test's exception list cannot drift from the
/// fixture's real contents.
pub const MANIFESTS: &[&str] = &["Cargo.toml", "go.mod", "package.json"];

const RUST_MODEL: &str = r#"/// Faktura s částkou v haléřích.
/// Záporné hodnoty jsou neplatné.
pub struct RustInvoice {
    pub amount: i64,
}

impl RustInvoice {
    pub fn new(amount: i64) -> RustInvoice {
        RustInvoice { amount }
    }
}

/// Spočítá celkovou částku faktury.
/// Nikdy nevrací zápornou hodnotu.
pub fn rust_compute_total(invoice: &RustInvoice) -> i64 {
    invoice.amount
}
"#;

const RUST_CONSUMER: &str = r#"use crate::model::{rust_compute_total, RustInvoice};

/// Výkaz shrnující jednu fakturu.
/// Drží už jen výslednou částku.
pub struct RustReport {
    pub total: i64,
}

/// Sestaví výkaz z částky.
/// Použije model ze sousedního souboru.
pub fn rust_build_report(amount: i64) -> RustReport {
    let invoice = RustInvoice::new(amount);
    RustReport {
        total: rust_compute_total(&invoice),
    }
}
"#;

const TS_MODEL: &str = r#"/**
 * Faktura s částkou.
 * Záporné hodnoty jsou neplatné.
 */
export class TsInvoice {
  amount: number;

  constructor(amount: number) {
    this.amount = amount;
  }
}

/**
 * Spočítá celkovou částku.
 * Nikdy nevrací zápornou hodnotu.
 */
export function tsComputeTotal(invoice: TsInvoice): number {
  return invoice.amount;
}
"#;

const TS_CONSUMER: &str = r#"import { TsInvoice, tsComputeTotal } from "./model";

/**
 * Výkaz shrnující jednu fakturu.
 * Drží už jen výslednou částku.
 */
export class TsReport {
  total: number;

  constructor(total: number) {
    this.total = total;
  }
}

/**
 * Sestaví výkaz z částky.
 * Použije model ze sousedního souboru.
 */
export function tsBuildReport(amount: number): TsReport {
  const invoice = new TsInvoice(amount);
  return new TsReport(tsComputeTotal(invoice));
}
"#;

// JSX in both files on purpose: `.tsx` is parsed with LANGUAGE_TSX rather than
// LANGUAGE_TYPESCRIPT, and a body that is also valid `.ts` would not tell the
// two grammars apart.
const TSX_MODEL: &str = r#"/**
 * Faktura vykreslovaná do stránky.
 * Částka se zobrazuje v haléřích.
 */
export class TsxInvoice {
  amount: number;

  constructor(amount: number) {
    this.amount = amount;
  }
}

/**
 * Vykreslí částku faktury.
 * Vrací jediný prvek span.
 */
export function tsxRenderAmount(invoice: TsxInvoice) {
  return <span className="amount">{invoice.amount}</span>;
}
"#;

const TSX_CONSUMER: &str = r#"import { TsxInvoice, tsxRenderAmount } from "./model";

/**
 * Výkaz vykreslovaný do stránky.
 * Drží už jen výslednou částku.
 */
export class TsxReport {
  amount: number;

  constructor(amount: number) {
    this.amount = amount;
  }
}

/**
 * Sestaví výkaz z částky.
 * Použije model ze sousedního souboru.
 */
export function tsxBuildReport(amount: number) {
  const invoice = new TsxInvoice(amount);
  return <div className="report">{tsxRenderAmount(invoice)}</div>;
}
"#;

const PY_MODEL: &str = r#"class PyInvoice:
    """Faktura s částkou.
    Záporné hodnoty jsou neplatné."""

    def __init__(self, amount):
        self.amount = amount


def py_compute_total(invoice):
    """Spočítá celkovou částku.
    Nikdy nevrací zápornou hodnotu."""
    return invoice.amount
"#;

const PY_CONSUMER: &str = r#"from .model import PyInvoice, py_compute_total


class PyReport:
    """Výkaz shrnující jednu fakturu.
    Drží už jen výslednou částku."""

    def __init__(self, total):
        self.total = total


def py_build_report(amount):
    """Sestaví výkaz z částky.
    Použije model ze sousedního souboru."""
    invoice = PyInvoice(amount)
    return PyReport(py_compute_total(invoice))
"#;

const CS_MODEL: &str = r#"namespace Fixture.Billing
{
    /// Faktura s částkou.
    /// Záporné hodnoty jsou neplatné.
    public class CsInvoice
    {
        public int Amount { get; set; }

        public CsInvoice(int amount)
        {
            Amount = amount;
        }
    }

    public class CsCalculator
    {
        /// Spočítá celkovou částku.
        /// Nikdy nevrací zápornou hodnotu.
        public int CsComputeTotal(CsInvoice invoice)
        {
            return invoice.Amount;
        }
    }
}
"#;

const CS_CONSUMER: &str = r#"namespace Fixture.Billing
{
    /// Výkaz shrnující jednu fakturu.
    /// Drží už jen výslednou částku.
    public class CsReport
    {
        public int Total { get; set; }

        /// Sestaví výkaz z částky.
        /// Použije model ze sousedního souboru.
        public CsReport BuildReport(int amount)
        {
            var invoice = new CsInvoice(amount);
            var calculator = new CsCalculator();
            return new CsReport { Total = calculator.CsComputeTotal(invoice) };
        }
    }
}
"#;

const GO_MODEL: &str = r#"package fixture

// GoInvoice je faktura s částkou.
// Záporné hodnoty jsou neplatné.
type GoInvoice struct {
	Amount int64
}

// GoComputeTotal spočítá celkovou částku.
// Nikdy nevrací zápornou hodnotu.
func GoComputeTotal(invoice GoInvoice) int64 {
	return invoice.Amount
}
"#;

const GO_CONSUMER: &str = r#"package fixture

// GoReport je výkaz shrnující jednu fakturu.
// Drží už jen výslednou částku.
type GoReport struct {
	Total int64
}

// GoBuildReport sestaví výkaz z částky.
// Použije model ze sousedního souboru.
func GoBuildReport(amount int64) GoReport {
	invoice := GoInvoice{Amount: amount}
	return GoReport{Total: GoComputeTotal(invoice)}
}
"#;

// JavaScript, added with the grammar (29.09.2026). Both module systems on
// purpose: the consumer `require`s the model (CommonJS) and the model's
// function is an exported arrow — the form most JavaScript is written in and
// the one a `function`-only walker never saw.
const JS_MODEL: &str = r#"/**
 * Faktura s částkou.
 * Záporné hodnoty jsou neplatné.
 */
class JsInvoice {
  amount = 0;

  constructor(amount) {
    this.amount = amount;
  }
}

/**
 * Spočítá celkovou částku.
 * Nikdy nevrací zápornou hodnotu.
 */
const jsComputeTotal = (invoice) => invoice.amount;

module.exports = { JsInvoice, jsComputeTotal };
"#;

const JS_CONSUMER: &str = r#"const { JsInvoice, jsComputeTotal } = require("./model");

/**
 * Výkaz shrnující jednu fakturu.
 * Drží už jen výslednou částku.
 */
class JsReport extends JsInvoice {
  constructor(total) {
    super(total);
  }
}

/**
 * Sestaví výkaz z částky.
 * Použije model ze sousedního souboru.
 */
function jsBuildReport(amount) {
  const invoice = new JsInvoice(amount);
  return new JsReport(jsComputeTotal(invoice));
}
"#;

// Java, added with the grammar (29.09.2026): a package as the module path, a
// class hierarchy across two files, Javadoc with diacritics, and the three
// usage shapes — a method invocation, a construction, a type in a signature.
const JAVA_MODEL: &str = r#"package demo;

/**
 * Faktura s částkou.
 * Záporné hodnoty jsou neplatné.
 */
public class JavaInvoice {
    private final long amount;

    public JavaInvoice(long amount) {
        this.amount = amount;
    }

    /**
     * Spočítá celkovou částku.
     * Nikdy nevrací zápornou hodnotu.
     */
    public long total() {
        return amount;
    }
}
"#;

const JAVA_CONSUMER: &str = r#"package demo;

/**
 * Výkaz shrnující jednu fakturu.
 * Drží už jen výslednou částku.
 */
public class JavaReport extends JavaInvoice {
    public JavaReport(long total) {
        super(total);
    }

    /**
     * Sestaví výkaz z částky.
     * Použije model ze sousedního souboru.
     */
    public static JavaReport build(long amount) {
        JavaInvoice invoice = new JavaInvoice(amount);
        return new JavaReport(invoice.total());
    }
}
"#;

const CARGO_TOML: &str = r#"[package]
name = "six-grammar-fixture"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = "1.0"
"#;

const GO_MOD: &str = r#"module example.test/fixture

go 1.22

require github.com/google/uuid v1.6.0
"#;

const PACKAGE_JSON: &str = r#"{
  "name": "six-grammar-fixture",
  "version": "1.0.0",
  "dependencies": {
    "left-pad": "1.3.0"
  }
}
"#;

/// Every file in the fixture, in the order they are committed.
///
/// The Rust pair sits under `rust/src/` rather than directly under `rust/` so
/// `crate::model` resolves — see the module doc. The `rust/` prefix in
/// [`GRAMMARS`] still slices both.
pub fn files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("rust/src/model.rs", RUST_MODEL),
        ("rust/src/consumer.rs", RUST_CONSUMER),
        ("ts/model.ts", TS_MODEL),
        ("ts/consumer.ts", TS_CONSUMER),
        ("tsx/model.tsx", TSX_MODEL),
        ("tsx/consumer.tsx", TSX_CONSUMER),
        ("py/model.py", PY_MODEL),
        ("py/consumer.py", PY_CONSUMER),
        ("cs/Model.cs", CS_MODEL),
        ("cs/Consumer.cs", CS_CONSUMER),
        ("go/model.go", GO_MODEL),
        ("go/consumer.go", GO_CONSUMER),
        ("js/model.js", JS_MODEL),
        ("js/consumer.js", JS_CONSUMER),
        ("java/demo/JavaInvoice.java", JAVA_MODEL),
        ("java/demo/JavaReport.java", JAVA_CONSUMER),
        ("Cargo.toml", CARGO_TOML),
        ("go.mod", GO_MOD),
        ("package.json", PACKAGE_JSON),
    ]
}

/// Init a git repo at `dir` holding the whole fixture in one commit, with the
/// fixed signature and time every fixture in this repo uses.
pub fn init(dir: &Path) -> Repository {
    super::init_repo_with_files(dir, "Alice", &files())
}
