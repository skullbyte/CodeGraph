# codegraph
Fast offline code-graph CLI in Rust: lets AI agents ask "where is X / who calls X" with tiny outputs. Requirements: `docs/REQUIREMENTS.md`.

## Commands
- Check all (run before saying you're done): `just check`
- Test: `just test` · one test: `cargo test <name>`
- Lint: `just lint` · Format: `just fmt`
- Run: `cargo run -- <args>` · Build: `cargo build`
- Without just: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt`

## Layout
- `src/` — Rust source (entry: `src/main.rs`, still a stub)
- `docs/` — `REQUIREMENTS.md` (phases, commands, output format)
- Generated, never edit: `target/`, `.claude/codemap.md`

## Code map
- `.claude/codemap.md`: every symbol as `path:line name`, plus `uses:` imports per file; refreshed automatically.
- Find a definition: `rg '<symbol>' .claude/codemap.md`. Find callers: `~/.claude/bin/callers.sh <name>`. Then read only those lines.

## Code rules
- Match the style of surrounding code; small functions, clear names.
- No new dependencies without asking.
- No `unwrap()` outside tests; clippy clean.

## Workflow
- Plan, progress and decisions: `docs/PROGRESS.md` (read it at the start of a task; /handoff updates it).
- Non-trivial change: plan first (plan mode), then small steps.
- New behaviour or bug: write the failing test first, see it fail, then write the code.
- Noisy test output → `test-runner` subagent; a tiny suite can run directly.
- Searches: codemap first, then `explorer`. Before commit: `code-reviewer`.
- Context > 50% on unfinished work: /handoff, then /clear.

## Git
- No Co-Authored-By or other AI attribution in commits or PRs.
- Conventional Commits (`feat:`, `fix:`, …) via /commit.
- Never push, force-push or rewrite history unless asked.
- Never commit secrets, .env or HANDOFF.md.

## Efficiency
- Read files before writing code; don't re-read unchanged files.
- Prefer targeted edits over rewriting whole files.
- Skip files over 100KB unless explicitly needed.
- Concise output, thorough reasoning. No openers or recap fluff.
- Stop when the task is done and tests pass; no unrequested extras.

## Gotchas
- Credit Graphify (safishamsi/graphify) in README and keep its notice for ported logic.
