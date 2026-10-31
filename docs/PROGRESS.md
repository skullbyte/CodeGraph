# codegraph progress

## Now
Goal: Phase 7 (documents in the graph, token-cheap at any size). Plan: ~/.claude/plans/foamy-enchanting-turing.md (approved 2026-10-31).
State: Phases 1-6 done and installed here (binary in ~/.local/bin, `.mcp.json`, hooks, skill; SCIP imported, `just scip` refreshes it). Phase 7 step 1 (free markdown scan) committed as 5f8c698 after 4 review rounds. `just check` green: 202 unit + 25 end-to-end tests. Tree clean; nothing pushed (the user pushes).
Step 2 (uncommitted, reviewed in 3 rounds, last answer "no remaining issues"; `just check` green: 217 unit + 26 e2e): `src/semantic.rs` (store, `is_sensitive`, `pending`, `ingest` with validation, `def_with`/`callers_with`/`explain_with`), `walk::list_docs`, `codegraph docs pending|ingest`; `def`/`callers`/`explain` (CLI and MCP) show `[concept]` and `[doc]` lines from the store, AMBIGUOUS hidden.
Next step: step 3 (skill `codegraph-docs`, installed by `install`), then step 5 (numbers on real repos, commit by file name). Step 4 (PDFs) needs the user's OK before any new crate.

## Checklist
- [x] Phase 1 (A.1) core engine: Rust + TS/TSX extraction, walker, parallel incremental index, codemap/graph.json/graph.bin, index/update/watch/def/callers/explain/path/affected/hubs, tsconfig aliases, benchmarks (done 2026-10-27)
- [x] Phase 2 (A.2) Analysis: file-level Louvain communities, `codegraph report` -> .claude/GRAPH_REPORT.md, `codegraph benchmark` vs Graphify (reviewed, committed)
- [x] Phase 3 (A.3) Visuals: `codegraph html` (offline 3D view + Graphify web features), `mermaid`, `obsidian`; reviewed, committed (done 2026-10-30)
- [x] Phase 4 (A.4) More languages: Rust, TS/TSX, JS, Python, Java, C#, Go, C/C++ (plan in ~/.claude/plans/glowing-skipping-mountain.md)
- [~] Phase 5 (A.5) Precision (committed; checked on real indexes: this repo, pizzeria and tokio, numbers in Decisions): SCIP import (rust-analyzer, scip-typescript); fixes name-based matching, e.g. `Some(x)` vs real calls and same-named methods
- [x] Phase 6 (A.6) Claude Code integration: MCP server (`serve`), hook, skill, `install`/`uninstall`; installed here 2026-10-31 (binary symlinked to ~/.local/bin), MCP tools `def`/`stats` verified live after restart; hook nudge observed; `install` replaced the codemap.sh SessionStart hook, re-added by hand (re-check after any reinstall); SCIP imported here (6029 of 6212 calls), refresh with `just scip`
- [~] Phase 7 (A.7) Documents (plan: ~/.claude/plans/foamy-enchanting-turing.md; token-cheap at any size)
  - [x] step 1: free markdown scan (`src/lang/markdown.rs`: headings, doc links, `code` mentions; `Kind::Heading`; docs never count as code: `graph_calls()` empty for docs, `Kind::is_code_def`, `callers` lists `[doc]` mentions only for names with one code definition); reviewed (4 rounds, "no remaining issues"), committed
  - [x] step 2: `codegraph docs pending|ingest` + `.claude/semantic.json` store (AI pass, spends Claude tokens, cached by hash) (store, pending, ingest, queries read the store; reviewed)
  - [ ] step 3: skill `codegraph-docs`; [ ] step 4: PDFs (ask before adding a crate); [ ] step 5: review loop, numbers, commit
- [ ] Part B: full Graphify parity (order chosen by the user later)
- [ ] Open: update on tokio is ~19-21 ms (target < 20 ms); `docs/REQUIREMENTS.md` Part A/B is the source of truth for scope

## Graphify parity checklist
Compared on 2026-10-28 against Graphify 0.9.97 (source: see CLAUDE.md). `G/` = graphify package dir. Mark: [x] done, [~] partial, [ ] todo. Update when a feature lands.

### Parity score (2026-10-30; item = 1, partial = 0.5; Graphify 0.9.97 `--help`, `exporters/`, `extractors/`, `extract.py`)
| Area | Done / total | % |
|---|---|---|
| Commands (install, path, explain, diagnose, clone, merge-driver, merge-graphs, add, watch, update, cluster-only, label, query, affected, god-nodes, prs, save-result, reflect, check-update, tree, extract, provider, global, benchmark, hook, serve) | 8 / 26 (done: path, explain, watch, update, cluster-only, affected, god-nodes; partial: extract = AST only, benchmark) | 31% |
| Exports (json, html, obsidian, canvas, graphml, svg, cypher/falkordb, wiki, callflow html, tree html) | 3 / 10 (json, html; partial: obsidian, callflow) | 30% |
| Analysis (communities, god nodes, surprising links, cohesion, suggested questions, dedup, resolution passes, incremental cache, rcfile, query log, security caps, validate) | 5 / 12 | 42% |
| Web view features | 11 / 16 | 69% |
| Languages (Graphify has ~45 extractors) | 9 / 45 (Rust, TS/TSX, JS, Python, Java, C#, Go, C, C++) | 20% |
| Ingest / semantic (docs, PDF, images, audio, Google, Postgres, manifest, URL, LLM pass) | 0 / 9 | 0% |
| **Total** | **36 / 118** | **~31%** |

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
- [ ] query "<question>" (BFS/DFS with --depth/--budget)
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
- 2026-10-28 the page source is split into `src/html/style.css`, `src/html/js/01..10-*.js` and a skeleton `template.html`, joined by `assemble()` in `src/html.rs`; output proved byte-identical to the single file. Reason: agents re-read the whole 1700-line file every step (~200k tokens per pass). User rule: one `code-reviewer` run per finished feature
- 2026-10-28 page design: done by agents on Sonnet (user rule: no Opus), using the pizzeria skills (frontend-design, apple-design, animate, visual-qa) plus ui-ux-pro-max, taste-skill and impeccable (installed without hook, telemetry off, gitignored). The page must stay offline, single-file, image-free; the template line `const DATA = /*DATA*/;` and `{{TITLE}}` are fixed by Rust tests; the SVG namespace is built from two strings to keep the no-`http://` test
- 2026-10-28 Graphify's web view is 2D (vis-network via CDN), so "port the 3D web" was a wrong assumption; ours stays offline. User chose: match Graphify's web features, plus our own offline 3D (plain canvas, no libraries)
- 2026-10-28 user rule: test pages in Brave. My browser tools ARE connected to Brave (`await navigator.brave.isBrave()` is true), so test there directly (earlier note saying they drive Chrome was wrong)
- 2026-10-29 git identity is `skullbyte <278924712+skullbyte@users.noreply.github.com>` (global + this repo); history of codegraph and 4 other solo repos rewritten (author/committer only): the user hides the Proton email, their name is fine
- 2026-10-29 `origin` is github.com/skullbyte/CodeGraph; never push/force-push unless asked (the user pushed themselves); `DI-UD1-GRUPO3` (classmate's commits) left alone on purpose
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

- 2026-10-31 hooks written by `codegraph install` are guarded (`command -v codegraph >/dev/null 2>&1 || exit 0; codegraph hook ...`, exit code of the hook itself passes through): a fresh clone or CI without the binary must not fail every tool call; the old plain commands stay recognised so reinstall upgrades and uninstall removes them
- 2026-10-31 `install` rewrites the old `codemap.sh` SessionStart line into the guarded `codegraph index` (first match only; nothing is rewritten once a guarded line exists), so a hand-kept codemap.sh line survives reinstalls
- 2026-10-31 `just scip` (rust-analyzer scip . + codegraph scip) refreshes exact call data after code changes; `index.scip` is git-ignored; CLAUDE.md rule: review loop = repeat `code-reviewer` (Sonnet) until it answers "no remaining issues"
- 2026-10-31 global `~/.claude/CLAUDE.md` lists the installed modern CLI tools (rg fd eza bat sd jq duf tldr fzf just dust tokei delta difft yq gh ast-grep hyperfine procs btm xh) for every session, repo or not
- 2026-10-31 Phase 7 design (user chose both layers, token efficiency at any size): free markdown scan on every index; the AI semantic pass (spends Claude tokens, never call it "paid") is opt-in, cached per file hash in its own store so a rebuilt binary does not redo it; small jobs inline, large ones in ~6000-line chunks to Sonnet subagents
- 2026-10-31 docs are not code: a heading is not a definition (`Kind::is_code_def`), doc mentions are not calls (`FileEntry::graph_calls` is empty for docs; `Index::code_totals` for counts), `callers` appends `[doc]` mentions only for names with exactly one code definition; scanner skips fences/front matter, `#hashtag` is not a heading; left alone on purpose: nested parentheses in link targets, indented code blocks, percent-encoded links

## Gotchas
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

- 2026-10-30 Phase 4: one table-driven walker (`src/lang/generic.rs`, idea from Graphify's `LanguageConfig`) plus a small module per language; JS reuses the TSX grammar; C and C++ share one table and `.h` is tried as C then C++; C#/Go record no imports (namespace/package, not files); after Phase 4 tokio cached index 26 ms, update 20 ms (unchanged), release binary 20 MB
- 2026-10-30 Obsidian vault verified in real Obsidian 1.14.5 (via its debug port, `app.metadataCache`): 107 notes, 768 links resolved, 0 unresolved. Java `this(..)`/`super(..)`/`A::f` and C# `global::Foo` are calls now.
- 2026-10-30 Phase 4 gaps closed: C# destructors/operators are symbols and property/indexer accessors are callers (`Config.scopes`); `Config.name` returns several names (multi-name typedefs); `.h` uses `Lang::Header` (C grammar, C++ when C has errors). Real-code check (copies in git-ignored `bench/real/`): Go `net` 435 files, glm 291 C++ files, Graphify's Python package 97 files, a Spring app 6 Java files all index; names like `Close`/`len` top the hubs (name matching, Phase 5)
- 2026-10-30 Phase 5 design: hand-written SCIP protobuf reader (`src/scip.rs`, no crate; field numbers from sourcegraph/scip scip.proto); resolutions live per file in `FileEntry.resolved` (call position -> Def{path,line} | External), not on `Call`, because postcard (graph.bin) cannot skip fields and graph.json would grow ~16 bytes per call; `External` calls are dropped from callers/explain/affected/hubs/path/mermaid edges and from community links; Graphify's `scip_ingest.py` is only a skeleton for a simplified JSON shape (not wired), so this is an extra, not a parity item
- 2026-10-30 Phase 5 real-world numbers (release): codegraph itself, rust-analyzer 1.99.1 `scip` 3.4 s, import resolved 3957 of 4256 calls (3111 external), `callers new` 65 -> 5 hits, `new` gone from the top hubs; pizzeria (scip-typescript, no node_modules), index 0.7 MB, import 4 ms, 438 of 824 calls resolved (290 external), `callers formatPrice` stays 12 sites / 6 files (exact), 9 index documents match no indexed file (not our languages/skipped)
- 2026-10-30 Phase 5 tokio (release): `rust-analyzer scip` 28 s, index 21.8 MB, `codegraph scip` import 63 ms, 43868 of 53600 calls resolved (22630 external); `callers notify_waiters` 33 hits (same count, now each shows `-> path:line`; one is a bench-local fn of the same name); communities 91 -> 42 but the biggest went 179 -> 188 files (cohesion 0.03): resolved calls add real links, so the giant cluster is Louvain's resolution limit, not name noise. `hubs`: `new` still tops (1374 -> 1123 in-calls) because node identity is still the bare name; `drop` fell out of the top 5. Next fix for the giant community: Graphify's oversized-community split (skipped in Phase 2 until a real repo showed one; this is that repo)
- 2026-10-30 Phase 5 follow-up (tokio, release, after the fixes): (1) `last_name` now reads backticks inside `[..]`/`(..)` descriptors (``slice/impl#[`[T]`]len().``): resolved 43,868 -> 44,396 of 53,600; (2) `codegraph scip` reports files the index does not cover (tokio: 101 files, 4,493 calls behind cfg flags such as loom/tokio_uring; they keep name matching) and clears old answers of files a re-run no longer covers; (3) Broken pipe: a panic hook exits 0 quietly (`codegraph callers x | head`); (4) communities: ported Graphify's two split rules (group > max(10, n/4) files, or >= 50 files with cohesion < 0.05, re-clustered with Louvain on its own links): tokio 42 -> 74 communities, biggest 188 -> 51 files; (5) `hubs` are per definition once SCIP data exists and a name has >= 2 definitions (unresolved calls to such names are skipped as unattributable); without SCIP data it is unchanged. tokio top hubs are now single definitions (`spawn` tokio-test in=287, `spawn` tokio/task in=224, `block_on` in=172) instead of one merged `new`. `explain`/`affected`/`path` are still name-level. `callers notify_waiters` stays 33 by name: only 18 resolve to `Notify::notify_waiters` (notify.rs:740), 5 to a bench-local fn, 4 to other same-named fns, 6 are in `loom_notify.rs` (not indexed); the old "32 exact" target was a name-match count
- 2026-10-30 scale check (release, copy of Go's stdlib in git-ignored `bench/real/gostd`, 8,167 files, 157 MB): cold index 1.09 s, cached index 254 ms, `def`/`callers` 81-83 ms, `hubs` 440 ms, peak memory 289 MB, graph.bin 27 MB, graph.json 56 MB, codemap.md 10 MB. Not tried: 50k+ files. User's goals (2026-10-30): faster, more accurate, OFFLINE, saves tokens, any project size; parity percentage is secondary. Token saving needs Claude Code to actually call the graph: Phase 6
- 2026-10-30 Phase 6 design: hand-written MCP (JSON-RPC over stdio, no crate) with 8 tools named like our commands (Graphify's are `query_graph`, `get_node`, `get_neighbors`, `get_community`, `god_nodes`, `graph_stats`, `shortest_path`; ours map to def/callers/explain/community/hubs/stats/path); hook after Graphify's `hook-guard` (facts not commands, per-agent budget because nudging every call cost more tokens than the queries; budget 3, Graphify 5); `--strict` and the PR tools skipped; project scope only; installer keeps unrelated settings but rewrites the file with sorted keys (no `preserve_order`/indexmap, user chose "the better option")
- 2026-10-30 Phase 6 numbers (release, tokio): `callers notify_waiters` 12 ms vs `callers.sh` 38 ms (33 lines / 2,993 bytes with `-> path:line` vs 32 lines / 1,944 bytes without); full index 34 ms vs `codemap.sh` 6,335 ms; `tools/list` 2,307 bytes (~576 tokens); on the 8k-file Go tree `def` 1 ms, `callers` 4 ms, `stats` 90 ms, first call 1 s (index rebuild after a new binary); `hubs` is cached per index version. `codegraph` is NOT on PATH yet: `install` writes bare `codegraph` commands, so do not run it before the binary is installed (each hook call would fail)
- 2026-10-30 hook security (review): counters live in a private 0700 dir (XDG_RUNTIME_DIR, else temp/codegraph-hook-<user>), append-only files named by a blake3 hash of session+agent ids, symlinks refused
- 2026-10-31 Phase 7 step 1 numbers (release): tokio now 839 files (25 are .md): cold index 77 ms, cached 28 ms (was ~60 / 25); `hubs` unchanged; this repo 46 files; `callers assemble` shows its code call plus `docs/PROGRESS.md:85  in Decisions  [doc]`. Design: a heading is not a definition (would break unique-name checks), doc mentions are not calls (heading text like "Getting started" would become a fake caller), the hook still ignores reads of .md
- 2026-10-31 user rule: never call AI passes "paid"; codegraph is free, the AI layer spends Claude tokens
- 2026-10-31 Phase 7 step 2 design: ingest prunes store entries of docs that vanished or became secret-named; names lose control/bidi/zero-width chars (a newline would fake a result line); links/concepts need a line inside the doc; `docs ingest` reads at most 1 MB; the MCP loads the store only for def/callers/explain and ignores a damaged one (stderr note)
