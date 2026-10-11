# plugin-xaml

A reverse-architect language plugin for XAML, built the way a third party builds one: in its own workspace,
against nothing but the published plugin contract (`ra:plugin@0.3.0`, [`wit-v3/`](../../wit-v3/extractor.wit))
and [`docs/plugins.md`](../../docs/plugins.md). Pure Rust (`quick-xml`), so no C toolchain and no wasi-sdk is
needed. It serves all three passes: `declarations`, `usages` and `bodies`.

What it reads and what the contract could not carry is in [`FINDINGS.md`](FINDINGS.md).

## Build

```sh
rustup target add wasm32-wasip2
cargo test                                      # native tests of the parser and the three passes
cargo build --release --target wasm32-wasip2
```

The component is `target/wasm32-wasip2/release/plugin_xaml.wasm`; a copy is kept as
`plugins/prebuilt/xaml.wasm`, one of the bundled plugins (`src/plugin/bundled.rs`), built with the
`--remap-path-prefix` flags `plugins/build.ps1` sets (which builds it like `plugin-sql`), so it carries no path of the machine that built it.
Dependencies are pinned with `=`: the component's bytes are part of a run's fingerprint.

## Load

```sh
reverse-architect <repo> --plugin target/wasm32-wasip2/release/plugin_xaml.wasm --out <dir>
```

`.xaml` files are then covered by the language `xaml` in `coverage.json`; a file that is not well-formed XML
is refused by every pass with a short reason (`err`) and listed as failed, the rest of the run goes on.

## What it reports

- **Vocabulary:** the namespace `xaml` with the node kinds `xaml:style`, `xaml:template`, `xaml:resource`,
  `xaml:element` and `xaml:view`, each with a card kind of the same name.
- **Declarations:** styles, templates and resources as symbols of those kinds, keyed `sym:xaml:<file>#<key>`. A
  style or template keyed by its target type has the key `type:<CLR name>`, the same whatever `xmlns` prefix
  spells the type. Named elements inside templates are keyed `<container>/<name>` and are members of their
  template (`has_member`). The root of a file with `x:Class` is a `xaml:view` symbol keyed
  `sym:xaml:<file>#x:Class`; the plugin never declares a key of another language. A key that repeats in one file
  gets `~2`, `~3`, ….
- **Blocks and references by name:** every `*.Resources` element is a lookup block (lines and parent block);
  a declaration and a reference carry the innermost block they are written in. `{StaticResource K}` and
  `{DynamicResource K}` are references by name to a `xaml:style` (for `BasedOn`, `Style`, `…Style`) or a
  `xaml:resource`; `BasedOn` on a declared style becomes `extends` once resolved.
- **Imports:** merged resource dictionaries, from a relative `Source` or a `pack://…;component/…` URI, when the
  host confirms the path exists.
- **Usages:** type references from `TargetType`, `DataType`, `{x:Type}`, prefixed element names and
  `{x:Static}`, with the file's `clr-namespace` mappings as usings.
- **Bodies:** one `markup` body per style, template and `x:Class` root (and per keyed resource that holds a
  markup extension it cannot parse, carrying only the `unparsed-markup` tag), paired with its declaration by the
  name position, with a node per element; binding paths, `{x:Static}`, attached properties and setters as
  member accesses; event handlers and `EventSetter`s as subscriptions; attribute texts as literals; and the
  tags `target-type`, `data-source`, `trigger`, `control-key`, `key-gesture` and `unparsed-markup` (a value
  written as a markup extension the plugin could not take apart). The rules, including the
  heuristic that tells an event handler from a plain value, are in the comment at the top of
  [`src/bodies.rs`](src/bodies.rs).
