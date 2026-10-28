---
name: codegraph
description: Answer "where is X defined", "who calls X" and "what depends on X" from the code graph instead of grepping. Use it before grep or reading files in an indexed project.
---
<!-- codegraph-managed: `codegraph uninstall` removes this file -->
# codegraph

This project is indexed. The `codegraph` MCP tools (or the same commands on the CLI) answer in a few lines what grep and file reads answer in many:

- `def <name>`: where a symbol is defined (`path:line signature`).
- `callers <name>`: call sites, `path:line  in <calling function>`.
- `explain <name>`: definition, callers and callees together.
- `affected <name>`: everything that depends on it (callers of callers).
- `path <A> <B>`: shortest call chain between two functions, or import chain between two files.
- `hubs`, `community <file>`, `stats`: the shape of the project.

Notes:
- When a name has several definitions, each hit ends with `-> path:line`; use that to tell them apart.
- Calls are matched by name until `codegraph scip` has imported a compiler index (`rust-analyzer scip .`, `scip-typescript index`); generic names such as `new` are noisy before that.
- Grep is still the right tool for text in strings, comments and config files.
- Out of date after edits? `codegraph update <file>` or `codegraph index`.
