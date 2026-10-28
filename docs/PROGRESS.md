# codegraph progress

## Now
Goal: Part A phase 3 (Visuals): `codegraph html` done (offline 3D space view, reviewed); next `mermaid`, then `obsidian`.
State: Phase 1 is DONE and committed (8674776, 652302a). Phase 2 is DONE, reviewed and committed (src/community.rs, report.rs, benchmark.rs; CLI `report`, `benchmark`). `just check` green: 76 unit + 13 end-to-end tests.
Next step: commit the html work (add files by name, never `git add -A`), then step 2 `codegraph mermaid` and step 3 `codegraph obsidian` per the approved plan (~/.claude/plans/floating-munching-lantern.md).

## Checklist
- [x] Phase 1 (Part A.1) core engine, reviewed and committed
  - [x] Rust + TS/TSX extraction: symbols, imports, calls with enclosing fn (incl. macro args, cfg_*! bodies)
  - [x] walker (.gitignore, skip node_modules/target/Library/build/dist), parallel + incremental index
  - [x] outputs: .claude/codemap.md (codemap.sh format), graph.json, graph.bin (cache)
  - [x] commands: index, update, watch, def, callers, explain, path, affected, hubs; `--json`, `--dir`
  - [x] tsconfig `paths` aliases, project lock, atomic writes, binary cache, README (credits Graphify), Apache-2.0 LICENSE
  - [x] benchmarks on tokio + pizzeria (see Gotchas for numbers)
- [x] Phase 2 (A.2) Analysis: file-level Louvain communities, `codegraph report` -> .claude/GRAPH_REPORT.md, `codegraph benchmark` vs Graphify (reviewed, committed)
- [ ] Phase 3 (A.3) Visuals
  - [x] `codegraph html` (offline page; 2D committed eb91796; space-themed 3D view with Graphify web features done and reviewed, not committed yet)
  - [ ] Mermaid callflow export
  - [ ] Obsidian vault export
- [ ] Phase 4 (A.4) More languages: Python, Java, C#, Go, JavaScript, C/C++
- [ ] Phase 5 (A.5) Precision: SCIP import (rust-analyzer, scip-typescript); fixes name-based matching, e.g. `Some(x)` vs real calls and same-named methods
- [ ] Phase 6 (A.6) Claude Code integration: MCP server, skill, hook; replace codemap.sh/callers.sh only if benchmarks win
- [ ] Phase 7 (A.7) Documents: semantic pass over docs/PDFs/markdown via a skill
- [ ] Part B: full Graphify parity (order chosen by the user later)
- [ ] Open: update on tokio is ~19-21 ms (target < 20 ms); `docs/REQUIREMENTS.md` Part A/B is the source of truth for scope

## Graphify parity checklist
Compared on 2026-10-28 against Graphify 0.9.97 (source: see CLAUDE.md). `G/` = graphify package dir. Mark: [x] done, [~] partial, [ ] todo. Update when a feature lands.

Web view (`G/exporters/html.py`; Graphify's is 2D vis-network 9.1.6 from a CDN, NOT 3D, NOT offline):
- [x] interactive graph page: ours = offline plain-canvas page with own 3D force layout (`src/html.rs`, `src/html/template.html`); Graphify uses vis-network from a CDN
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
- [ ] callflow-html (`callflow_html.py`: Mermaid page with pan/zoom; Mermaid from a CDN) -> ours: `codegraph mermaid`, Phase 3 step 2
- [ ] obsidian vault + canvas (`to_obsidian`, `to_canvas`) -> Phase 3 step 3
- [ ] tree (`tree_html.py`, D3 collapsible tree), wiki, svg, graphml, neo4j, falkordb -> Part B

Commands:
- [x] path, explain, affected, god-nodes (= our `hubs`), watch, update, benchmark (~: index time + explain tokens only)
- [x] cluster-only (= our `report`); [ ] label (LLM-named communities)
- [ ] query "<question>" (BFS/DFS with --depth/--budget)
- [ ] diagnose multigraph, check-update, cache-check
- [ ] hook install/uninstall/status (git hooks), install/uninstall per AI platform, serve (graph server/MCP) -> Phase 6
- [ ] extract (AST + LLM), provider, add <url>, transcribe (audio/video), manifest/mcp/pg ingest -> Phase 7 / Part B
- [ ] merge-graphs, global graph, merge-driver, clone, prs, save-result, reflect -> Part B

Analysis:
- [~] communities: Louvain only (Graphify: Leiden + Louvain fallback, oversized-community split skipped), file-level
- [x] god nodes, surprising links; [ ] suggested questions, dedup (`dedup.py`, MinHash)
- [~] languages: Rust + TS/TSX of ~30 (`G/extractors/`) -> Phase 4
- [ ] resolution passes (symbol/interface/cross-repo, `extractors/resolution.py`), SCIP ingest (`scip_ingest.py`) -> Phase 5
- [ ] project config `.graphifyrc`, query log, security caps (URL/SSRF, size limits) -> Part B

Licence: Apache-2.0 (LICENSE) + MIT for older code (LICENSE-MIT) + NOTICE; Graphify ships no JS. If we port its code or embed vis-network/d3/mermaid, keep those notices and check each library's licence first.

## Decisions
- 2026-10-28 the page source is split into `src/html/style.css`, `src/html/js/01..10-*.js` and a skeleton `template.html`, joined by `assemble()` in `src/html.rs`; output proved byte-identical to the single file. Reason: agents re-read the whole 1700-line file every step (~200k tokens per pass). User rule: one `code-reviewer` run per finished feature
- 2026-10-28 page design: done by agents on Sonnet (user rule: no Opus), using the pizzeria skills (frontend-design, apple-design, animate, visual-qa) plus ui-ux-pro-max, taste-skill and impeccable (installed without hook, telemetry off, gitignored). The page must stay offline, single-file, image-free; the template line `const DATA = /*DATA*/;` and `{{TITLE}}` are fixed by Rust tests; the SVG namespace is built from two strings to keep the no-`http://` test
- 2026-10-28 Graphify's web view is 2D (vis-network via CDN), so "port the 3D web" was a wrong assumption; ours stays offline. User chose: match Graphify's web features, plus our own offline 3D (plain canvas, no libraries)
- 2026-10-28 user rule: test pages in Brave (Flatpak com.brave.Browser); my browser tools only drive Chrome, so verify there and say so
- 2026-10-27 walk the tree-sitter syntax tree by hand, no query API: queries need the extra `streaming-iterator` crate
- 2026-10-27 `calls` are matched by name only (no types): exact resolution is Phase 5 (SCIP), so don't re-litigate it earlier
- 2026-10-27 re-parse the body of `cfg_*!` macros as Rust and token-scan other macro arguments: tokio hides items and calls in macros
- 2026-10-27 keep graph.json (readable) plus graph.bin (postcard, binary): loading the 4.6 MB JSON cost ~15 ms of every query
- 2026-10-27 graph stores a fingerprint of the codegraph executable (size + mtime): a rebuilt binary re-parses everything, no manual VERSION bump
- 2026-10-27 project lock via std `File::lock` on .claude/graph.lock: parallel hook runs can't overwrite each other (rust-version 1.89)
- 2026-10-27 `watch` polls mtime/size instead of using the `notify` crate: no new dependency, ms-level cost
- 2026-10-27 TS parser keeps every import specifier; tsconfig `paths` are stored as prefix->folder and resolved at query time; packages like `react` are ignored
- 2026-10-27 license: Apache-2.0 (Graphify is Apache-2.0 + MIT; no Graphify code ported yet, credit kept in README)
- 2026-10-27 approved dependencies: tree-sitter(+rust,+typescript), rayon, ignore, clap, serde, serde_json, anyhow, blake3, postcard, tempfile (dev); anything else needs asking
- 2026-10-27 user rule: don't mention problems that a later phase fixes anyway; stop for OK between phases; run the code-reviewer agent before every commit

- 2026-10-27 communities group FILES, not symbols (name matching makes `new`/`clone` glue everything); edge = resolved import + call to a name defined in exactly one file
- 2026-10-27 hand-written deterministic Louvain (no dependency); skipped Graphify's oversized-community split: no fixture needs it yet, add it if a real repo shows one giant community
- 2026-10-27 `report` and `benchmark` are separate commands: `index`/`update`/`watch` stay at ~20 ms
- 2026-10-27 `benchmark` shells out to `graphify extract` (temp dir, removed after) and times both tools as processes; needs an existing index

## Gotchas
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
  - CLAUDE.md gained "No Co-Authored-By or other AI attribution" and was swept into the Phase 2 commit (652302a) by `git add -A`; confirm that was intended
  - unchecked `[]` lookups (`pos[..]`, `community_of[..]`, `index.files[..]`) in src/report.rs and src/community.rs: safe today, switch to `.get()` if the invariants change
  - untested: `benchmark` with Graphify installed, `benchmark --json`, multi-level Louvain with weighted edges
  - `benchmark` explain answers read the saved index, which can be older than the cold build it times
  - tokio communities are lumpy (biggest 179 files, name-match noise like `new_v4`): Phase 5 fixes the cause; Graphify's oversized-community split was skipped
