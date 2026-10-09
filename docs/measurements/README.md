# Measurements

This directory holds the reports that rights in the code rest on. A resolution rule (catalogue in
`src/extract/resolve/rules.rs`) may only **delete** a candidate, not just dispute it, once a
measurement against a SCIP index shows that it never removed a candidate the index confirmed.
`reverse-architect measure` produces the report; a person grants the right with a commit.

The grant itself is a record in code (`granted_records()` in `src/extract/resolve/rights.rs`) that names
the report, its SHA-256 and the rule revision. Report and record are committed together.

## Running a measurement

```
reverse-architect index <REPO> --restore
reverse-architect measure <REPO> --scip-index <REPO>.reverse-architect/index.scip
```

`--restore` is needed when the repository's packages are not restored. Without them a project still
gets indexed, but the index most likely does not know the symbols from packages and cannot judge the
use sites that lead into them. With `--restore`, `index` runs
`dotnet restore <project>.csproj /p:EnableWindowsTargeting=true` before indexing each project. A
project whose restore fails counts as failed, so no index is written without `--allow-partial`, and
`measure` refuses a partial index. The record next to the index says in its `restore` field whether
the restore ran. `index` runs the repository's build logic (MSBuild): do not run it on a repository
you do not trust.

`measure` builds the graph exactly like a generation with an index (same options, shared cache in the
sidecar or in `--corpus`). It takes `--plugin`, `--ambiguity-ceiling` and `--secret-prefix` like a
generation does. It writes the report to `--out`, or to `<stem>.measurement.json` next to the index.

The index is refused, with the reason and a non-zero exit code, when:

- there is no `<stem>.record.json` from `reverse-architect index` next to it, or its format version differs,
- the index's SHA-256 does not match the record (the index was replaced or changed),
- the index is from a different commit than the repository is on (or the record has no commit),
- the index was built from a working copy with changes (`dirty`),
- the index is partial: some project failed to index.

## What the report counts

The report is JSON with sorted keys, no timestamps and not a single identifier of the measured code
(no files, no symbols), so it can be committed here. For the same commit, index and build of the tool
it has the same bytes.

Header: `schema` (report format version, currently 1), `commit`, `index_sha256`, `resolver` (the
revision of every catalogue rule and the fingerprint of the rights table the run used) and
`thresholds` (the thresholds the proposal was made with).

`origins` holds one entry per origin of use sites: `language`, `component`, `component_sha256` and
`contract` — exactly the fields a right is bound to. Each origin has:

- `sites`: site counts by what the index said about them (`resolved`, `external`,
  `unverified_document`, `unmapped`, `no_document`; their sum is `total`),
- `rules`: for every catalogue rule its `revision`, `may_delete` (whether it may ever delete at all),
  the counts and the `proposal`.

Counts are counts of sites. A rule **judged** a site (`judged`) when it gave at least one candidate a
verdict other than "abstain". What is measured is the trail of verdicts (what the rule said), not the
candidate's outcome: a rule without a right only disputes, but its trail says "exclude", and that is
what counts. Of the judged sites:

- `unmapped`: the index could not judge the site (file without a document in the index, shifted
  document, unpaired occurrence, target without a single verified definition). It neither confirms
  nor refutes anything,
- `evaluated`: the rest, split into four groups; the first that fits applies:
  1. `would_remove_confirmed`: the rule disputed or excluded a candidate the index confirmed,
  2. `confirmed`: the rule backed a candidate the index confirmed,
  3. `refuted`: the rule backed a candidate the index did not confirm (a different one, or any
     candidate where the index points outside the repository),
  4. `disputed_refuted`: the rule only disputed or excluded candidates the index did not confirm.

`judged = evaluated + unmapped` and
`evaluated = would_remove_confirmed + confirmed + refuted + disputed_refuted`.

The host rules `index-elsewhere` and `index-outside` are verdicts of the index, not catalogue rules,
and do not appear in the report.

## The proposal

`proposal` is `proposed` only when all of this holds:

- the rule may delete at all (`arity`, `namespace-visibility`, `implicit-this` and `base-spelling`
  never do: `never-deletes`),
- `would_remove_confirmed` is 0 (otherwise `removed-confirmed`),
- `evaluated` is at least `min_evaluated_sites`, and always at least 1 (otherwise `too-few-evaluated`),
- the share `unmapped / judged` is below `max_unmapped_percent` (otherwise `unmapped-share-too-high`).
  A rule without a single unmapped site always meets this, so `--max-unmapped-percent 0` means
  "no unmapped site", not "never".

The defaults are `--min-evaluated-sites 100` and `--max-unmapped-percent 20`.

## From proposal to a record in the table

A report only proposes a right; it writes nothing. Granting is a manual code change:

1. Copy the report here under a descriptive name, for example
   `docs/measurements/2026-10-xx-csharp-reference.json`. The bytes must not change (no reformatting,
   no line-ending change), otherwise the fingerprint does not match.
2. Compute the SHA-256 of the copied file.
3. In `granted_records()` in `src/extract/resolve/rights.rs`, add one `DeleteRight` per proposed rule:
   `rule`, `revision` from the rule's entry in the report, `language`, `component`,
   `component_sha256` and `contract` from its origin, `report` as the path to the report
   (`docs/measurements/…`) and `report_sha256`.
4. Commit the report and the record together.

The test `the_shipped_rights_table_is_consistent_with_the_committed_reports` in `tests/measure.rs`
(function `measure::check_rights`) checks that every record has its report in `docs/measurements/`,
that the SHA-256 matches, that the report measured this origin and this rule revision, that the rule
removed nothing confirmed and that the report proposed it. A record backed by a report with a removed
confirmed edge does not pass.

At run time the resolver applies a right only to sites of the same language, the same component with
the same SHA-256, the same contract and the same rule revision. Rebuilding the plugin component or
changing the rule (a higher revision) voids the right until a new measurement exists.

## Reports in this directory

`2026-10-03-csharp-reference.json` — a large C# repository (149 of 149 projects indexed), component
`csharp` 0.2. C# sites: 642 462 in total, 421 427 confirmed by the index on a declaration in the
repository, 116 082 leading outside it, 82 411 unmapped, 22 386 in unverified documents.

| Rule | Evaluated | Confirmed | Refuted | Disputed unconfirmed | Would remove confirmed | Unmapped | Proposal |
|---|---:|---:|---:|---:|---:|---:|---|
| role-mismatch | 9 310 | 0 | 0 | 9 310 | **0** | 235 | **proposed → granted** |
| declared-receiver | 146 328 | 86 406 | 8 499 | 45 607 | 5 816 | 14 543 | removed-confirmed |
| static-receiver | 70 524 | 70 042 | 132 | 322 | 28 | 3 118 | removed-confirmed |
| implicit-this | 61 974 | 61 841 | 133 | 0 | 0 | 3 851 | never-deletes |
| arity | 237 016 | 0 | 0 | 228 966 | 8 050 | 27 055 | never-deletes |
| namespace-visibility | 24 656 | 0 | 0 | 24 639 | 17 | 21 159 | never-deletes |
| base-spelling | 0 | 0 | 0 | 0 | 0 | 0 | never-deletes |

Granted: `role-mismatch` revision 1 for component `csharp` with SHA-256 `800518666c3f…` and contract
0.2 (record in `granted_records()`). The thresholds decided nothing here: the proposed rule has
thousands of evaluated sites and 2.5 % unmapped; zero removed confirmed edges decided it.
