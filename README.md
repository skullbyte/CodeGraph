# codegraph

Fast, offline code-graph CLI written in Rust. It answers "where is X", "who calls X" and
"how does A reach B" with tiny, exact, one-line-per-result output, so AI coding agents spend
fewer tokens. Rust, TypeScript/TSX, JavaScript, Python, Java, Kotlin, PHP, Ruby, Swift, C#, Go and C/C++ are supported (via tree-sitter). C# and Go files link through calls only: their imports name namespaces and packages, not files.

## Usage
```
codegraph index [dir]        # writes .claude/codemap.md and .claude/graph.json (incremental); `update` after an edit only logs the change (delta.bin) and patches codemap.md, so graph.json lags until the next full index
codegraph query "<question>" # plain English: matching symbols + what they call / are called by (--depth, --budget tokens, --dfs)
codegraph tour               # reading order: entry files, then the key file of each cluster (--top N)
codegraph def <name>         # path:line signature (--teach adds what the keywords mean + the doc comment)
codegraph callers <name>     # path:line  in <enclosing fn>, -> def when known (--def PATH[:LINE]: one definition only), with -> def when known (--def PATH[:LINE]: one definition only)
codegraph path <A> <B>       # shortest call chain (function names) or import chain (file paths)
codegraph update <file>      # re-index one file
codegraph explain <name>     # definition + callers + callees (--teach: plain-language lines + what to read next)
codegraph affected <name>    # everything that depends on it (--depth N to limit)
codegraph hubs               # most-connected symbols (--top N, default 10)
codegraph report             # writes .claude/GRAPH_REPORT.md: key modules, hubs, surprising links, communities
codegraph html               # writes .claude/graph.html: offline interactive map of the files
codegraph mermaid <name>     # Mermaid flowchart of what <name> calls (--depth N, default 2)
codegraph obsidian           # writes an Obsidian vault to .claude/obsidian (--out DIR); notes of deleted files are not removed
codegraph scip [index.scip]  # makes callers/hubs/communities exact from a SCIP index (see below)
codegraph precision          # how exact are call links? scored against SCIP (run `just scip` first)
codegraph benchmark          # vs Graphify (if installed): index time + explain answers (--name X, repeatable)
codegraph docs pending       # docs Claude has not read yet (new or changed), with the plan (--max-lines N)
codegraph docs ingest <json> # validate and store what an agent found in one doc
codegraph docs text <doc>    # text of a doc; PDFs are read locally (cheap in tokens)
codegraph watch [dir]        # index, then keep the index fresh as files change (--interval-ms)
```
Add `--json` for JSON output and `--dir <root>` to point at another project.

## Use it from Claude Code (fewer tokens)
Claude only saves tokens if it asks the graph instead of grepping and reading files. One project at a time:

    cargo build --release && install -Dm755 target/release/codegraph ~/.local/bin/codegraph   # once; any dir on your PATH
    codegraph install            # in the project: hooks, MCP server, skill (--print shows them first)
    codegraph uninstall          # removes only what install added

`install` edits `.claude/settings.json` (keys end up sorted; your other settings and hooks stay), `.mcp.json`
and `.claude/skills/codegraph/SKILL.md`; it never touches `~/.claude` and refuses a settings file that is not
valid JSON. Restart Claude Code and approve the `codegraph` server when asked.

- **MCP server** (`codegraph serve`, offline, no network): tools `query`, `tour`, `def`, `callers`, `explain`, `path`, `affected`,
  `hubs`, `community`, `stats`. All ten descriptions together cost about 600 tokens per session; answers
  are the CLI lines, cut at 40 lines (`limit`) with `... +N more`. It reloads when the graph changes and builds
  an index by itself if there is none.
- **Hook** (`codegraph hook search|read`): when Claude is about to grep, or read a source file, in an indexed
  project it adds one sentence saying the tools exist. At most 3 times per agent per session
  (`CODEGRAPH_HOOK_NUDGES=0` turns it off) and it never blocks a tool call.
- `install` also swaps an old `codemap.sh` session-start line for `codegraph index`.

## Documents (optional, spends Claude tokens)
Markdown docs are indexed for free on every `codegraph index`: headings, links between docs and
`code` mentions (shown as `[doc]` in `callers`). For more, ask Claude to "index the docs": the
`codegraph-docs` skill reads only the docs that are new or changed, stores the concepts and the
code they explain in `.claude/semantic.json` (cached by file hash, kept when the binary is rebuilt),
and `def`, `callers` and `explain` then show `[concept]` and `[doc]` lines. Small jobs run in the main
conversation, big ones in Sonnet subagents of about 6000 lines. Files named like secrets are never offered.

## Exact calls with SCIP (optional)
By default `callers` matches calls by name. A SCIP index, written by the language's own compiler
tooling, says which function each call really goes to, so calls into the standard library
(`new`, `clone`, ...) stop showing up as project edges:

    codegraph scip --run             # starts the installed indexer for this project, applies it

`--run` picks `rust-analyzer` (Cargo.toml), `scip-typescript` (tsconfig.json) and `scip-python`
(pyproject.toml, setup.py or setup.cfg) when they are on PATH, keeps their output in
`.claude/scip/` and applies it. Indexers run the project's own build (build scripts, plugins), so
codegraph never starts one by itself. Later `codegraph index` runs bring the answers back by
themselves for every file that was re-parsed and not edited since the indexer ran. Add `.claude/scip/` to your .gitignore (the files are big). There is no time limit: interrupt a stuck indexer with Ctrl-C. By hand:
`rust-analyzer scip . && codegraph scip` (reads `index.scip`).

When one name has several project definitions, `callers` adds `-> path:line` to say which one,
and `hubs` lists each definition on its own. `codegraph scip` also says how many files the index
did not cover (code behind cfg flags the indexer skipped); their calls keep name matching.
Re-parsing a changed file forgets its answers until `codegraph scip` runs again, and the
`-> path:line` of a definition is where it was when the index was made: re-run `codegraph scip`
after editing.

## License
Apache License 2.0; see [LICENSE](LICENSE).

## Credit
Inspired by [Graphify](https://github.com/safishamsi/graphify) (safishamsi/graphify,
Apache-2.0 + MIT). codegraph is an independent Rust implementation; any logic ported from
Graphify keeps its original notice. Ported so far: the "surprising links" scoring, the cohesion
formula and the chars/4 token estimate (`src/report.rs`, `src/community.rs`, `src/benchmark.rs`).
