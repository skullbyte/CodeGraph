# codegraph
Fast offline code-graph CLI in Rust: lets AI agents ask "where is X / who calls X" with tiny outputs. Requirements: `docs/REQUIREMENTS.md`.

## Commands
- Check all (run before saying you're done): `just check`
- Refresh exact call data: `just scip` (run it after code changes, before using the codegraph MCP tools to answer; `just install` first if the code changed)
- Test: `just test` · one test: `cargo test <name>`
- Lint: `just lint` · Format: `just fmt`
- Run: `cargo run -- <args>` · Build: `cargo build`
- Without just: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt`

## Layout
- `src/` — Rust source (entry: `src/main.rs`; one module per command, languages in `src/lang/`). Big test blocks live beside their file as `src/<name>/tests.rs` (`query`, `lang/rust`): read the code file without its tests.
- `docs/` — `REQUIREMENTS.md` (phases, commands, output format)
- `src/html/` — the graph page in small parts. Per-area rules load automatically from `.claude/rules/` (html, typescript, rust, scip) when you touch those files.
- Generated, never edit: `target/`, `.claude/codemap.md`

## Graphify reference (we are porting it to Rust)
- Graphify v0.9.97 (Python) is installed locally, no download needed: `/home/lain/.local/share/uv/tools/graphifyy/lib/python3.12/site-packages/graphify/` (CLI: `~/.local/bin/graphify`). Read-only; never edit it.
- Many files are huge (extract.py 540KB, cli.py 270KB): use the `reader` agent or `rg`, don't read them whole.
- Rule: before building a feature, find how Graphify does it (e.g. web view: `exporters/html.py`, `callflow_html.py`) and port that behaviour, don't invent a different one. Keep its notice on ported logic.
- Rule: keep a checklist in `docs/PROGRESS.md` of every Graphify feature (commands, exports, analysis, web view), compared against our code: mark each done / partial / todo, with the Graphify file it comes from. Update it whenever a feature lands.

## Code map
- `.claude/codemap.md`: every symbol as `path:line name`, plus `uses:` imports per file; refreshed automatically.
- First choice: the codegraph MCP tools (`def`, `callers`, `explain`, `affected`); they answer in a few lines. Fallback: `rg '<symbol>' .claude/codemap.md`. Then read only those lines.

## Code rules
- Match the style of surrounding code; small functions, clear names.
- Follow YAGNI: build only what's asked, the simplest thing that works; never skip input validation, error handling or security.
- No new dependencies without asking.
- No `unwrap()` outside tests; clippy clean.

## Workflow
- Plan, progress and new decisions: `docs/PROGRESS.md` (read it at the start of a task; /handoff updates it). Older dated decisions and measurements: `docs/HISTORY.md` (never read it whole, search it with `rg`). Open work: `docs/TODO.md`.
- Non-trivial change: plan first (plan mode), then small steps.
- New behaviour or bug: write the failing test first, see it fail, then write the code.
- Noisy test output → `test-runner` subagent; a tiny suite can run directly.
- Searches: codegraph MCP tools first, then codemap, then `explorer`. Before every commit: Default model is `opusplan` (Opus plans, Sonnet writes); for a hard task pick `/model opus` before the first message, never mid-session (a switch re-reads the whole context uncached). `code-reviewer` runs on sonnet, given only the diff (`git diff`) plus the touched function names, not whole files. Fix real findings, then run it again on the new diff; stop when it says "no remaining issues" (tell the reviewer to answer with exactly that phrase) or after 3 rounds, whichever comes first, and list anything still open in `docs/TODO.md`. Once per finished feature, not after every small fix.
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
