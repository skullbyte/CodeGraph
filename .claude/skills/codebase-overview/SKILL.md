---
name: codebase-overview
description: Architecture, folder map and conventions of codegraph. Use before working on unfamiliar parts of the code or planning a change across modules.
---
## Architecture
Planned: parse Rust/TS files with tree-sitter into symbols, imports and call sites, write `.claude/codemap.md` + `graph.json`, then answer `def`/`callers`/`path` queries. Today only a `main()` stub exists in `src/main.rs`.
## Folder map
- `src/main.rs` — entry point (stub)
- `docs/REQUIREMENTS.md` — phases 1-3 and CLI spec
## Conventions
- None established yet; follow REQUIREMENTS.md: compact plain-text output, one result per line.
## Where to look
- What to build next / CLI spec → `docs/REQUIREMENTS.md`
## More detail
Read `docs/REQUIREMENTS.md` only if needed.
