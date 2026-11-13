---
paths:
  - "src/scip.rs"
  - "src/scope.rs"
  - "src/precision.rs"
---
# SCIP notes
- Indexers: ~/.local/bin (scip-go, scip-java, scip-clang, scip-ruby), npm -g (scip-typescript, scip-python), ~/.dotnet/tools (scip-dotnet, not on PATH). Run them in the repo root.
- After every `cargo install` the binary fingerprint changes and every `.claude/graph.bin` is stale: re-run `codegraph index && codegraph scip` before `codegraph precision`.
- `codegraph precision --misses N` lists N misses per form kind with SCIP's answer: fastest way to find why a language scores low.
- Scoring against Graphify (script not kept): for each Graphify `calls` edge take caller file + `source_location` line + target file/line, look up our call at that line in `.claude/graph.json` (`resolved` after `codegraph scip`); right = same file and def line within 5.
