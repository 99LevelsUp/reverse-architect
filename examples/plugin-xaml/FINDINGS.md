# Findings: what XAML needs from the plugin contract

This plugin was built the way a third party would build one: from the published contract and
[`docs/plugins.md`](../../docs/plugins.md) alone, in its own workspace, loaded with `--plugin`, with no change to
the host. Its first version targets `ra:plugin@0.2.0` ([`wit-v2/extractor.wit`](../../wit-v2/extractor.wit)).
This book records, for every XAML construct the plugin reads or was asked to read, whether the 0.2 contract
carries it:

- **fits** — 0.2 says it, and the host does the right thing with it;
- **sacrificed** — left out on purpose, as a known limit;
- **needs-ra-change** — the contract or the host cannot carry it; the proposal says what would.

It describes constructs of the language, not any repository it was run over. Line numbers refer to
`wit-v2/extractor.wit` (WIT) and `docs/plugins.md` (docs) at the revision the plugin was built against.

## Constructs

| Construct | What the plugin does on 0.2 | Verdict | Decided by / proposal | State |
|---|---|---|---|---|
| `TargetType` | `reference` usage of the type; a style or template with no `x:Key` is keyed `type:<CLR name>` | fits | usage kinds, WIT 136–146; docs 74–75 | — |
| `DataType` | the same (implicit `DataTemplate`) | fits | as above | — |
| `{x:Type T}` | `reference` usage; for a nested type (`Outer+Inner`) the outer type goes to `qualifier` | fits | `qualifier`, WIT 247–251; without it the nested-type rule disputes the right candidate (docs 98) | — |
| `xmlns` → CLR namespace | every `clr-namespace:`/`using:` mapping of the file becomes a using of scope 0; a prefix mapped to a URI gives `context: none` | fits, with a limit | scopes and usings, WIT 162–178; docs 94–96. A type name that exists in two mapped namespaces stays ambiguous: the visibility rule disputes the namespace that is not visible but does not back the one that is | — |
| `qualifier` on a type reference | not used for the CLR namespace of a prefix | needs-ra-change | No rule runs on a site whose `qualifier` is a namespace (docs do not say what the host does with it). Proposal: a rule that backs the candidate whose namespace equals the qualifier and disputes the others | open |
| Type-keyed implicit styles | key `type:<CLR name>`, the same key whatever prefix spells the type | fits | keys, docs 44 | — |
| Resources with `x:Key` | symbol of kind `resource` | fits | a kind outside the core lists is kept and shown, nothing resolves to it (docs 64) | — |
| Styles | symbol of kind `style` | fits | as above | — |
| Templates | symbol of kind `template` | fits | as above | — |
| Kinds with a card of their own | styles, templates and resources are members of the file's card only | needs-ra-change | 0.2 cannot create a node of a plugin's own kind: every symbol becomes a core symbol node, and only the core kinds get a card. Proposal: a symbol may carry a node kind from the plugin's vocabulary, which gets the card its policy names | closed in 0.3 (`symbol.node-kind`) |
| `x:Name` inside a template | symbol of kind `element`, keyed `<container>/<name>`, with a `symbol_container` fact; no `has_member` edge | fits | a `has_member` target is left off the file card as a member of a type (docs 64, 66), and a template has no card of its own on 0.2 | — |
| Nesting of elements under templates | flattened to `symbol_container` | needs-ra-change | Proposal: with kinds of their own (above) a template can own its elements | closed in 0.3 |
| `x:Name` outside templates | not reported | sacrificed | in a file with `x:Class` these names are fields of the generated code-behind, not XAML declarations | — |
| `x:Class` | symbol `sym:xaml:<file>#x:Class` of kind `view`, name position on the value; no key of the C# class is declared | needs-ra-change | `view` is a referenced kind with a card of its own (WIT 92–94, docs 63), so one class gets a second card beside its C# card; the plugin qualifies `symbol_name` to keep the XAML symbol out of the candidates of every reference to the class. Proposal: a kind of the plugin's own vocabulary with a card policy (0.3), or a "second half of a partial type" relation the host folds into the C# card | closed in 0.3 (`xaml:view`) |
| Merged dictionaries | `imports` edge to the merged file, from a relative path or a `pack://…;component/…` URI mapped to a top-level directory named after the assembly, always confirmed through `path-exists` | fits | edges, docs 66; host import, WIT 333–337 | — |
| `*.Resources` blocks | containers of resources; a key repeated in one file gets `~N` | fits | keys, docs 44 | — |
| Block a resource or a reference sits in | not reported | needs-ra-change | Nothing in 0.2 says which block a definition or a reference is written in, so nearest-block lookup is impossible. Proposal: a list of the file's lookup blocks with line ranges and parent index, and a `block` index on a resource declaration and on a reference | closed in 0.3 (`file-declarations.blocks`, `symbol.block`, `name-ref.block`) |
| `{StaticResource K}`, `{DynamicResource K}` | not reported | needs-ra-change | No usage kind resolves to a resource (docs 64). Proposal: a reference by name to a declaration of the plugin's own kind, resolved by the host after all files are read, nearest scope first | closed in 0.3 (`name-ref`) |
| `BasedOn` | only `{StaticResource {x:Type T}}` naming a type-keyed style of the same file becomes an `extends` relation | needs-ra-change | both ends are cardless kinds, so the host lifts the relation onto the file and the file card says it extends itself. Proposal: a `name-ref` with relation `extends` between nodes of the plugin's own kinds; in the meantime the host should drop a relation whose ends fall on the same card | closed in 0.3 (`name-ref.relation`) |
| Binding paths | not reported | needs-ra-change | No usage kind expresses reading a property on a type the file does not name (WIT 136–146). Proposal: member accesses with a path and a mode, read in a pass over element bodies, and a tag saying where the binding's data comes from | closed in 0.3 (`bodies`: `member-access`, tags) |
| `Command=` | not reported | needs-ra-change | a binding path (above); a command is also a trigger | closed in 0.3 (`bodies`, tags) |
| Event handlers (`Click="OnSave"`) | not reported | needs-ra-change | not a call but a delegate binding to a method of the class. Proposal: a subscription record | closed in 0.3 (`subscription`) |
| `{x:Static T.M}` | `reference` to `T` | fits; the member `M` sacrificed | no usage kind for a static member read (docs 64) | member: closed in 0.3 (`member-access`) |
| Attached properties (`a:Owner.Prop=`) | not reported | fits (deferred) | the owner type would fit as a `reference`; the write through its accessors needs a member access | closed in 0.3 (`member-access`) |
| `Setter Property=` | not reported | needs-ra-change | the property is a member of the style's `TargetType`; needs a member access | closed in 0.3 (`member-access`) |
| String literals in attributes | not reported | sacrificed | the host's lexer does not read `.xaml`, so attribute values never become keywords; reporting them needs a literal record the host redacts | closed in 0.3 (`string-literal`) |
| `KeyBinding` in `InputBindings` | not reported | sacrificed | the command is a binding, the gesture a literal | closed in 0.3 (tags) |
| Implicit two-way bindings | not reported | sacrificed | the default mode lives in the framework's property metadata, which no file states (WIT 226–227: everything is read from the one file) | — |
| Why a file failed | the file traps; coverage says it failed | needs-ra-change | `declarations` and `usages` return plain lists, so the only failure a plugin can report is a trap, and the panic text is lost. Proposal: a `result<…, string>` or an error record per pass | open |

## Verdict on the architecture

A declarative language fits the plugin model as far as declarations, type references, imports and file
coverage go: all of that came through 0.2 with no change to the host. What did not fit is the declarative
graph itself — resources referenced by name, blocks, the kinds that should carry a card, and everything a binding
says about members. Contract 0.3 adds those as typed records rather than as text, so the plugin moves to 0.3 for
them; the two items marked open stay proposals.
