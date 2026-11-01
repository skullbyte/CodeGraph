//! How exact are our call links? Measured against SCIP, the compiler's own answer: for calls to a
//! name with several project definitions, which definition does each strategy pick, and does it
//! match SCIP? Needs `codegraph scip` data (Rust, TypeScript); the table of how many call sites
//! point at one, several or no project definition works for every language. Ours, not in Graphify.

use std::collections::HashMap;

use serde::Serialize;

use crate::model::{Call, FileEntry, Form, Index, Resolution};
use crate::scope::{Def, Resolver, Tier};

/// One way of choosing among the definitions of a name, scored against SCIP.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Score {
    pub name: &'static str,
    /// Exactly one candidate left, and it is SCIP's.
    pub unique_right: usize,
    /// Exactly one candidate left, and SCIP says another.
    pub unique_wrong: usize,
    /// Several candidates left, SCIP's among them, fewer than all definitions.
    pub narrowed: usize,
    /// All definitions still candidates.
    pub ambiguous: usize,
    /// The strategy dropped SCIP's definition, or returned nothing.
    pub lost: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    /// Call sites in code files.
    pub calls: usize,
    /// ... whose callee has exactly one, several, or no project definition.
    pub one: usize,
    pub several: usize,
    pub none: usize,
    /// Calls to a several-definition name that SCIP resolved to one of our extracted definitions:
    /// the calls the strategies are scored on.
    pub truth: usize,
    /// SCIP resolved these to something we do not extract as a symbol (macro-generated code, a
    /// field, a declaration): no strategy could find it, so they are left out of the scores.
    pub unscored: usize,
    pub scores: Vec<Score>,
    /// The `form` strategy again, split by how the call is written (one `Score` per form).
    pub by_form: Vec<Score>,
}

/// Picks candidate definitions (indexes into the name's definition list) for a call made in
/// `file`. More strategies are added next to these as resolution gets smarter.
type Strategy = fn(&Resolver, &FileEntry, &Call, &[Def]) -> Vec<usize>;

const STRATEGIES: [(&str, Strategy); 6] = [
    ("name", name_only),
    ("same-file", same_file),
    ("imported", imported),
    ("same-folder", same_folder),
    ("scope", by_scope),
    ("form", by_form),
];

/// One tier alone: its definitions when it has any, else every definition (it did not apply).
fn tier_or_all(resolver: &Resolver, tier: Tier, file: &FileEntry, defs: &[Def]) -> Vec<usize> {
    let hits = resolver.in_tier(tier, &file.path, defs);
    if hits.is_empty() {
        (0..defs.len()).collect()
    } else {
        hits
    }
}

fn same_file(resolver: &Resolver, file: &FileEntry, _: &Call, defs: &[Def]) -> Vec<usize> {
    tier_or_all(resolver, Tier::SameFile, file, defs)
}

fn imported(resolver: &Resolver, file: &FileEntry, _: &Call, defs: &[Def]) -> Vec<usize> {
    tier_or_all(resolver, Tier::Imported, file, defs)
}

fn same_folder(resolver: &Resolver, file: &FileEntry, _: &Call, defs: &[Def]) -> Vec<usize> {
    tier_or_all(resolver, Tier::SameFolder, file, defs)
}

/// Today's rule: every definition of the name is a candidate.
fn name_only(_: &Resolver, _: &FileEntry, _: &Call, defs: &[Def]) -> Vec<usize> {
    (0..defs.len()).collect()
}

/// How the call is written decides which definitions are eligible (`scope.rs`).
fn by_form(resolver: &Resolver, file: &FileEntry, call: &Call, defs: &[Def]) -> Vec<usize> {
    resolver.resolve(&file.path, call, defs)
}

/// Same file, else an imported file, else the same folder (`scope.rs`).
fn by_scope(resolver: &Resolver, file: &FileEntry, _: &Call, defs: &[Def]) -> Vec<usize> {
    resolver.narrow(&file.path, defs)
}

/// Project definitions by name (`impl` blocks and doc headings do not count).
fn definitions(index: &Index) -> HashMap<&str, Vec<Def>> {
    let mut map: HashMap<&str, Vec<Def>> = HashMap::new();
    for file in index.files.values() {
        for s in file.symbols.iter().filter(|s| s.kind.is_code_def()) {
            map.entry(s.name.as_str()).or_default().push(Def {
                path: file.path.clone(),
                line: s.line,
                container: s.container.clone(),
                returns: s.returns.clone(),
            });
        }
    }
    for list in map.values_mut() {
        list.sort();
        list.dedup(); // two symbols on one line (generated code) are one definition
    }
    map
}

pub fn measure(index: &Index) -> Report {
    let defs = definitions(index);
    let resolver = Resolver::new(index);
    let mut report = Report {
        scores: STRATEGIES
            .iter()
            .map(|(name, _)| Score {
                name,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    for file in index.files.values().filter(|f| !f.is_doc()) {
        for (i, call) in file.calls.iter().enumerate() {
            let candidates = defs
                .get(call.callee.as_str())
                .map_or(&[][..], Vec::as_slice);
            report.calls += 1;
            match candidates.len() {
                0 => report.none += 1,
                1 => report.one += 1,
                _ => report.several += 1,
            }
            let Some(Resolution::Def { path, line }) = file.resolution(i) else {
                continue;
            };
            if candidates.len() < 2 {
                continue;
            }
            let Some(truth) = candidates
                .iter()
                .position(|d| d.path == *path && d.line == *line)
            else {
                report.unscored += 1;
                continue;
            };
            if separate_crates(&file.path, path) {
                report.unscored += 1;
                continue;
            }
            report.truth += 1;
            for ((name, pick), score) in STRATEGIES.iter().zip(&mut report.scores) {
                let picked = pick(&resolver, file, call, candidates);
                score.add(&picked, truth, candidates.len());
                if *name == "form" {
                    let kind = form_kind(&call.form);
                    let at = match report.by_form.iter().position(|s| s.name == kind) {
                        Some(at) => at,
                        None => {
                            report.by_form.push(Score {
                                name: kind,
                                ..Default::default()
                            });
                            report.by_form.len() - 1
                        }
                    };
                    report.by_form[at].add(&picked, truth, candidates.len());
                }
            }
        }
    }
    report
}

/// Cargo builds each file directly inside `benches/`, `tests/` or `examples/` as its own crate, so
/// two different ones cannot call each other; an index that says they do has merged same-named
/// items (rust-analyzer does for tokio's benches). Such answers are not scored.
fn separate_crates(from: &str, to: &str) -> bool {
    let single = |path: &str| {
        let mut parts = path.rsplit('/');
        let file = parts.next().unwrap_or("");
        let dir = parts.next();
        let above = parts.next();
        file.ends_with(".rs")
            && matches!(dir, Some("benches" | "tests" | "examples"))
            && above != Some("src") // `src/tests/a.rs` is a module of the crate
    };
    from != to && single(from) && single(to)
}

/// Up to `limit` calls per form kind that the `form` strategy did not pin to one right definition,
/// one line each: where it is, how it is written, what was picked and what SCIP says.
pub fn misses(index: &Index, limit: usize) -> Vec<String> {
    let defs = definitions(index);
    let resolver = Resolver::new(index);
    let mut shown: HashMap<&'static str, usize> = HashMap::new();
    let mut out = Vec::new();
    for file in index.files.values().filter(|f| !f.is_doc()) {
        for (i, call) in file.calls.iter().enumerate() {
            let Some(candidates) = defs.get(call.callee.as_str()).filter(|c| c.len() > 1) else {
                continue;
            };
            let Some(Resolution::Def { path, line }) = file.resolution(i) else {
                continue;
            };
            let Some(truth) = candidates
                .iter()
                .position(|d| d.path == *path && d.line == *line)
            else {
                continue;
            };
            if separate_crates(&file.path, path) {
                continue;
            }
            let picked = resolver.resolve(&file.path, call, candidates);
            if picked == [truth] {
                continue;
            }
            let kind = form_kind(&call.form);
            let count = shown.entry(kind).or_default();
            if *count >= limit {
                continue;
            }
            *count += 1;
            let at = |d: &Def| format!("{}:{}", d.path, d.line);
            let names: Vec<String> = picked.iter().take(3).map(|&p| at(&candidates[p])).collect();
            out.push(format!(
                "{kind} {}:{} {} {:?} owner={:?}: picked {}/{} [{}{}], SCIP {}",
                file.path,
                call.line,
                call.callee,
                call.form,
                call.owner,
                picked.len(),
                candidates.len(),
                names.join(" "),
                if picked.len() > 3 { " ..." } else { "" },
                at(&candidates[truth])
            ));
        }
    }
    out
}

/// The kind of a call form, for the breakdown.
fn form_kind(form: &Form) -> &'static str {
    match form {
        Form::Unknown => "unknown",
        Form::Bare => "bare",
        Form::SelfRef => "self",
        Form::Type(_) => "type",
        Form::Module(_) => "module",
        Form::Method => "method",
        Form::Returned(_) => "chain",
    }
}

impl Score {
    fn total(&self) -> usize {
        self.unique_right + self.unique_wrong + self.narrowed + self.ambiguous + self.lost
    }

    fn add(&mut self, picked: &[usize], truth: usize, all: usize) {
        let hit = picked.contains(&truth);
        match (hit, picked.len()) {
            (false, 1) => self.unique_wrong += 1,
            (false, _) => self.lost += 1,
            (true, 1) => self.unique_right += 1,
            (true, n) if n < all => self.narrowed += 1,
            (true, _) => self.ambiguous += 1,
        }
    }
}

fn percent(part: usize, whole: usize) -> usize {
    // Rounded to the nearest percent (a row of outcomes can add up to 99 or 101).
    (part * 200 + whole).checked_div(whole * 2).unwrap_or(0)
}

impl Report {
    pub fn lines(&self) -> Vec<String> {
        let share = |n| format!("{n} ({}%)", percent(n, self.calls));
        let mut out = vec![
            format!(
                "{} call sites in code files, by project definitions of the callee:",
                self.calls
            ),
            format!("  one (exact by name): {}", share(self.one)),
            format!(
                "  several (name alone can be wrong): {}",
                share(self.several)
            ),
            format!("  none (library or dynamic): {}", share(self.none)),
        ];
        let unscored = format!(
            "  {} more SCIP answers point at code we do not extract (macros, fields): not scored",
            self.unscored
        );
        if self.truth == 0 {
            out.push(if self.unscored == 0 {
                "no SCIP answers for calls to multi-definition names: run `just scip` (Rust, TypeScript)".into()
            } else {
                format!("nothing to score: {}", unscored.trim_start_matches(' '))
            });
            return out;
        }
        out.push(format!(
            "{} calls to several-definition names have a SCIP answer we can score; what each strategy picks:",
            self.truth
        ));
        for s in &self.scores {
            let of = |n| format!("{}%", percent(n, self.truth));
            out.push(format!(
                "  {}: right {}, wrong {}, narrowed {}, still all {}, lost the right one {}",
                s.name,
                of(s.unique_right),
                of(s.unique_wrong),
                of(s.narrowed),
                of(s.ambiguous),
                of(s.lost)
            ));
        }
        if !self.by_form.is_empty() {
            out.push("  `form` by how the call is written:".into());
            let mut kinds: Vec<&Score> = self.by_form.iter().collect();
            kinds.sort_by_key(|s| std::cmp::Reverse(s.total()));
            for s in kinds {
                let of = |n| format!("{}%", percent(n, s.total()));
                out.push(format!(
                    "    {} ({}): right {}, wrong {}, narrowed {}, still all {}, lost {}",
                    s.name,
                    s.total(),
                    of(s.unique_right),
                    of(s.unique_wrong),
                    of(s.narrowed),
                    of(s.ambiguous),
                    of(s.lost)
                ));
            }
        }
        if self.unscored > 0 {
            out.push(unscored);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Call, FileEntry, Form, Kind, Resolution, ResolvedCall, Symbol};

    fn symbol(name: &str, line: u32) -> Symbol {
        Symbol {
            name: name.into(),
            kind: Kind::Fn,
            line,
            signature: format!("fn {name}()"),
            container: None,
            returns: None,
        }
    }

    fn call(callee: &str, line: u32) -> Call {
        Call {
            callee: callee.into(),
            line,
            caller: Some("main".into()),
            form: Form::Unknown,
            owner: None,
        }
    }

    fn file(path: &str, symbols: Vec<Symbol>, calls: Vec<Call>) -> FileEntry {
        FileEntry {
            path: path.into(),
            symbols,
            calls,
            ..Default::default()
        }
    }

    /// `new` is defined in a.rs:1 and b.rs:1; `once` only in a.rs; main.rs calls all three plus `ext`.
    fn sample() -> Index {
        let mut main = file(
            "main.rs",
            vec![symbol("main", 1)],
            vec![
                call("new", 2),
                call("new", 3),
                call("once", 4),
                call("ext", 5),
            ],
        );
        // SCIP: the first `new` goes to b.rs:1, the second to a.rs:1 (dropped by no strategy).
        main.resolved = vec![
            ResolvedCall {
                call: 0,
                target: Resolution::Def {
                    path: "b.rs".into(),
                    line: 1,
                },
            },
            ResolvedCall {
                call: 1,
                target: Resolution::Def {
                    path: "a.rs".into(),
                    line: 1,
                },
            },
        ];
        let files = [
            file("a.rs", vec![symbol("new", 1), symbol("once", 2)], vec![]),
            file("b.rs", vec![symbol("new", 1)], vec![]),
            main,
        ];
        Index {
            files: files.into_iter().map(|f| (f.path.clone(), f)).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn counts_call_sites_by_how_many_definitions_the_callee_has() {
        let r = measure(&sample());
        assert_eq!((r.calls, r.one, r.several, r.none), (4, 1, 2, 1));
    }

    #[test]
    fn name_only_leaves_every_scip_resolved_call_ambiguous() {
        let r = measure(&sample());
        assert_eq!(r.truth, 2);
        let name = &r.scores[0];
        assert_eq!(name.name, "name");
        assert_eq!(
            (
                name.unique_right,
                name.unique_wrong,
                name.narrowed,
                name.ambiguous,
                name.lost
            ),
            (0, 0, 0, 2, 0)
        );
    }

    #[test]
    fn a_pick_is_filed_under_exactly_one_outcome() {
        let mut s = Score::default();
        s.add(&[1], 1, 3); // one left, SCIP's
        s.add(&[0], 1, 3); // one left, another
        s.add(&[0, 1], 1, 3); // narrowed
        s.add(&[0, 1, 2], 1, 3); // nothing excluded
        s.add(&[0, 2], 1, 3); // several left, SCIP's dropped
        s.add(&[], 1, 3); // nothing left
        assert_eq!(
            (
                s.unique_right,
                s.unique_wrong,
                s.narrowed,
                s.ambiguous,
                s.lost
            ),
            (1, 1, 1, 1, 2)
        );
    }

    #[test]
    fn docs_impl_blocks_and_headings_are_not_definitions_or_calls() {
        let mut index = sample();
        let mut doc = file("NOTES.md", vec![symbol("new", 1)], vec![call("new", 2)]);
        doc.symbols[0].kind = Kind::Heading;
        let mut imp = file("c.rs", vec![symbol("new", 1)], vec![]);
        imp.symbols[0].kind = Kind::Impl;
        index.files.insert(doc.path.clone(), doc);
        index.files.insert(imp.path.clone(), imp);
        let r = measure(&index);
        assert_eq!((r.calls, r.one, r.several, r.none), (4, 1, 2, 1));
        assert_eq!(r.truth, 2, "still two definitions of `new`");
    }

    #[test]
    fn a_scip_answer_we_do_not_extract_is_unscored_not_lost() {
        let mut index = sample();
        let main = index.files.get_mut("main.rs").unwrap();
        main.resolved[0].target = Resolution::Def {
            path: "gen.rs".into(),
            line: 9,
        };
        let r = measure(&index);
        assert_eq!((r.truth, r.unscored), (1, 1));
        assert_eq!(r.scores[0].lost, 0);
        assert!(r.lines().join("\n").contains("1 more SCIP answers"));
    }

    #[test]
    fn when_every_scip_answer_is_unscored_the_report_does_not_ask_for_scip_data() {
        let mut index = sample();
        let main = index.files.get_mut("main.rs").unwrap();
        for r in &mut main.resolved {
            r.target = Resolution::Def {
                path: "gen.rs".into(),
                line: 9,
            };
        }
        let text = measure(&index).lines().join("\n");
        assert!(
            text.contains("nothing to score: 2 more SCIP answers"),
            "{text}"
        );
        assert!(!text.contains("just scip"), "{text}");
    }

    #[test]
    fn two_symbols_on_one_line_are_one_definition() {
        let mut index = sample();
        let b = index.files.get_mut("b.rs").unwrap();
        b.symbols.push(symbol("new", 1)); // e.g. generated twice on the same line
        assert_eq!(measure(&index).several, 2, "b.rs:1 is still one definition");
        let a = index.files.get_mut("a.rs").unwrap();
        a.symbols.retain(|s| s.name != "new");
        assert_eq!(measure(&index).several, 0, "one definition left is exact");
    }

    #[test]
    fn an_index_without_calls_or_scip_data_reports_zeros_and_says_how_to_get_data() {
        let empty = measure(&Index::default());
        assert_eq!((empty.calls, empty.truth), (0, 0));
        let text = empty.lines().join("\n");
        assert!(text.contains("0 call sites"), "{text}");
        assert!(text.contains("just scip"), "{text}");
    }

    #[test]
    fn the_text_report_and_json_name_every_number() {
        let r = measure(&sample());
        let lines = r.lines();
        assert_eq!(
            lines[0],
            "4 call sites in code files, by project definitions of the callee:"
        );
        assert_eq!(lines[1], "  one (exact by name): 1 (25%)");
        assert_eq!(lines[2], "  several (name alone can be wrong): 2 (50%)");
        assert_eq!(lines[3], "  none (library or dynamic): 1 (25%)");
        assert!(lines[5].starts_with("  name: right 0%, wrong 0%, narrowed 0%, still all 100%"));
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["scores"][0]["name"], "name");
        assert_eq!(json["truth"], 2);
    }

    #[test]
    fn the_form_strategy_is_also_split_by_how_calls_are_written() {
        let r = measure(&sample());
        let total: usize = r.by_form.iter().map(Score::total).sum();
        assert_eq!(
            total, r.truth,
            "every scored call lands in exactly one kind"
        );
        assert!(r.by_form.iter().all(|s| s.total() > 0));
        let text = r.lines().join("\n");
        assert!(text.contains("`form` by how the call is written"), "{text}");
    }

    #[test]
    fn misses_list_calls_the_form_strategy_did_not_pin_down() {
        let all = misses(&sample(), 5);
        assert_eq!(all.len(), 2, "{all:?}");
        assert!(
            all[0].contains("main.rs:2") && all[0].contains("new") && all[0].contains("b.rs:1")
        );
        assert_eq!(misses(&sample(), 1).len(), 1, "capped per kind of form");
        assert!(misses(&sample(), 0).is_empty());
    }

    #[test]
    fn calls_between_separate_single_file_crates_are_not_scored() {
        // Each file directly in `benches/` is its own crate: SCIP merging their `rt` is its quirk.
        let mut a = file("benches/a.rs", vec![symbol("rt", 1)], vec![call("rt", 5)]);
        a.resolved = vec![ResolvedCall {
            call: 0,
            target: Resolution::Def {
                path: "benches/b.rs".into(),
                line: 9,
            },
        }];
        let b = file("benches/b.rs", vec![symbol("rt", 9)], vec![]);
        let files = [a, b];
        let index = Index {
            files: files.into_iter().map(|f| (f.path.clone(), f)).collect(),
            ..Default::default()
        };
        let r = measure(&index);
        assert_eq!((r.truth, r.unscored), (0, 1));
        assert!(misses(&index, 5).is_empty());
        assert!(separate_crates("x/benches/a.rs", "x/benches/b.rs"));
        assert!(!separate_crates("benches/a.rs", "benches/a.rs"));
        assert!(!separate_crates("src/a.rs", "src/b.rs"));
        assert!(!separate_crates("benches/sub/a.rs", "benches/sub/b.rs"));
        assert!(!separate_crates("tests/a.py", "tests/b.py"), "Rust only");
        assert!(!separate_crates("src/tests/a.rs", "src/tests/b.rs"));
    }
}
