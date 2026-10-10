# plugin-xaml

A reverse-architect language plugin for XAML, built the way a third party builds one: in its own workspace,
against nothing but the published plugin contract and [`docs/plugins.md`](../../docs/plugins.md). Pure Rust
(`quick-xml`), so no C toolchain and no wasi-sdk is needed.

What it reads and what the contract could not carry is in [`FINDINGS.md`](FINDINGS.md).

## Build

```sh
rustup target add wasm32-wasip2
cargo test                                      # native tests of the parser and both passes
cargo build --release --target wasm32-wasip2
```

The component is `target/wasm32-wasip2/release/plugin_xaml.wasm`. Dependencies are pinned with `=`: the
component's bytes are part of a run's fingerprint.

## Load

```sh
reverse-architect <repo> --plugin target/wasm32-wasip2/release/plugin_xaml.wasm --out <dir>
```

`.xaml` files are then covered by the language `xaml` in `coverage.json`; a file that is not well-formed XML
fails on its own and is listed as failed, the rest of the run goes on.

## What it reports

- **Declarations:** styles, templates and resources as symbols, keyed `sym:xaml:<file>#<key>`. A style or
  template keyed by its target type has the key `type:<CLR name>`, the same whatever `xmlns` prefix spells the
  type. Named elements inside templates are keyed `<container>/<name>`. The root of a file with `x:Class` is a
  `view` symbol keyed `sym:xaml:<file>#x:Class`; the plugin never declares a key of another language. A key that
  repeats in one file gets `~2`, `~3`, ….
- **Imports:** merged resource dictionaries, from a relative `Source` or a `pack://…;component/…` URI, when the
  host confirms the path exists.
- **Usages:** type references from `TargetType`, `DataType`, `{x:Type}`, prefixed element names and
  `{x:Static}`, with the file's `clr-namespace` mappings as usings.
