# codegraph progress

## Now
Task: E1, plan `~/.claude/plans/refactored-cooking-umbrella.md` (approved 2026-11-12, 4 steps). Steps 1+2 committed in bd020c2 (not pushed): TS `returns_of` + `hop_of`/`Form::Returned` in `call_form`; `just check` green (495 lib + 44 cli).
zod after 1+2: form still 8% right / 8% wrong; new "chain" row 3866 calls: 6% right, 0% wrong, 94% narrowed (needs step 4, inheritance).
E1 steps 3+4 done 2026-11-13, uncommitted (with TODO 00): call-made `const` locals (`Known::{Class, Value(Node)}`, top level too) and inheritance (`Symbol.bases` from TS extends/implements, `Resolver::members_up`, VERSION 10); `just check` green (500 lib + 44 cli), binary installed.
zod: form right 9% / wrong 8% (was 8/8); type 88% right (was 4%); chain 7737 calls 5% right / 0% wrong: blocked by package self-imports (TODO E1b). tokio 64/0 and this repo 97/0 unchanged.
code-reviewer: 3 rounds, ends "no remaining issues" (fixed: shadowed names in a stored value, overloads with different returns, `require` locals); `just check` 502 lib + 44 cli.
Next step: commit (ask the user: TODO 00 rules split, then E1 steps 3+4), then E1b or E2.
Scores (tokio form): right 64% / wrong 0% / narrowed 22% / lost 0%; this repo 97% right; zod 8%. SCIP on this repo: 14454 of 14973 calls resolved (2026-11-12).
`origin` = github.com/skullbyte/CodeGraph; `main` tracks `origin/main`, pushed up to 98b3143, bd020c2 local only (user pushes with `! git push`).
Rules: tests first, model `opusplan` (opus only for hard tasks, chosen before the first message), `code-reviewer` (sonnet, diff only, max 3 rounds) until "no remaining issues", commit by file name, no AI attribution, never push unless asked, `cargo add` only by the user with `!`.

## Checklist
- [x] Phases 1-4 (done 2026-10-27..30): core engine, communities + report + benchmark, html/mermaid/obsidian, 9 languages (Rust, TS/TSX, JS, Python, Java, C#, Go, C, C++)
- [~] Phase 5 (A.5) Precision: SCIP import (rust-analyzer, scip-typescript), committed and checked on this repo, pizzeria and tokio (numbers in Decisions)
- [x] Phase 6 (A.6) Claude Code integration: MCP server, hook, skill, `install`/`uninstall` (installed here; re-check the SessionStart codemap.sh line after any reinstall)
- [x] Phase 7 (A.7) Documents: markdown scan, `docs pending|ingest|text`, `.claude/semantic.json`, skill `codegraph-docs`, PDFs via `pdf-extract` (done 2026-11-01)
- [~] Part B: full Graphify parity
  - [x] round 1: `query` (5f76a49), `tour` (62d7c39), `--teach` (24f6bf2 + 0e68c28)
  - [ ] round 2, in the user's order: [ ] more languages: [x] Kotlin, [x] PHP, [x] Ruby, [x] Swift (one commit each, plan ~/.claude/plans/swirling-cooking-robin.md); [ ] big-graph html view (aggregated above 5000 nodes, --node-limit, --no-viz); [ ] Obsidian `canvas` export; [ ] `label` (LLM-named communities) + `save-result`/`reflect`
  - [~] precision plan 2 (~/.claude/plans/synthetic-foraging-milner.md): [x] TS steps 1-4 (d851158, 4c0f85d, 8a759ec); [x] generic walker container/owner/form + Python (131a8c3); [x] `explain` modes (ad42b5f); [x] Rust receiver types (2d9efdb); [x] call chains via return types; [x] `--def` filter (wired into callers/explain); [ ] file explain; [x] E1 TS return types for chains: [x] 1 return types, [x] 2 chains (bd020c2), [x] 3 call-made locals incl. top level, [x] 4 inheritance (`bases`, `members_up`); [ ] forms for Java/Kotlin/Go/C#/C++/Ruby/PHP/Swift (needs SCIP truth, blocked, see Now); [ ] Dart/Flutter
  - [x] 2026-11-02: `callers`/`explain` use the resolver + `--def`; `scip --run` + saved-SCIP re-apply in `index`; `update` delta log (7 ms)
- Open items, in priority order: `docs/TODO.md` (the single list; older duplicate lines are in `docs/HISTORY.md`)

## Graphify parity checklist
Compared on 2026-10-28 against Graphify 0.9.97 (source: see CLAUDE.md). `G/` = graphify package dir. Mark: [x] done, [~] partial, [ ] todo. Update when a feature lands.

### Parity score (2026-10-30; item = 1, partial = 0.5; Graphify 0.9.97 `--help`, `exporters/`, `extractors/`, `extract.py`)
| Area | Done / total | % |
|---|---|---|
| Commands (install, path, explain, diagnose, clone, merge-driver, merge-graphs, add, watch, update, cluster-only, label, query, affected, god-nodes, prs, save-result, reflect, check-update, tree, extract, provider, global, benchmark, hook, serve) | 8 / 26 (done: path, explain, watch, update, cluster-only, affected, god-nodes; partial: extract = AST only, benchmark) | 31% |
| Exports (json, html, obsidian, canvas, graphml, svg, cypher/falkordb, wiki, callflow html, tree html) | 3 / 10 (json, html; partial: obsidian, callflow) | 30% |
| Analysis (communities, god nodes, surprising links, cohesion, suggested questions, dedup, resolution passes, incremental cache, rcfile, query log, security caps, validate) | 5 / 12 | 42% |
| Web view features | 11 / 16 | 69% |
| Languages (Graphify has ~45 extractors) | 13 / 45 (Rust, TS/TSX, JS, Python, Java, Kotlin, PHP, Ruby, Swift, C#, Go, C, C++) | 29% |
| Ingest / semantic (docs, PDF, images, audio, Google, Postgres, manifest, URL, LLM pass) | 3 / 9 (markdown scan, PDF text, LLM pass via skill; ours is Claude-Code-only, no API key) | 33% |
| **Total** | **39 / 118** | **~33%** |

### Provenance: did we read Graphify's code first?
- Followed its source: communities (`cluster.py`: Louvain idea, cohesion, both split rules), report (`report.py`: surprising links), web view features (`exporters/html.py`).
- Only the idea, not the detail: languages (`LanguageConfig` table idea; node-type tables written by us; `engine.py`/resolution passes not read).
- NOT read before building (rule broken): `mermaid` and `obsidian`. Graphify's real versions differ a lot (comparison on 2026-10-30): `callflow_html.py` draws a whole-graph, community-level overview plus one capped chart per community (18 nodes / 24 edges, file subgraphs, kind classes, edge ranking, `%% Omitted` note, HTML with pan/zoom, Mermaid from a CDN); `to_obsidian` writes one note per NODE named by label (dedup `_N`), frontmatter `source_file/type/community(name)/location/tags`, `## Connections` with `[[Target]] - \`relation\` [CONFIDENCE]`, `_COMMUNITY_<label>.md` notes with cohesion, members, bridge nodes, cross-community links, `.obsidian/graph.json` colours, a manifest that never clobbers user notes and prunes stale ones; `to_canvas` writes a `.canvas` (community groups in a grid, file cards, top 200 edges). Ours: one function's callees; one note per file. These two are ours, not ported.
- Ours only (no Graphify equivalent): offline 3D view, SCIP import (Graphify's `scip_ingest.py` is a skeleton), per-definition hubs, coverage report, broken-pipe hook.

Web view (`G/exporters/html.py`; Graphify's is 2D vis-network 9.1.6 from a CDN, NOT 3D, NOT offline):
- [x] interactive graph page: ours = offline plain-canvas page with own 3D force layout (`src/html.rs` + parts in `src/html/`); Graphify uses vis-network from a CDN
- [x] search box, click-to-inspect side panel with neighbours
- [x] community colouring; [x] community legend with per-community checkboxes + All/None
- [x] node size by degree; [x] labels only on hubs (degree >= 30% of max, Graphify 15%; also selected and search hits)
- [x] hover tooltip, system/light/dark theme toggle (saved in localStorage), stats (header status: files, links, communities)
- [x] arrows on edges (direction = imports/calls); [ ] dashed edges for non-EXTRACTED confidence (needs Phase 5 confidence data)
- [ ] aggregated community-level view above 5000 nodes (`MAX_NODES_FOR_VIZ`, `GRAPHIFY_VIZ_NODE_LIMIT`), `--node-limit`, `--no-viz`
- [ ] hyperedges as convex-hull regions; learning overlay (green/amber/stale rings; needs `reflect`)
- [x] extra, not in Graphify: folder/kind/community filters
- [x] extra, not in Graphify: offline 3D view (perspective, depth shading, drag to rotate, spin, 2D/3D toggle)

Exports (`G/export.py`, `G/exporters/`):
- [x] graph.json (ours also graph.bin)
- [x] GRAPH_REPORT.md (`report.py`)
- [~] callflow-html (`callflow_html.py`: Mermaid page with pan/zoom; Mermaid from a CDN) -> ours: `codegraph mermaid` prints plain Mermaid text (callees only, depth, 60-edge cap), no page
- [~] obsidian vault (`to_obsidian`): done as `codegraph obsidian` (file notes, community notes, `[[path.md|path]]` links); [ ] canvas (`to_canvas`)
- [ ] tree (`tree_html.py`, D3 collapsible tree), wiki, svg, graphml, neo4j, falkordb -> Part B

Commands:
- [x] path, explain, affected, god-nodes (= our `hubs`), watch, update, benchmark (~: index time + explain tokens only)
- [x] cluster-only (= our `report`); [ ] label (LLM-named communities)
- [x] query "<question>" (`src/ask.rs`; Graphify `serve.py` `_score_query`/`_pick_seeds`/`_bfs`; ours: symbol nodes, hubs last, 1000-token default, MCP tool `query`)
- [ ] diagnose multigraph, check-update, cache-check
- [ ] hook install/uninstall/status (git hooks), install/uninstall per AI platform, serve (graph server/MCP) -> Phase 6
- [ ] extract (AST + LLM), provider, add <url>, transcribe (audio/video), manifest/mcp/pg ingest -> Phase 7 / Part B
- [ ] merge-graphs, global graph, merge-driver, clone, prs, save-result, reflect -> Part B

Analysis:
- [~] communities: Louvain + both split rules (Graphify: Leiden preferred, Louvain fallback), file-level
- [x] god nodes, surprising links; [ ] suggested questions, dedup (`dedup.py`, MinHash)
- [~] languages: Rust, TS/TSX, JS, Python, Java, C#, Go, C/C++ of ~30 (`G/extractors/`); others (Ruby, PHP, Kotlin, Swift, ...) on request
- [ ] resolution passes (symbol/interface/cross-repo, `extractors/resolution.py`), SCIP ingest (`scip_ingest.py`) -> Phase 5
- [ ] project config `.graphifyrc`, query log, security caps (URL/SSRF, size limits) -> Part B

Licence: Apache-2.0 (LICENSE) + MIT for older code (LICENSE-MIT) + NOTICE; Graphify ships no JS. If we port its code or embed vis-network/d3/mermaid, keep those notices and check each library's licence first.

## Decisions
New decisions go here, one dated line each; older ones are in `docs/HISTORY.md` (search it with `rg`, never read it whole).
- 2026-11-12 PROGRESS.md split: dated decisions/notes live in `docs/HISTORY.md` (search with rg), open work in `docs/TODO.md` (single list): PROGRESS was ~20k tokens read every session
- 2026-11-12 big test blocks move to `src/<module>/tests.rs` (`mod tests;`): agents read the code without 1k lines of tests
- 2026-11-12 workflow: default model `opusplan`, `/model opus` only before a hard task (a mid-session switch re-reads the whole context uncached); all subagents incl. `code-reviewer` on sonnet, diff only, max 3 rounds; global CLAUDE.md uses `codegraph` for code search in every project (callers.sh retired)
- 2026-11-12 priority: precision of languages before more speed work (user): E1 TS chains, E2 Rust type identity, E3 macro_rules; then C3, C1/C2
- 2026-11-12 `index` re-matches SCIP calls only in re-parsed files and skips writing an unchanged graph; a SCIP file newer than graph.json forces both a full re-apply and a write: else its stamp keeps forcing a full re-apply
- 2026-11-12 E1 includes inheritance (`Symbol.bases`, resolver walks bases when a type has no member of that name): zod misses show `parse` lives on base `ZodType`, so chains alone gain nothing (measured)
- 2026-11-12 E1 out of scope, to TODO: `Module("util")` matching both v3 and v4 util files; two `ZodType`s (v3/v4) sharing a name
- 2026-11-12 release profile tuning (fat LTO, codegen-units=1) measured: -1% index, -6% callers: not worth the build time; `strip` only saves 3 MB; never `panic = "abort"` (PDF reader uses catch_unwind)

## Gotchas
- `cargo install` without `--locked` ignores Cargo.lock (pulled tree-sitter 0.27.2): use `just install`. `codegraph stats` (and MCP `stats`) warns when SCIP data is missing or stale
- Pushing: ask the user to run `! git push` (Claude is denied, no `gh` login). Check `git remote -v` after any `set-url`: a stray trailing `.` once broke the URL
- After `cargo fmt`, a literal `str.replace` on code can silently miss (fmt reflows lists/calls): twice a reviewed fix was not applied. Check with `rg` that every scripted edit landed; prefer slices over fixed-size arrays for name lists
- Linux file times advance in coarse clock ticks (a few ms): tests comparing mtimes must sleep ~50 ms between writes. Multi-line `sd -F` patterns silently fail to match: use Edit
- Reviewer subagents sometimes edit the file to probe: check `git diff --stat` and run `just check` before committing
- tree-sitter grammar field names differ per version: check `node-types.json` in ~/.cargo/registry/src/*/tree-sitter-<lang>-*/src (e.g. rust `type_parameters` children are `type_parameter`)
- Scratchpad analysis scripts (byform.py, meth*.py, barewrong.py, chainshare.py, score.py, unscored.py) are in the session scratchpad and not kept
- zsh does not split a string variable into words: use arrays (`d=(--dir x)`, `"${d[@]}"`) in shell loops; `set -- $pe` with a space-joined pair breaks
- Graphify, offline: `graphify extract <dir> --code-only --force --out <tmp>`; output `<tmp>/graphify-out/graph.json` (nodes with `source_file`, `label`; `links` with `relation`); scratchpad scripts `cmp.py`, `cls.py`, `rev.py`, `wrong.py`, `forms.py` live in the session scratchpad and are not kept
- Real clones (git-ignored) are in `bench/real/`: okhttp, framework (laravel), rails, Alamofire, plus older gonet/gostd/glm/spring/graphify-py
- A big string splice with `s[:a] + new + s[b:]` where `b < a` duplicates code; check function lists with `rg -n '^(fn|const|struct)' file` after such edits
- `codegraph install` converts any line equal to the old codemap.sh command into `codegraph index`: check `.claude/settings.json` SessionStart after a reinstall
- `paru -S` aborts the whole transaction when one package fails to download (difftastic did on the mirrors; a retry later worked); zsh aborts a command whose glob matches nothing before running it
- `fd` skips hidden dirs (use `-H`)
- bench/ (git-ignored) holds tokio and a copy of pizzeria-emiliano; `rm -rf` is denied in settings, delete files one by one if needed
- Every `cargo build` changes the executable fingerprint, so the next `index` is a full re-parse
- zsh `time` has no "real" line: time with `bash -c` and `date +%s%N`
- The auto-mode classifier blocked `cargo add`/downloads once; the user ran them with `!`
- `update <relative path>` is resolved against the cwd first, then --dir; the format hook passes absolute paths
- `tests/cli.rs::watch_*` runs the real binary with a 50 ms poll; it keeps the child's stdout reader alive (a closed pipe kills `watch`)
- Hubs/path/explain on big repos are dominated by generic names (`new`, `clone`) until Phase 5
- Phase 2 numbers (release): report tokio 53 ms (91 communities, biggest 179 files, name-match noise like `new_v4` links; Phase 5 fixes), pizzeria 2 ms (14 communities); benchmark: index tokio codegraph 44 ms vs graphify 4391 ms, pizzeria 3 vs 557 ms; explain formatPrice 144 tok/1 ms vs 313 tok/96 ms
- To check later (open issues from the Phase 2 review, none block Phase 3):
  - unchecked `[]` lookups (`pos[..]`, `community_of[..]`, `index.files[..]`) in src/report.rs and src/community.rs: safe today, switch to `.get()` if the invariants change
  - untested: `benchmark` with Graphify installed, `benchmark --json`, multi-level Louvain with weighted edges
  - `benchmark` explain answers read the saved index, which can be older than the cold build it times
  - tokio communities are lumpy (biggest 179 files, name-match noise like `new_v4`): Phase 5 fixes the cause; Graphify's oversized-community split was skipped

