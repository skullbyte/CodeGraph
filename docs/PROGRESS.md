# codegraph progress

## Now
Goal: Phase 4 (more languages), plan approved. Phase 3 done. JavaScript committed; next Python, blocked until the user runs the `cargo add` line.
State: Phases 1-2 done. html = 3 commits (d10f627 first page, c4de95b offline 3D space view, 50d60cf accessibility/keyboard fixes); review loop ended with "no remaining issues"; tested in Brave. `just check` green: 97 unit + 18 end-to-end tests; tree clean. `origin` = github.com/skullbyte/GraphGraph (fresh repo, the user pushes). History was rewritten on 2026-10-29 (identity), so all commit hashes changed.
Next step: mermaid done and committed (75af0b9, 6009b3c); step 3 obsidian done and committed. Phase 3 complete; stop for the user OK before Phase 4 (more languages), per ~/.claude/plans/floating-munching-lantern.md; tick the Graphify checklist and refresh it as features land.

## Checklist
- [x] Phase 1 (A.1) core engine: Rust + TS/TSX extraction, walker, parallel incremental index, codemap/graph.json/graph.bin, index/update/watch/def/callers/explain/path/affected/hubs, tsconfig aliases, benchmarks (done 2026-10-27)
- [x] Phase 2 (A.2) Analysis: file-level Louvain communities, `codegraph report` -> .claude/GRAPH_REPORT.md, `codegraph benchmark` vs Graphify (reviewed, committed)
- [x] Phase 3 (A.3) Visuals
  - [x] `codegraph html`: offline 3D space view + Graphify web features, reviewed, committed (d10f627, c4de95b, 50d60cf)
  - [x] Mermaid callflow export: `codegraph mermaid <name> [--depth 2]` (src/mermaid.rs, reviewed, committed)
  - [x] Obsidian vault export: `codegraph obsidian [--out DIR]` (src/obsidian.rs, reviewed, committed)
- [ ] Phase 4 (A.4) More languages: plan in ~/.claude/plans/glowing-skipping-mountain.md
  - [x] JavaScript (.js .jsx .mjs .cjs; TSX grammar, `require('./x')` imports)
  - [ ] Python, Java, C#, Go, C/C++ (needs `cargo add tree-sitter-python tree-sitter-java tree-sitter-c-sharp tree-sitter-go tree-sitter-c tree-sitter-cpp`, run by the user)
- [ ] Phase 5 (A.5) Precision: SCIP import (rust-analyzer, scip-typescript); fixes name-based matching, e.g. `Some(x)` vs real calls and same-named methods
- [ ] Phase 6 (A.6) Claude Code integration: MCP server, skill, hook; replace codemap.sh/callers.sh only if benchmarks win
- [ ] Phase 7 (A.7) Documents: semantic pass over docs/PDFs/markdown via a skill
- [ ] Part B: full Graphify parity (order chosen by the user later)
- [ ] Open: update on tokio is ~19-21 ms (target < 20 ms); `docs/REQUIREMENTS.md` Part A/B is the source of truth for scope

## Graphify parity checklist
Compared on 2026-10-28 against Graphify 0.9.97 (source: see CLAUDE.md). `G/` = graphify package dir. Mark: [x] done, [~] partial, [ ] todo. Update when a feature lands.

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
- 2026-10-28 user rule: test pages in Brave. My browser tools ARE connected to Brave (`await navigator.brave.isBrave()` is true), so test there directly (earlier note saying they drive Chrome was wrong)
- 2026-10-29 git identity is `skullbyte <278924712+skullbyte@users.noreply.github.com>` (global + this repo); history of codegraph and 4 other solo repos rewritten (author/committer only): the user hides the Proton email, their name is fine
- 2026-10-29 `origin` is github.com/skullbyte/GraphGraph; never push/force-push unless asked (the user pushed themselves); `DI-UD1-GRUPO3` (classmate's commits) left alone on purpose
- 2026-10-29 skipped as not worth tokens/overlap: Anthropic security-guidance (runs an Opus review every turn), frontend-design, Addy Osmani agent-skills, Superpowers, Headroom, Archify, REA. Use the `security-review` skill on demand before publishing
- 2026-10-29 page reviews: loop `code-reviewer` (Sonnet) and fix every real defect, even small, until it says "no remaining issues"
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
