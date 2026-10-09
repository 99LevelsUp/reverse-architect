# The wasm toolchain

What you need installed to build a language extractor as a WASM component, and
how to prove your setup works before you rely on it.

Everything here was reproduced on 04.09.2026; the versions in the table at the
bottom are the ones that actually ran, not the ones that ought to.

This note is about making a component **build and parse**. What the boundary
**costs** — in-guest parse throughput and the price of one pull query — is
measured separately, with the setup below.

## Why any of this is pinned

The project's product claim is byte-identical output. Once extraction moves
behind a component boundary (U11), the component's content hash becomes a
run-fingerprint input under R13 — so the compiler that produced it is part of
the reproducibility story, not an implementation detail. `rust-toolchain.toml`
therefore pins an exact Rust version rather than `stable`.

Two decisions worth knowing before you fight them:

- **No `cargo-component`.** Plain `cargo build --target wasm32-wasip2` plus
  `wit-bindgen` produces a genuine component, and `cargo-component` is being
  deprecated upstream (KTD18).
- **WASI p2, not p3.** p3 exists in wasmtime 48, but it would force an async
  runtime into a host whose extraction walks are synchronous rayon (KTD19).

## What rustup does for you

Nothing to run. `rust-toolchain.toml` at the repo root pins Rust 1.95.0, adds
the `wasm32-wasip2` target, and installs `clippy` and `rustfmt`; rustup applies
it the first time you run `cargo` in this checkout.

## What you install by hand

**wasi-sdk 34.0** — required because every tree-sitter grammar ships C that has
to be cross-compiled. Without it `cargo build --target wasm32-wasip2` fails at
`cc-rs: failed to find tool "clang"`, which reads like a missing PATH entry and
is not.

Take the archive for your platform from the `wasi-sdk-34` release of
`WebAssembly/wasi-sdk` on GitHub (the Windows asset unpacks to a directory named
`wasi-sdk-34.0-x86_64-windows`) and extract it anywhere. This note calls that
directory `<sdk>`.

**wasmtime 48.0.1** — only to *run* a component from the command line while
developing. The host embeds wasmtime as a library from U7 onward; today it is
not a Cargo dependency of this repo at all, so the version below is a recorded
decision rather than something you can read out of `Cargo.toml`.

## Environment

The C cross-compilation variables are not optional and not discoverable:

| Variable | Value |
|---|---|
| `WASI_SDK_PATH` | `<sdk>` |
| `CC_wasm32_wasip2` | `<sdk>/bin/clang` (`clang.exe` on Windows) |
| `AR_wasm32_wasip2` | `<sdk>/bin/llvm-ar` (`llvm-ar.exe` on Windows) |
| `CFLAGS_wasm32_wasip2` | `--sysroot=<sdk>/share/wasi-sysroot` |

The `--sysroot` flag is the one people leave out. Without it clang compiles
against the host's headers and the link fails with errors that point at
tree-sitter rather than at the missing sysroot.

## Proving your setup: the R19 probe

R19 is the requirement that a tree-sitter grammar plus the tree-sitter runtime
can be built as a `wasm32-wasip2` component and actually parse there. It was
satisfied on 03.09.2026, but a proof you cannot re-run is not a proof you can
trust — so the probe is reproduced here in full.

It is deliberately end-to-end. That the crate *links* proves nothing; the parse
has to produce the same shape it produces natively, or the gate has said
nothing about whether extraction can cross the boundary.

Create a scratch directory outside this repository with these three files.

`rust-toolchain.toml` — **not optional, and the reason is easy to miss.** The
repo's pin applies only inside the repo, and the probe deliberately lives
outside it. Without this copy the probe builds on whatever your default
toolchain happens to be, which may be a different compiler and may not have
`wasm32-wasip2` installed at all — so the probe would fail, or pass, for
reasons unrelated to the setup you are trying to check:

```toml
[toolchain]
channel = "1.95.0"
targets = ["wasm32-wasip2"]
```

`Cargo.toml`:

```toml
[package]
name = "r19-wasm-gate"
version = "0.0.0"
edition = "2021"

# Exact pins, matching this repo's Cargo.lock. A caret requirement here defeats
# the probe's whole purpose — see "A trap this note exists to prevent" below.
[dependencies]
tree-sitter = "=0.26.10"
tree-sitter-rust = "=0.24.2"

[workspace]
```

`src/main.rs`:

```rust
fn main() {
    let source = r#"
pub struct Cfg { pub n: u8 }

impl Cfg {
    pub fn build(n: u8) -> Cfg {
        Cfg { n }
    }
}

pub fn main_entry() -> u8 {
    let c = Cfg::build(7);
    c.n
}
"#;

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .expect("loading the Rust grammar must succeed");

    let tree = parser.parse(source, None).expect("parse must produce a tree");
    let root = tree.root_node();

    // Walk the whole tree so the probe exercises more than construction.
    let mut named = 0usize;
    let mut cursor = root.walk();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.is_named() {
            named += 1;
        }
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    println!("root_kind={}", root.kind());
    println!("has_error={}", root.has_error());
    println!("named_nodes={named}");
    println!("sexp_head={}", &root.to_sexp()[..60.min(root.to_sexp().len())]);
}
```

Build it, run it under wasmtime, and run it natively for comparison:

```
cargo build --target wasm32-wasip2
wasmtime target/wasm32-wasip2/debug/r19-wasm-gate.wasm
cargo run --quiet
```

Both runs must print exactly this — the guest and the host agreeing is the
whole point:

```
root_kind=source_file
has_error=false
named_nodes=43
sexp_head=(source_file (struct_item (visibility_modifier) name: (type_
```

### Check you built a component, not a core module

`cargo build` succeeding does not tell the two apart, and the difference
matters: only a component can be loaded through the interface this project is
built around. It is visible in the first eight bytes.

| Preamble | Meaning |
|---|---|
| `00 61 73 6d 0d 00 01 00` | component — what you want |
| `00 61 73 6d 01 00 00 00` | core module — wrong artifact |

PowerShell:

```powershell
[System.IO.File]::ReadAllBytes('target/wasm32-wasip2/debug/r19-wasm-gate.wasm')[0..7] |
  ForEach-Object { $_.ToString('x2') }
```

Unix shells: `head -c8 target/wasm32-wasip2/debug/r19-wasm-gate.wasm | xxd`.

## A trap this note exists to prevent

Re-running the probe on 04.09.2026 built `tree-sitter 0.26.13`, not the
`0.26.10` this repo's `Cargo.lock` holds. The probe's manifest said
`tree-sitter = "0.26.10"` — a caret requirement, which cargo is free to resolve
to any later patch, and its own lockfile had drifted there.

The output was unaffected, so R19's conclusion still stands. But the probe had
stopped proving what it claimed: that *this repo's* dependency set builds and
parses on the target. That is why the manifest above uses `=` pins, and it is
worth checking whenever a proof outlives the day it was run.

## Recorded versions

| Component | Version | How it is fixed |
|---|---|---|
| Rust | 1.95.0 | `rust-toolchain.toml` |
| Target | `wasm32-wasip2` | `rust-toolchain.toml` |
| wasi-sdk | 34.0 (clang 23.1.0-rc3) | this note; installed by hand |
| wasmtime | 48.0.1 (`7bac2c277`, 2026-08-24) | this note; becomes a Cargo dependency in U7, and a fingerprint input under R13 |
| tree-sitter | 0.26.10 | `Cargo.lock` |
| tree-sitter-rust | 0.24.2 | `Cargo.lock` |
