# codegraph progress

## Now
Goal: Part A phase 2 (Analysis: community detection + GRAPH_REPORT.md + `codegraph benchmark` vs Graphify).
State: Phase 1 is DONE and committed (8674776, 652302a). Phase 2 is DONE, reviewed and committed (src/community.rs, report.rs, benchmark.rs; CLI `report`, `benchmark`). `just check` green: 76 unit + 13 end-to-end tests.
Next step: first check the "To check" list in Gotchas, then (after the user's OK) plan Phase 3 (visuals: `codegraph html`, Mermaid, Obsidian) in plan mode.

## Checklist
- [x] Phase 1 (Part A.1) core engine, reviewed and committed
  - [x] Rust + TS/TSX extraction: symbols, imports, calls with enclosing fn (incl. macro args, cfg_*! bodies)
  - [x] walker (.gitignore, skip node_modules/target/Library/build/dist), parallel + incremental index
  - [x] outputs: .claude/codemap.md (codemap.sh format), graph.json, graph.bin (cache)
  - [x] commands: index, update, watch, def, callers, explain, path, affected, hubs; `--json`, `--dir`
  - [x] tsconfig `paths` aliases, project lock, atomic writes, binary cache, README (credits Graphify), Apache-2.0 LICENSE
  - [x] benchmarks on tokio + pizzeria (see Gotchas for numbers)
- [x] Phase 2 (A.2) Analysis: file-level Louvain communities, `codegraph report` -> .claude/GRAPH_REPORT.md, `codegraph benchmark` vs Graphify (reviewed, committed)
- [ ] Phase 3 (A.3) Visuals: `codegraph html`, Mermaid callflow export, Obsidian vault export
- [ ] Phase 4 (A.4) More languages: Python, Java, C#, Go, JavaScript, C/C++
- [ ] Phase 5 (A.5) Precision: SCIP import (rust-analyzer, scip-typescript); fixes name-based matching, e.g. `Some(x)` vs real calls and same-named methods
- [ ] Phase 6 (A.6) Claude Code integration: MCP server, skill, hook; replace codemap.sh/callers.sh only if benchmarks win
- [ ] Phase 7 (A.7) Documents: semantic pass over docs/PDFs/markdown via a skill
- [ ] Part B: full Graphify parity (order chosen by the user later)
- [ ] Open: update on tokio is ~19-21 ms (target < 20 ms); `docs/REQUIREMENTS.md` Part A/B is the source of truth for scope

## Decisions
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
