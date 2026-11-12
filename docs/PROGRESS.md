# codegraph progress

## Now
Session 2026-11-12: all committed (up to c69f928), `just check` green (493 lib + 43 cli tests). Done: A3, B2 (`codegraph stats` + SCIP hint), C0 part 1 (partial SCIP re-apply).
Next step: from `docs/TODO.md`: rest of C0 (cached `index` 48 ms on tokio), then C1-C3 (C1/C2 need the user's format decision); hard ones E1-E3 with `/model opus` chosen before starting.
Scores (tokio form): right 64% / wrong 0% / narrowed 22% / lost 0%; this repo 97% right; zod 8%. SCIP on this repo: 14368 of 14885 calls resolved (2026-11-12).
`origin` = github.com/skullbyte/CodeGraph; pushed up to 9ef627d; later commits not pushed (user pushes with `!`).
Rules: tests first, model `opusplan` (opus only for hard tasks, chosen before the first message), `code-reviewer` (sonnet, diff only, max 3 rounds) until "no remaining issues", commit by file name, no AI attribution, never push unless asked, `cargo add` only by the user with `!`.

## Checklist
- [x] Phases 1-4 (done 2026-10-27..30): core engine, communities + report + benchmark, html/mermaid/obsidian, 9 languages (Rust, TS/TSX, JS, Python, Java, C#, Go, C, C++)
- [~] Phase 5 (A.5) Precision: SCIP import (rust-analyzer, scip-typescript), committed and checked on this repo, pizzeria and tokio (numbers in Decisions)
- [x] Phase 6 (A.6) Claude Code integration: MCP server, hook, skill, `install`/`uninstall` (installed here; re-check the SessionStart codemap.sh line after any reinstall)
- [x] Phase 7 (A.7) Documents: markdown scan, `docs pending|ingest|text`, `.claude/semantic.json`, skill `codegraph-docs`, PDFs via `pdf-extract` (done 2026-11-01)
- [~] Part B: full Graphify parity
  - [x] round 1: `query` (5f76a49), `tour` (62d7c39), `--teach` (24f6bf2 + 0e68c28)
  - [ ] round 2, in the user's order: [ ] more languages: [x] Kotlin, [x] PHP, [x] Ruby, [x] Swift (one commit each, plan ~/.claude/plans/swirling-cooking-robin.md); [ ] big-graph html view (aggregated above 5000 nodes, --node-limit, --no-viz); [ ] Obsidian `canvas` export; [ ] `label` (LLM-named communities) + `save-result`/`reflect`
  - [~] precision plan 2 (~/.claude/plans/synthetic-foraging-milner.md): [x] TS steps 1-4 (d851158, 4c0f85d, 8a759ec); [x] generic walker container/owner/form + Python (131a8c3); [x] `explain` modes (ad42b5f); [x] Rust receiver types (2d9efdb); [x] call chains via return types; [x] `--def` filter (wired into callers/explain); [ ] file explain; [ ] TS return types for chains; [ ] forms for Java/Kotlin/Go/C#/C++/Ruby/PHP/Swift (needs SCIP truth, blocked, see Now); [ ] Dart/Flutter
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
- 2026-11-12 release profile tuning (fat LTO, codegen-units=1) measured: -1% index, -6% callers: not worth the build time; `strip` only saves 3 MB; never `panic = "abort"` (PDF reader uses catch_unwind)

## Gotchas
- `cargo install` without `--locked` ignores Cargo.lock (pulled tree-sitter 0.27.2): use `just install`. `codegraph stats` (and MCP `stats`) warns when SCIP data is missing or stale
- Pushing: ask the user to run `! git push` (Claude is denied, no `gh` login). Check `git remote -v` after any `set-url`: a stray trailing `.` once broke the URL
- After `cargo fmt`, a literal `str.replace` on code can silently miss (fmt reflows lists/calls): twice a reviewed fix was not applied. Check with `rg` that every scripted edit landed; prefer slices over fixed-size arrays for name lists
- SCIP indexers live in ~/.local/bin (scip-go, scip-java, scip-clang, scip-ruby), npm -g (scip-typescript, scip-python), ~/.dotnet/tools (scip-dotnet, not on PATH). Run indexers in the repo root; for zod run in `bench/real/zod/packages/zod` (`scip-typescript index --no-progress-bar`; `--pnpm-workspaces` fails). After every `cargo install` the fingerprint changes: re-run `codegraph index && codegraph scip` before `codegraph precision` (tokio: `cd bench/tokio`)
- Reviewer subagents sometimes edit the file to probe: check `git diff --stat` and run `just check` before committing
- tree-sitter grammar field names differ per version: check `node-types.json` in ~/.cargo/registry/src/*/tree-sitter-<lang>-*/src (e.g. rust `type_parameters` children are `type_parameter`)
- Scratchpad analysis scripts (byform.py, meth*.py, barewrong.py, chainshare.py, score.py, unscored.py) are in the session scratchpad and not kept
- Graphify-vs-SCIP scoring script (not kept): for each Graphify `calls` edge take caller file + `source_location` line + target node file/line, look up our call at that line in `.claude/graph.json` (`resolved` after `codegraph scip`), right = same file and def line within 5
- zsh does not split a string variable into words: use arrays (`d=(--dir x)`, `"${d[@]}"`) in shell loops; `set -- $pe` with a space-joined pair breaks
- `just check` is the gate; after a release rebuild every `.claude/graph.bin` is stale (fingerprint), so re-run `codegraph index` and `codegraph scip` before `codegraph precision`
- SCIP truth: `rust-analyzer scip .` in the repo or in bench/tokio (27 s), then `codegraph scip`; `index.scip` is git-ignored; pizzeria (TS) is too small to score
- Graphify, offline: `graphify extract <dir> --code-only --force --out <tmp>`; output `<tmp>/graphify-out/graph.json` (nodes with `source_file`, `label`; `links` with `relation`); scratchpad scripts `cmp.py`, `cls.py`, `rev.py`, `wrong.py`, `forms.py` live in the session scratchpad and are not kept
- Real clones (git-ignored) are in `bench/real/`: okhttp, framework (laravel), rails, Alamofire, plus older gonet/gostd/glm/spring/graphify-py
- A big string splice with `s[:a] + new + s[b:]` where `b < a` duplicates code; check function lists with `rg -n '^(fn|const|struct)' file` after such edits
- `codegraph install` converts any line equal to the old codemap.sh command into `codegraph index`: check `.claude/settings.json` SessionStart after a reinstall
- `paru -S` aborts the whole transaction when one package fails to download (difftastic did on the mirrors; a retry later worked); zsh aborts a command whose glob matches nothing before running it
- Page work: edit only the part files in `src/html/` (js/01..10, style.css, template.html); `rg -c 'https?://|innerHTML|<img|url\(' src/html/` must print nothing; Rust tests fix `const DATA = /*DATA*/;` and `{{TITLE}}`
- Brave tests: serve `bench/pizzeria/.claude` with `exec python3 -m http.server <port> --bind 127.0.0.1` as a background task; stop it by PID from `ss -ltnp` (`pkill -f` kills the shell, exit 144). Regenerate with `cargo build --release && ./target/release/codegraph index --dir bench/pizzeria && ./target/release/codegraph html --dir bench/pizzeria`
- Impeccable files (`.claude/skills/impeccable/`, `.claude/agents/impeccable-*.md`, `.impeccable/`) are gitignored: commit by file name, never `git add -A`
- Agent cost: a design pass ~160-215k tokens, a review 20-85k: keep jobs narrow; `fd` skips hidden dirs (use `-H`)
- bench/ (git-ignored) holds tokio and a copy of pizzeria-emiliano; `rm -rf` is denied in settings, delete files one by one if needed
- Bench numbers (release): tokio 814 files cold index ~55-70 ms, cached ~25 ms, update 1 file ~20 ms; notify_waiters 33 callers; def TcpListener -> tokio/src/net/tcp/listener.rs:57; pizzeria formatPrice 12 sites in 6 files
- Every `cargo build` changes the executable fingerprint, so the next `index` is a full re-parse
- zsh `time` has no "real" line: time with `bash -c` and `date +%s%N`
- The auto-mode classifier blocked `cargo add`/downloads once; the user ran them with `!`
- `update <relative path>` is resolved against the cwd first, then --dir; the format hook passes absolute paths
- `tests/cli.rs::watch_*` runs the real binary with a 50 ms poll; it keeps the child's stdout reader alive (a closed pipe kills `watch`)
- Hubs/path/explain on big repos are dominated by generic names (`new`, `clone`) until Phase 5
- Phase 2 numbers (release): report tokio 53 ms (91 communities, biggest 179 files, name-match noise like `new_v4` links; Phase 5 fixes), pizzeria 2 ms (14 communities); benchmark: index tokio codegraph 44 ms vs graphify 4391 ms, pizzeria 3 vs 557 ms; explain formatPrice 144 tok/1 ms vs 313 tok/96 ms
- `resolve_rust_module` fallback was O(imports x files) with string allocs per file; hoisted them (119 ms -> 13 ms on tokio)
- To check later (open issues from the Phase 2 review, none block Phase 3):
  - unchecked `[]` lookups (`pos[..]`, `community_of[..]`, `index.files[..]`) in src/report.rs and src/community.rs: safe today, switch to `.get()` if the invariants change
  - untested: `benchmark` with Graphify installed, `benchmark --json`, multi-level Louvain with weighted edges
  - `benchmark` explain answers read the saved index, which can be older than the cold build it times
  - tokio communities are lumpy (biggest 179 files, name-match noise like `new_v4`): Phase 5 fixes the cause; Graphify's oversized-community split was skipped

- tree-sitter-rust: a `mut_pattern`'s first named child is the `mut` keyword, find the `identifier` child; `_` in a tuple pattern is a child that must still count for indexes; filter comments and punctuation by kind
