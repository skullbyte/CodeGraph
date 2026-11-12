# codegraph TODO (audit of 2026-11-12)

Written so any model (Sonnet included) can pick an item up cold. Order = value per token spent.
Rules stay the same: failing test first, `just check`, `code-reviewer` loop, no new dependency without asking.

## Audit result in short
- Size: 39 Rust files, 19.2k lines of code, 531 tests; page = 10 JS parts (1.3k lines).
- Speed is already good; the release profile is NOT the bottleneck. Measured 2026-11-12 on tokio (hyperfine, a tool that times commands many times):
  fat LTO + `codegen-units=1` + `strip` vs the plain release build: cold `index` 142.7 vs 144.6 ms (-1%, noise), `callers spawn` 36.0 vs 38.4 ms (-6%), binary 32 vs 35 MB.
  So remaining time is in our own code and file formats, not compiler flags.
- Weak spots found: token cost of our own docs, stale installed binary / no SCIP data in this repo, cold index 2x slower than the older notes, files that grew too big.

## A. Token savers (do first: they make every later session cheaper)
- [x] A1. Done 2026-11-12 (55.8 KB -> under 15 KB; history in `docs/HISTORY.md`, CLAUDE.md updated). Was: Split `docs/PROGRESS.md`. It is ~60 KB (lines of 1-3 KB) and CLAUDE.md says to read it at the start of every task: ~20k tokens each session. Move `## Decisions` and the dated lines of `## Gotchas` to `docs/HISTORY.md` (read only on demand, with `rg`); keep in PROGRESS.md only Now, Checklist, parity table and the short gotchas. Update the `handoff` skill and CLAUDE.md to say where dated notes go. Check: `wc -c docs/PROGRESS.md` under 10 KB.
- [x] A2. Done 2026-11-12 (old lines moved to HISTORY, this file is the single list). Was: Merge the duplicate lists in the PROGRESS checklist (lines 18-23 repeat the same open items three times) into one list. Fold this TODO in or point to it.
- [ ] A3. Commit (with the rest of this audit, via /commit) the pending comment reflow in `src/lang/generic.rs` (2 lines, harmless, sitting uncommitted).

## B. Keep the tool honest about itself
- [x] B1. Done 2026-11-12: binary reinstalled, `just scip` resolved 14368 of 14885 calls (was 0). Use `cargo install --locked --path .` from now on: without `--locked` cargo ignores Cargo.lock and pulls newer dependencies (it took tree-sitter 0.27.2) than the ones the tests ran with. `just install` does this now.
- [ ] B2. Make this visible automatically: the MCP `stats` tool prints a one-line hint when SCIP data is missing or stale (`Stats.stale_documents` already exists). `stats` is MCP-only today (`codegraph stats` = "unrecognized subcommand"): add the CLI command too. Small, ours.
- [x] B3. Measured 2026-11-12 (release, tokio copy, hyperfine): cold `index` 86 ms without saved SCIP, 131 ms with it (re-applying the 22 MB `rust.scip` costs ~45 ms), cached 40 ms. Older notes: 55-70 / 25-28 ms, so parsing got ~20 ms slower with forms/returns/docs and cached ~12 ms. New items: C0 below. Was: Re-measure and write down the real numbers. Cold tokio `index` measured 144 ms today; notes say 55-70 ms (before docs scan, SCIP re-apply, forms, returns). Profile once (`perf record` or timing prints per phase) and record the split, so the next perf work starts from facts.

## C. Performance (only after B3 shows where the time goes)
- [ ] C0. SCIP re-apply is a third of a cold index (45 of 131 ms): skip it when no re-parsed file is covered by the saved SCIP, or decode only the documents of re-parsed files. Then find the +12 ms in cached `index` (40 ms vs 28).
- [ ] C1. graph.bin load is the floor of every query (~15 ms on tokio, `callers` 36 ms). Options already noted in PROGRESS line 24: intern (store once, refer by number) caller signatures per file, or per-file shards. Needs the user's format decision; bump `Index` VERSION.
- [ ] C2. graph.json: 8 of 13 MB is repeated caller signatures. Nothing in codegraph reads it; either intern them or write it only on `--json`/export. Ask the user.
- [ ] C3. `callers` builds the resolver for names with >= 2 definitions (tokio 20 -> 36 ms). Cache what it needs in graph.bin or build only the parts for that one name.
- [ ] C4. Kotlin/Java import suffix match is still O(files) per import: add a basename index (map from file name to paths) when a 20k-file repo shows it. Not before.
- [ ] C5. Clone/alloc hot spots to look at with a profiler, not by eye: `src/query.rs` (33 `.clone()`), `src/lang/rust.rs` (19), `src/usepath.rs` (18). Only change what the profile shows.
- [ ] C6. Optional, tiny: `[profile.release] strip = true` saves 3 MB of binary. Do NOT set `panic = "abort"`: the PDF reader relies on `catch_unwind` (catching a crash instead of dying).

## D. Structure (makes agent edits cheaper: smaller files = fewer tokens per read)
- [~] D1. Done 2026-11-12 for `src/query.rs` (1976 -> 1026) and `src/lang/rust.rs` (1964 -> 909): tests moved to `src/query/tests.rs` and `src/lang/rust/tests.rs`, 489 + 41 tests still pass. Same move next for `src/scope.rs` (1356), `src/lang/typescript.rs` (1331), `src/scip.rs` (1180). One file per commit, tests must stay green untouched.
- [ ] D2. `src/main.rs` is 846 lines (CLAUDE.md line fixed 2026-11-12): move command bodies out if any live there.
- [ ] D3. Unchecked `[]` lookups in `src/report.rs` / `src/community.rs` -> `.get()` (open since Phase 2).
- [ ] D4. Untested paths: `benchmark` with Graphify, `benchmark --json`, weighted multi-level Louvain.

## E. Precision (the user's top goal; hardest items, best done with the strongest model)
- [ ] E1. TS return-type chains (`z.string().optional()`): zod is 8% right. Rust already has `Ret`/`Form::Returned`; port the same design to `src/lang/typescript.rs`.
- [ ] E2. Rust type identity: several `Builder`/`Runtime` types share a name (3,517 tokio calls). Needs type = (file, name), not name.
- [ ] E3. Rust `macro_rules!` definitions as symbols (1,122 unscored tokio answers) and receivers inside macro arguments (2,845 misses).
- [ ] E4. Fields of generic structs; tuple patterns after `..`.
- [ ] E5. Forms for Java/Kotlin/Go/C#/C++/Ruby/PHP/Swift. Blocked: scoring needs SCIP indexers that run the project's build; the user must OK each (`scip-java`, `scip-go`, `scip-clang`, `scip-dotnet`).
- [ ] E6. File explain ("ask about a file"), promised 2026-11-01.
- [ ] E7. Stemming for `query` ("notifies" should match `notify`).

## F. Graphify parity (39/118 = 33%), in the user's order
- [ ] F1. Big-graph html view: aggregated community view above 5000 nodes, `--node-limit`, `--no-viz` (`G/exporters/html.py`).
- [ ] F2. Obsidian `canvas` export (`to_canvas`); then re-port `obsidian` and `mermaid` from Graphify's real behaviour (built without reading its code, see Provenance).
- [ ] F3. `label` (LLM-named communities) + `save-result` / `reflect`.
- [ ] F4. Dashed edges for non-exact calls in the page (the confidence data now exists).
- [ ] F5. Cheap exports: graphml, svg, tree html. Suggested questions, dedup.
- [ ] F6. More languages on request (Dart first, noted in the checklist).

## Suggested use of the strongest model before 2026-11-17
E1, E2, E3 (design-heavy, many reviewer rounds last time) and the C1/C2 format design. A, B, D, F5 are mechanical and fine for Sonnet later.
