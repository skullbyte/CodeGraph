---
paths:
  - "src/lang/rust.rs"
  - "src/lang/rust/**"
  - "src/usepath.rs"
---
# Rust extractor notes
- SCIP truth: `rust-analyzer scip .` in the repo root or in `bench/tokio` (27 s), then `codegraph scip`; `index.scip` is git-ignored.
- tokio bench (release, 814 files): cold index ~86 ms without saved SCIP, ~131 ms with; cached ~32 ms; `def TcpListener` -> `tokio/src/net/tcp/listener.rs:57`; `notify_waiters` 33 callers. Run commands with `cd bench/tokio`.
- tree-sitter-rust: a `mut_pattern`'s first named child is the `mut` keyword, find the `identifier` child; `_` in a tuple pattern is a child that must still count for indexes; filter comments and punctuation by kind.
- `resolve_rust_module` fallback was O(imports x files) with string allocs per file; keep them hoisted (119 -> 13 ms on tokio).
