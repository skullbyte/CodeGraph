---
paths:
  - "src/html/**"
  - "src/html.rs"
---
# Graph page rules
- The page is built from small parts joined at build time by `src/html.rs`: `template.html` (skeleton), `style.css`, `js/01-data.js` … `js/10-loop.js` (run in that order). Edit only the part you need; never re-read all of it.
- Hard rules: `rg -c 'https?://|innerHTML|<img|url\(' src/html/` must print nothing; one `/*DATA*/` (in `js/01-data.js`, as `const DATA = /*DATA*/;`), `{{TITLE}}` only in `template.html` (Rust tests check both).
- Port Graphify's web view (`exporters/html.py`, 2D vis-network from a CDN, breaks offline). Ours stays offline and self-contained (no CDN); any 3D look is our own extra.
- Design skills (ui-ux-pro-max, taste-skill, impeccable): page stays offline and single-file. No images at all, no CDN, no web fonts, no telemetry, no hooks.
- Impeccable files (`.claude/skills/impeccable/`, `.claude/agents/impeccable-*.md`, `.impeccable/`) are gitignored: commit by file name, never `git add -A`.
- Agent cost: a design pass ~160-215k tokens, a review 20-85k: keep jobs narrow.
- Brave tests (the claude-in-chrome tools drive Brave): regenerate with `cargo build --release && ./target/release/codegraph index --dir bench/pizzeria && ./target/release/codegraph html --dir bench/pizzeria`; serve `bench/pizzeria/.claude` with `exec python3 -m http.server <port> --bind 127.0.0.1` as a background task; stop it by PID from `ss -ltnp` (`pkill -f` kills the shell, exit 144).
