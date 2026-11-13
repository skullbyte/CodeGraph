---
paths:
  - "src/lang/typescript.rs"
  - "src/lang/typescript/**"
---
# TypeScript extractor notes
- Measure on zod: run `scip-typescript index --no-progress-bar` in `bench/real/zod/packages/zod` (`--pnpm-workspaces` fails), then `codegraph index && codegraph scip && codegraph precision` there. pizzeria (TS) is too small to score.
- zod: `const Test = z.object(..)` makes `Test.parse()` look like a class call (uppercase rule in `call_form`); `export const ZodString: core.$constructor<ZodString>` is both a value and an interface name.
- zod misses: `parse` lives on base `ZodType`, so chains need inheritance (E1 step 4). Out of scope: `Module("util")` matching both v3 and v4 util files; two `ZodType`s (v3/v4) sharing a name.
