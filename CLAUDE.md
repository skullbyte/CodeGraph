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
- `src/html/` — the graph page, in small parts joined at build time by `src/html.rs`: `template.html` (skeleton), `style.css`, `js/01-data.js` … `js/10-loop.js` (run in that order). Edit only the part you need; never re-read all of it. Hard rules for these files: `rg -c 'https?://|innerHTML|<img|url\(' src/html/` must print nothing; one `/*DATA*/` (in `js/01-data.js`), `{{TITLE}}` only in `template.html`.
- Generated, never edit: `target/`, `.claude/codemap.md`

## Graphify reference (we are porting it to Rust)
- Graphify v0.9.97 (Python) is installed locally, no download needed: `/home/lain/.local/share/uv/tools/graphifyy/lib/python3.12/site-packages/graphify/` (CLI: `~/.local/bin/graphify`). Read-only; never edit it.
- Many files are huge (extract.py 540KB, cli.py 270KB): use the `reader` agent or `rg`, don't read them whole.
- Rule: before building a feature, find how Graphify does it (e.g. web view: `exporters/html.py`, `callflow_html.py`) and port that behaviour, don't invent a different one. Keep its notice on ported logic.
- Rule: keep a checklist in `docs/PROGRESS.md` of every Graphify feature (commands, exports, analysis, web view), compared against our code: mark each done / partial / todo, with the Graphify file it comes from. Update it whenever a feature lands.
- Port the web version too (`exporters/html.py`). Graphify's is 2D (vis-network 9.1.6, loaded from a CDN, so it breaks offline); ours must stay offline and self-contained (no CDN). Any 3D look is our own extra, not a Graphify feature.

## Code map
- `.claude/codemap.md`: every symbol as `path:line name`, plus `uses:` imports per file; refreshed automatically.
- Find a definition: `rg '<symbol>' .claude/codemap.md`. Find callers: `~/.claude/bin/callers.sh <name>`. Then read only those lines.

## Code rules
- Match the style of surrounding code; small functions, clear names.
- Follow YAGNI: build only what's asked, the simplest thing that works; never skip input validation, error handling or security.
- Design skills (ui-ux-pro-max, taste-skill, impeccable): the page stays offline and single-file. No images at all (no stock-photo fetching, no local images), no CDN, no web fonts, no telemetry, no hooks.
- No new dependencies without asking.
- No `unwrap()` outside tests; clippy clean.

## Workflow
- Plan, progress and decisions: `docs/PROGRESS.md` (read it at the start of a task; /handoff updates it).
- Non-trivial change: plan first (plan mode), then small steps.
- New behaviour or bug: write the failing test first, see it fail, then write the code.
- Noisy test output → `test-runner` subagent; a tiny suite can run directly.
- Searches: codemap first, then `explorer`. Before commit: `code-reviewer`; run it once per finished feature, not after every small fix.
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
