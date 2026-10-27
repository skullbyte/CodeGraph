# codegraph: requirements

A fast, precise, offline code-graph tool in Rust: a better Graphify (safishamsi/graphify,
Apache-2.0 + MIT; credit it in README and keep its notice for any ported logic).
Long-term goal: full feature parity with Graphify, better where we measured it falling short
(precision, ambiguous-name UX, speed, token cost). Build what the user needs first (Part A),
then the rest (Part B).
Purpose: let AI coding agents answer "where is X / who calls X / how does A reach B" with tiny,
exact outputs (fewer tokens), plus an interactive HTML graph to show off.

## Who builds it
Claude writes the code; the user is learning Rust. Explain each module and key Rust concept
briefly as you go (one short sentence per new term).

## Part A: what we need first (in order; stop for the user's OK between phases)
1. Core engine (MVP CLI)
   - tree-sitter parsing for Rust and TypeScript/TSX first; adding a language = grammar crate + a
     table of which nodes are symbols / calls / imports.
   - Extract symbols (fn, struct, enum, trait, impl, class, interface, type, exported consts),
     imports, and call sites with their enclosing function.
   - Parallel parsing (rayon); incremental: skip files whose hash didn't change (cache in .claude/).
   - Respect .gitignore; skip node_modules, target, Library, build, dist.
   - Commands:
     `codegraph index [dir]`    → .claude/codemap.md (format below) + .claude/graph.json
     `codegraph update <file>`  → re-index one file (used by the format hook)
     `codegraph watch [dir]`    → re-index on file changes
     `codegraph def <name>`     → `path:line signature`
     `codegraph callers <name>` → `path:line  in <enclosing fn>`
     `codegraph explain <name>` → definition + incoming/outgoing connections
     `codegraph path <A> <B>`   → shortest call/import chain
     `codegraph affected <name>`→ everything that depends on it (reverse traversal)
     `codegraph hubs`           → most-connected symbols (Graphify's god-nodes)
   - Output: compact plain text, one result per line, no decoration; `--json` optional.
   - Ambiguous names: print all candidates with paths in ONE response (never force a retry with internal IDs).
2. Analysis: community detection (clusters of related code) + GRAPH_REPORT.md (key modules,
   hubs, surprising links); `codegraph benchmark` compares against Graphify automatically.
3. Visuals: `codegraph html` → one self-contained interactive graph (search, zoom, filter by
   folder/kind/community, click a node for file:line and connections); callflow diagram export
   (Mermaid); Obsidian vault export.
4. More languages: Python, Java, C#, Go, JavaScript, C/C++ (the user's stack), then others on request.
5. Precision: SCIP import (`rust-analyzer scip .`, `scip-typescript index`) so definitions and
   callers are compiler-exact, especially Rust method calls (Graphify found 0 of 32).
6. Claude Code integration: MCP server (graph queries as tools), a skill and a hook nudging
   searches to the graph; outputs designed for minimum tokens. Then replace
   ~/.claude/bin/codemap.sh and callers.sh, only if the benchmarks show codegraph wins.
7. Documents: semantic pass over docs/PDFs/markdown linking them to code, run through a
   Claude Code skill (uses the subscription, no API key), like Graphify's.

## Part B: full Graphify parity (after Part A, in the user's chosen order)
- Remaining languages (Graphify supports ~40).
- Images and audio/video in the semantic pass; Google Workspace files.
- LLM-named communities (`label`), `reflect`, `save-result`, `prs`, `tree`, `diagnose`.
- Exports: wiki, svg, graphml, Neo4j, FalkorDB.
- Multi-repo: merge-graphs, global graph registry, git merge driver for graph.json.
- Install/uninstall for other assistants (Codex, Cursor, Gemini, Copilot, …).
Before starting Part B, re-run `graphify --help` to refresh this list against its latest version.

## Output formats to stay compatible with
codemap.md (current, produced by ~/.claude/bin/codemap.sh):
    ## src/lib.rs (60 lines)
    uses: game_core, tiles
    src/lib.rs:11 pub fn parse(input: &str) -> Option<Temp>
callers (current ~/.claude/bin/callers.sh):
    src/components/CartDrawer.tsx:205  in export function CartDrawer

## Benchmarks (clone test repos into ./bench/, which is git-ignored)
- tokio (git clone --depth 1 https://github.com/tokio-rs/tokio, ~814 .rs files)
- a copy of ~/Documentos/GITHUB/pizzeria-emiliano (83 TS files; copy, never modify the original)
Baselines measured on 2026-10-27:
| test | Graphify 0.9.97 | codemap.sh / callers.sh | target |
| full index tokio | 6.4–7.0 s | 6.3 s | < 1 s |
| update 1 file | – | 19 ms | < 20 ms |
| callers of notify_waiters (tokio) | 0 of 32 | 32 (name match, no type info) | 32, exact receivers with SCIP |
| callers of formatPrice (pizzeria) | 7/7 functions | 12 sites in 6 files | same, exact |
| def TcpListener | correct, often "ambiguous" | tokio/src/net/tcp/listener.rs:57 | same, one step |
Graphify is installed (`graphify`, via uv) for side-by-side runs: `GRAPHIFY_BACKEND=none graphify extract <dir> --code-only --force`.

## Constraints
- User is on Claude Pro: keep sessions lean; use the explorer/reader/test-runner subagents; plan mode before each phase.
- Test-first: write failing tests, then code. `just check` (fmt, clippy -D warnings, tests) must pass.
- Ask before adding any dependency; check its latest version with `cargo info <crate>`, never from memory.
- No unwrap/expect outside tests; errors with anyhow/thiserror (ask first).
- Never install system packages or touch other repos without asking.
