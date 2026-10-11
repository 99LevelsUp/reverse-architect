# Writing a language plugin

A plugin teaches reverse-architect a language. It is a WebAssembly component (`wasm32-wasip2`) that implements the `language-plugin` world of the plugin contract. The bundled languages are plugins of exactly this shape, and the host treats yours the same way.

The shortest complete example is [`examples/plugin-sql`](../examples/plugin-sql). It is pure Rust, builds without a C toolchain, and ships as the bundled SQL plugin. [`examples/plugin-xaml`](../examples/plugin-xaml) is the second one, on contract 0.3, and ships as the bundled XAML plugin: both are built outside the plugin workspace against the published WIT alone, the way a third party builds one.

## Contract versions

| Version | File | State |
|---|---|---|
| `ra:plugin@0.3.0` | [`wit-v3/extractor.wit`](../wit-v3/extractor.wit) | current: build new plugins against it |
| `ra:plugin@0.2.0` | [`wit-v2/extractor.wit`](../wit-v2/extractor.wit) | frozen, still loaded |
| `ra:plugin@0.1.0` | [`wit/extractor.wit`](../wit/extractor.wit) | frozen, still loaded |

0.2 keeps everything 0.1 says about what a plugin is, what it is given and what the host does with its output. It adds what the syntax says around each use of a name, so the host's rules can sift the candidates (see [Pass 2](#pass-2-usages)), and where a declaration's name is written. Extending a record is an incompatible type change in the component model, so this is a new version rather than an edit of 0.1.

0.3 keeps everything 0.2 says and adds a third pass over a file, `bodies`, and what a declaration could not say before (see [What 0.3 adds](#what-03-adds)). It is a new version for the same reason: extending a record is an incompatible type change.

**A 0.1 or 0.2 plugin keeps working unchanged.** The host offers every contract's imports to every component and then looks at what it exports: it tries `ra:plugin/extractor` of 0.3 first, then 0.2, then 0.1, and runs the component under the first it finds. New plugins should target 0.3; 0.2 and 0.1 plugins need no change. A component that exports none of them ends the run with a version mismatch that names the versions the host wanted and what the component exports. A 0.1 plugin's output is lifted to the 0.2 shape at the boundary, with no column and no context, which is all 0.1 could say. Its usages are then resolved as before the rules existed: by name only. `examples/plugin-sql` is a 0.1 plugin and the bundled SQL plugin, so every run exercises this path. `coverage.json` reports, per language, the contract its plugin ran under, and counts the plugins per contract version.

**Only C# supplies context today.** The other bundled languages except SQL, which stays on 0.1, and XAML, which runs under 0.3, run under 0.2 but leave `context` empty, so the rules do not run on their usages.

## The contract

The world exports five functions and imports one (`bodies` exists from 0.3 on):

| | Called | Returns |
|---|---|---|
| `describe()` | once, at load | a manifest: id, version, languages and their extensions, trust rank (0.3: the passes served and the record kinds emitted) |
| `declare()` | once, before any file | the plugin's own fact and edge kinds, or none |
| `declarations(path, source, config)` | once per file, pass 1 | everything the file declares |
| `usages(path, source)` | once per file, pass 2 | every name the file uses (0.2: with the file's scopes) |
| `bodies(path, source)` | once per file, pass 3 (0.3) | the bodies of the file's members, as trees |
| `path-exists(key)` *(host import)* | any time | whether a repository path exists, so an import can be resolved |

The host hands over one file's bytes at a time. A plugin never sees the repository, the graph, the disk or the network.

### The manifest

- **`id`** uses lowercase letters, digits and `-`. It names your facts in every provenance record, so keep it stable. A declared plugin whose id matches a bundled one (`csharp`, `ecmascript`, `go`, `java`, `python`, `rust`, `sql`, `xaml`) replaces it.
- **`languages`** lists a reporting name and extensions without the dot: `{ name: "sql", extensions: ["sql"] }`. The name appears in `coverage.json`.
- **`trust-rank`** decides which value wins when two extractors disagree on a single-valued fact. The bundled plugins use 30.

### Pass 1: declarations

Return a list of `op`s. Most of them are `symbol` groups: one declaration, its facts and the edges it owns.

**Keys.** A symbol key is `sym:<lang>:<module>#<Type>`, and a member is `sym:<lang>:<module>#<Type>.<member>`. Callables append their normalized parameter list, so overloads stay distinct. `<module>` is the language's own notion of a module (namespace, package, crate path, schema), not the file path. If one file yields the same key twice, suffix the second with `~2`. A file's own key is its repository path.

**Facts on a symbol**, all core kinds:

| Kind | Value | |
|---|---|---|
| `symbol_name` | the simple name | required; its `place` must be the declaration's **line range**, which the host uses to find the declaration around a usage |
| `symbol_kind` | see below | required |
| `symbol_signature` | the declaration's header line | recommended |
| `doc_comment` | the comment above it, with its own lines as `place` | recommended |
| `symbol_visibility` | `public`, `export`, … | optional |
| `symbol_container` | the enclosing type's name | for members |
| `entry_point` | `flag(true)` | for `main` and similar |

**`place` in 0.2** is a record: `span` (the lines, as 0.1's `place` was) and `name`, the line and column where the declaration's identifier itself is written, not where an attribute or a modifier before it starts. Set `name` on the symbol's facts (the host takes the first fact of the group that has one) and leave it `none` on anything that is not about a declaration. It is what tells two declarations on one line apart, both when the host looks for the declaration around a usage and when it pairs declarations with a SCIP index. Without it, such declarations are left unpaired rather than guessed.

**`symbol_kind` decides what a usage can resolve to**:

- `function`, `method`, `constructor` and `procedure` are *called*;
- `class`, `struct`, `interface`, `trait`, `enum`, `record`, `type`, `table`, `view` and `trigger` are *referenced*, and each gets a card of its own;
- any other kind (`field`, `property`, `const`, …) is shown on its type's card, and nothing resolves to it.

**Edges.** Include a `declares` edge from the file (source = the file's path) to every symbol. The host never infers it: without it, the file is reported as declaring nothing. Use `has_member` from a type to each member, as an `op::edge` placed after both symbols exist, because an edge to a missing node is dropped. Use `imports` from the file to another file's path, resolved with `path-exists`.

**Relations.** For `extends` or `implements`, return a `relation` with candidate keys for both ends, most specific first. 0.2 adds the kind `by-target-kind` for syntax that does not say which (a C# base list): the host implements an interface and extends anything else. The host resolves relations after every file has been read, so a base type in another file is found.

### Pass 2: usages

Every identifier the file uses is a usage with a `name`, a `kind` and a `line`:

- `call` resolves against callables;
- `construct` and `reference` resolve against types;
- `call-or-construct` tries callables first, then types.

`name` is the simple name: reduce a qualified spelling to its last segment.

Under 0.1, `usages` returns that list. Under 0.2 it returns one `file-usages` record per file, so a single parse serves both parts:

- **`scopes`**: the file's lexical scopes that can carry imports. Index 0 is the file itself. Every namespace declared in it follows, each with its qualified `namespace` (`A.B.C` for `namespace C` inside `namespace A.B`), the `lines` it spans, the index of its `parent`, and its own `usings` in source order: `target`, an `alias` for `using X = Y;`, `is-static` for `using static`, `is-global` for `global using`. A directive is in force in its scope and every scope nested inside. The list may be empty when no usage carries context.
- **`usages`**, in tree order. Each one may add:
  - `column`: the 1-based column of the used name, counted in characters. With a column, `line` is the line of the name itself, so `sb.Append(a).Append(b)` is two usages and a call in a chain that spans lines sits on the line it is written on.
  - `context`, what the syntax says about this one usage, read from this one file without type inference:
    - `scope`: the index of the innermost scope around it;
    - `receiver`, for a call: `none` for a bare `foo()` (said explicitly, because it is what lets the host look on the caller's own type), `value` for a name or dotted path whose first name the file declares as a value (a local, a parameter, a field or property, a pattern or catch variable — whether or not the declaration states its type), `name` for any other name or path (`this`, `base`, a type, a namespace-qualified path, a member declared in another file), `keyword` for a built-in type spelled as its keyword (`string` in `string.IsNullOrEmpty(x)`), or `expression` for anything else. Say `value` whenever the file declares the name: the host may read a `name` as a type, so `var Worker = Load(); Worker.Run()` sent as `name` would let it take `Worker.Run` for a call on a type `Worker`;
    - `declared-type`: the type the receiver is declared with in this file (a parameter, a field, a local with an explicit type or `= new T(…)`), with a `keyword` flag for built-in types. Leave it `none` when the declaration states no type (`var x = Load()`, a target-typed `new()`) or the file does not declare the receiver;
    - `argument-count` at a call or a construction, named arguments included;
    - `qualifier`: for a construction or a type reference written qualified, what precedes the name (`System.Text` in `new System.Text.StringBuilder()`).

  A usage with `context: none` is treated as 0.1 treated it.

The host does the rest, the same way for every plugin. It takes the repository's declarations of that name whose kind fits the usage as candidates, and a fixed catalog of rules sifts them using the context: the declared or static receiver type, a bare call on the caller's own type, the argument count, the namespaces visible in the scope, whether a nested type's simple name reaches the site, and the role the usage needs. A rule backs a candidate, disputes it, or says nothing. Without context no rule runs. Then:

- The namespaces visible at a site are those its own scopes import, plus what its C# project imports into every file: the `global using` directives of every file of its project (the directory of the nearest `.csproj` above it; a file under no project sees those of the whole repository), that project file's `<Using Include="…"/>` items, a `Directory.Build.props` above it, and the SDK's implicit usings where `ImplicitUsings` is enabled. The host reads these from the tracked files itself, so `is-global` on a directive needs no more than to be true. Where a project file holds a `<Using>` item the host cannot read (an MSBuild property, a wildcard), the namespace rule says nothing for that project's files.

- A type nested in another (`Catalog.List`) is reachable by its simple name only from inside its declaring type, a type nested in it or a type deriving from it, or where a `using static` of the declaring type or an alias of the name is in force: in the site's own scopes, or project-wide (`global using static`, `<Using … Static="true"/>`, an alias in either). Anywhere else a type position, a construction or a static receiver spelling that simple name is disputed: `List<int>` in a foreign file is the BCL's list. A site whose enclosing declaration the host could not tell says nothing.

- A call on a `keyword` receiver (`string.IsNullOrEmpty(x)`) becomes no edge: it cannot reach a declaration in the repository. A receiver whose declared type is a keyword has no repository type, so the rule on declared types disputes every candidate except an extension method whose `this` type the repository does not declare. About that one it says nothing, and it never backs it on any receiver: an instance member of the external type, which the host cannot see, takes precedence in C# (`s.IndexOfAny(chars, 1)` on a `string` is `string.IndexOfAny`).
- Every candidate gets its own edge and its own rating:
  - `rules_backed` when it is the only candidate of the site that a rule backed and none disputed. Two backed candidates (`Run(int)` and `Run(string)` on the declared type) are both `ambiguous` until another rule tells them apart, and their edges name the shared name, not the rule that backed them.
  - `ambiguous` when a rule disputed it, or when nothing decided about it and the site has other candidates. The card lists these under "Maybe also" with the reason.
  - `name_only` when nothing decided about it and it is the site's only candidate. It stays in the card's main lists with a `name only` label, because the real target may lie outside the repository.
- More candidates than the ambiguity ceiling (eight by default, `--ambiguity-ceiling`) become a single "ambiguous reference" fact and no edges.
- A name with no candidates is dropped. Reporting a built-in (`count(`, `print(`) therefore costs nothing.

### Your data decides the rules' verdicts

The rules decide, but they decide over what your plugin reports. A wrong declared type makes a rule back the wrong candidate and dispute the right one, and the card shows the result as a backed relation. Report context only where the syntax states it, and leave a field `none` rather than guess.

A rule may remove a candidate outright, not just dispute it, only with a right to delete earned by measurement against a SCIP index. A right is tied to the language, the plugin component's id and SHA-256 and the contract version that were measured (see [docs/measurements/README.md](measurements/README.md)). A third-party plugin, or a rebuilt bundled one, therefore never inherits a right; its data only moves candidates between the main lists and "Maybe also". There is no list of trusted plugins beyond that binding yet. That was considered and not built: no right has been granted so far, and the binding to the measured component's hash already keeps an unmeasured plugin from deleting anything.

### Kinds of your own

A kind that is not a core kind is spelled `<namespace>:<name>`. Declare it in `declare()` under a namespace equal to your plugin id. In `declare()` every name is bare (a node kind `style`, its card kind `style`, the namespace `xaml` beside them; a name holding `:` is refused and the plugin does not load), while a symbol's `node-kind` and a reference's `target-kind` spell the same kind with its namespace (`xaml:style`). A plugin cannot declare into another plugin's namespace. A kind you emit without declaring it is dropped at the boundary, with a message. The host declares a plugin's vocabulary only when the corpus holds a file one of its extensions claims, so a corpus without `.xaml` files carries nothing of the bundled XAML plugin's kinds.

## What 0.3 adds

**Which passes a component serves.** The manifest has two new fields. `passes` lists the passes the component serves (`declarations`, `usages`, `bodies`) for every language it lists; a pass it does not list is never called. `emits` lists the record kinds and tag names the component can report in `bodies` (`subscription`, `key-compare`, `menu-item`…). It says what the component reads, not what the host makes of it, and is empty for a component that does not serve `bodies`. A 0.1 or 0.2 plugin serves exactly `declarations` and `usages` and emits nothing.

The host routes each pair of extension and pass to the first plugin, in precedence order (the `--plugin` arguments, then the bundled ones), that claims the extension and serves the pass. A component that serves only `bodies` therefore never takes a file's declarations or usages from another plugin, and another plugin can read the declarations and usages of the same files.

**Every pass returns a `result`.** `declarations`, `usages` and `bodies` return `result<…, string>`. Return `err` with a short reason when the file cannot be read: it is not well formed, or it is in an encoding the component does not read. The host records the file as failed for that pass, with the reason redacted, in `coverage.json`, and goes on with the same instance. A trap still fails the file, but says nothing about why.

**`declarations` returns `file-declarations`.** Under 0.3 the result is a record, not a list:

- `ops`: what 0.2 returned, with the same meaning;
- `name-refs` and `blocks`: references by name and the nested blocks they are looked up in (see [References by name](#references-by-name)).

**A symbol has two new fields.**

- `node-kind` is a node kind from the component's own vocabulary, `<namespace>:<name>` (`xaml:style`). The symbol's node is then of that kind and gets the card that kind's policy names (the `card` of its `node-kind-decl` in what `declare()` returns). A kind the vocabulary does not declare, or one under another plugin's namespace, drops the whole symbol group with a message, as an undeclared fact kind drops its fact. `none` is an ordinary symbol, whose kind comes from the `symbol_kind` fact as before.
- `block` is the index into `file-declarations.blocks` of the innermost block the declaration is written in, or `none` when the file has no such blocks.

## Pass 3: bodies

`bodies(path, source)` returns, for one file, the syntax tree of the bodies of its members, with every node anchored to its lines. It feeds the second, optional layer of a run, enabled by `--detail-level`, which sits beside the graph. Without it the host does not call the pass, and the run is what it was under 0.2. Nothing in this pass becomes a fact of the graph. Only a 0.3 component that lists `bodies` in `passes` is asked for it, and it is not given `config`. With `--detail-level`, the host pairs the model with the first layer's declarations, analyzes it and shows it on the cards of C# types.

**Pairing with declarations.** A body is paired with a declaration of the first layer by the position of the declared name. `member-body.name-position` must be exactly the position `declarations` puts in `place.name` for that member. A body with no declaration of its own (an accessor, a local function) carries the position of the member it belongs to or is declared in. A lambda is not a body of its own: it is part of the body it is written in.

**What a file reports.** `file-bodies` holds `types` (`type-shape`: type parameters and bases), `members` (one `member-body` each), `typed-members` and `metas` (`declaration-meta`: modifiers, attributes, tags). A `member-body` holds:

- `nodes`: the body tree, normalized by logic rather than by spelling. `if`/`else if`/`else` and `switch` are both a `choice` of `arm`s; `for`, `foreach`, `while` and `do` are all a `loop`. A node comes before its children, and ids equal indexes;
- on a node, a `condition` with an expression tree (`expr`, operands before the node that uses them, the last node the root). The host compares and negates conditions over this tree, never over the text. `enum-refs` and `reads` are candidates the host filters;
- `accesses`: reads, writes, calls, returns and mutations of a member, a local or a parameter, each with an `access-path` (a root of `this`, `base`, `local`, `name` or `expression`, and the segments written after it) and, for a write, the value written;
- `invocations` and their `argument`s, each value a `value-ref` (a parameter, a path, a lambda, a literal, a construction), with a `flow-target` for where the result goes;
- `lambdas`, `subscriptions` (`x.E += handler`), `literals` with their `literal-context`, `name-ofs` and `typed-names`;
- `tags`, on a node, on a body or on a declaration.

[`wit-v3/extractor.wit`](../wit-v3/extractor.wit) documents every record and field; read it for the detail.

**Do not filter secrets.** Every text a plugin returns in `bodies` (literal values, condition texts, attribute arguments, tag values) is redacted by the host before it is stored, against the run's secret prefixes. The plugin does not filter and is not given the configuration: one filter, in one place.

**Tags are an open set.** A tag has a `name`, an optional `value` and further `args`. The host reads the tags it knows by name and carries the rest. List the tags you can report in `manifest.emits`.

## References by name

A declaration can refer to another by its name alone (`{StaticResource K}`), when only the whole repository can say which declaration is meant. Report it as a `name-ref` in `file-declarations.name-refs`:

- `source`: the key of the node that refers, a symbol this file declares or the file itself;
- `name`: the name as the reference spells it;
- `target-kind`: the node kind it can resolve to, from your vocabulary;
- `relation`: the edge kind the resolved reference becomes, an edge kind from your vocabulary or `extends`; `none` for a plain reference;
- `block`: the innermost block the reference is written in, or `none`;
- `place`.

For a language that looks names up block by block, also list the nested blocks in `file-declarations.blocks`. Each `lookup-block` has its `lines` and the index of its `parent` (`none` for an outermost one). A block is in force over its lines and inside every block nested in it, and the `block` index of a symbol or a reference points into this list.

The contract: the host looks a reference up once every file has been read and admits what it cannot resolve rather than dropping it. The first scope that holds a declaration of that name and kind decides: the reference's block and its parents, the same file, the files it imports (transitively), the files of the same project (the nearest directory with a project file), and last the whole repository. The first four give a rules-backed edge, the last a name-only one. Several candidates in the deciding scope give an ambiguous edge to each, unless their files are variants of one another (the same relative path under sibling project roots, or the same set of top-level names), when each gets an edge of its own. A reference that resolves nowhere becomes an `unresolved_reference` fact on its file. A target kind `xaml:resource` also matches styles and templates, because `{StaticResource K}` does not say which one `K` is. A name is redacted like any other text value, so it still matches the declared name it means.

## The sandbox

| | |
|---|---|
| Files | none: zero preopened directories |
| Network | refused |
| Environment and arguments | empty |
| Clocks | frozen at zero |
| Randomness | a fixed seed |
| Fuel | 500 million units per call, plus 50,000 per byte of input |
| Memory | 512 MiB per instance |

A plugin that traps or runs out of fuel on a file fails that file only. `coverage.json` then says which plugin failed, in which pass and why, and the run goes on. The next run retries the file rather than caching the failure.

Text a plugin returns (signatures, comments) is redacted again by the host with the built-in and operator-supplied credential prefixes. `config.secret-prefixes` is passed so that a plugin can redact early, but the host does not rely on it.

A plugin does not report string literals, ordinary comments or the SQL inside literals. The host's own lexer reads them from every file and attaches them to the innermost declaration the plugin returned around them: they become the anchors, keywords and database accesses of the cards and details. C# `case` labels and switch arms over an enum are read the same way. C and C++ files, which no plugin reads, still get the texts and the database accesses, owned by the file. Neither contract changed for it.

Output must be deterministic: the same bytes in, the same ops out, in the same order.

## Building

```toml
[lib]
crate-type = ["cdylib", "rlib"]    # rlib lets `cargo test` run natively

[dependencies]
wit-bindgen = "=0.51.0"
```

```rust
// `wit-v3` for contract 0.3; `wit-v2` for 0.2; `wit` for the frozen 0.1.
wit_bindgen::generate!({ path: "path/to/wit-v3", world: "language-plugin" });

// Only the component exports the interface. A native test build cannot carry
// the export names, so keep them out of it.
#[cfg(target_family = "wasm")]
export!(MyPlugin);
```

```sh
rustup target add wasm32-wasip2
cargo build --release --target wasm32-wasip2
reverse-architect my-project --plugin target/wasm32-wasip2/release/my_plugin.wasm
```

`cargo build --target wasm32-wasip2` produces a component directly, so no `cargo-component` step is needed. A plugin that links C code (a tree-sitter grammar, say) also needs [wasi-sdk](https://github.com/WebAssembly/wasi-sdk). See `plugins/build.sh` for the environment the bundled grammars use.

Keep the logic in plain Rust functions and test those natively (`cargo test`), as `examples/plugin-sql` does. The WIT types are ordinary structs you can assert on.

## Loading

```sh
reverse-architect project --plugin a.wasm --plugin b.wasm      # a wins over b wherever both claim an extension
reverse-architect project --plugin a.wasm=sha256:<64 hex>      # refused unless the bytes match
```

Every declared plugin is read, hash-checked and loaded before the first file is touched. A wrong digest, a broken component, a component that implements neither contract version or two plugins with one id end the run immediately, with the reason. The manifest records every plugin a corpus was built with (`plugins`: id, version, sha256, bundled or declared). The cache is keyed on the same list, so swapping a plugin never serves results the old one produced.
