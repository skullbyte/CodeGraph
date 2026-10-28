# codegraph

Fast, offline code-graph CLI written in Rust. It answers "where is X", "who calls X" and
"how does A reach B" with tiny, exact, one-line-per-result output, so AI coding agents spend
fewer tokens. Rust, TypeScript/TSX, JavaScript, Python, Java, C#, Go and C/C++ are supported (via tree-sitter). C# and Go files link through calls only: their imports name namespaces and packages, not files.

## Usage
```
codegraph index [dir]        # writes .claude/codemap.md and .claude/graph.json (incremental)
codegraph def <name>         # path:line signature
codegraph callers <name>     # path:line  in <enclosing fn>
codegraph path <A> <B>       # shortest call chain (function names) or import chain (file paths)
codegraph update <file>      # re-index one file
codegraph explain <name>     # definition + callers + callees
codegraph affected <name>    # everything that depends on it (--depth N to limit)
codegraph hubs               # most-connected symbols (--top N, default 10)
codegraph report             # writes .claude/GRAPH_REPORT.md: key modules, hubs, surprising links, communities
codegraph html               # writes .claude/graph.html: offline interactive map of the files
codegraph mermaid <name>     # Mermaid flowchart of what <name> calls (--depth N, default 2)
codegraph obsidian           # writes an Obsidian vault to .claude/obsidian (--out DIR); notes of deleted files are not removed
codegraph scip [index.scip]  # makes callers/hubs/communities exact from a SCIP index (see below)
codegraph benchmark          # vs Graphify (if installed): index time + explain answers (--name X, repeatable)
codegraph watch [dir]        # index, then keep the index fresh as files change (--interval-ms)
```
Add `--json` for JSON output and `--dir <root>` to point at another project.

## Exact calls with SCIP (optional)
By default `callers` matches calls by name. A SCIP index, written by the language's own compiler
tooling, says which function each call really goes to, so calls into the standard library
(`new`, `clone`, ...) stop showing up as project edges:

    rust-analyzer scip .             # Rust: writes index.scip
    scip-typescript index            # TypeScript/JavaScript: writes index.scip
    codegraph index && codegraph scip

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
