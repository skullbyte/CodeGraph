//! Choosing among several definitions of one name by where the call is made. Without a compiler
//! (SCIP) we only have names, so we use what the code says about scope: a call most likely goes to
//! a definition in its own file, else in a file it imports, else in its own folder (same package).
//! Ours; Graphify resolves per language with receiver types, which comes after this.

use std::collections::{BTreeSet, HashMap};

use crate::model::Index;
use crate::query;

/// Where a definition is: file and 1-based line.
pub type Def = (String, u32);

/// Where a definition sits relative to a call, best first. `SameFolder` also holds the caller's own
/// file (a file is in its folder), so scoring it alone reads as "same file or same folder".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    SameFile,
    Imported,
    SameFolder,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::SameFile, Tier::Imported, Tier::SameFolder];
}

/// Built once per index: resolving every import is the costly part, asking is cheap.
pub struct Resolver {
    /// File -> the other project files it imports.
    imports: HashMap<String, BTreeSet<String>>,
}

/// Folder of an index path; `""` for a file at the project root.
fn folder(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

impl Resolver {
    pub fn new(index: &Index) -> Resolver {
        let mut imports = query::import_edges(index);
        for (file, targets) in &mut imports {
            targets.remove(file); // `from . import x` can resolve to the file itself
        }
        Resolver {
            imports: imports.into_iter().collect(),
        }
    }

    /// Indexes into `defs` that a call made in file `from` can reach: the best tier that has any
    /// definition, or all of them when no tier does. `from` is an index path (`src/a.rs`, no `./`).
    pub fn narrow(&self, from: &str, defs: &[Def]) -> Vec<usize> {
        Tier::ALL
            .iter()
            .map(|&tier| self.in_tier(tier, from, defs))
            .find(|hits| !hits.is_empty())
            .unwrap_or_else(|| (0..defs.len()).collect())
    }

    /// The definitions in one tier alone (empty when it has none). `from` is an index path.
    pub fn in_tier(&self, tier: Tier, from: &str, defs: &[Def]) -> Vec<usize> {
        let here = folder(from);
        let imported = self.imports.get(from);
        (0..defs.len())
            .filter(|&i| {
                let path = defs[i].0.as_str();
                match tier {
                    Tier::SameFile => path == from,
                    Tier::Imported => imported.is_some_and(|set| set.contains(path)),
                    Tier::SameFolder => folder(path) == here,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FileEntry;

    fn file(path: &str, imports: &[&str]) -> FileEntry {
        FileEntry {
            path: path.into(),
            imports: imports.iter().map(|i| i.to_string()).collect(),
            ..Default::default()
        }
    }

    fn index(files: Vec<FileEntry>) -> Index {
        Index {
            files: files.into_iter().map(|f| (f.path.clone(), f)).collect(),
            ..Default::default()
        }
    }

    fn defs(places: &[(&str, u32)]) -> Vec<Def> {
        places.iter().map(|(p, l)| (p.to_string(), *l)).collect()
    }

    #[test]
    fn the_callers_own_file_wins() {
        let r = Resolver::new(&index(vec![
            file("src/a.ts", &["./b"]),
            file("src/b.ts", &[]),
        ]));
        let d = defs(&[("src/b.ts", 1), ("src/a.ts", 7), ("src/z/c.ts", 1)]);
        assert_eq!(r.narrow("src/a.ts", &d), vec![1]);
    }

    #[test]
    fn an_imported_file_beats_a_file_in_the_same_folder() {
        let r = Resolver::new(&index(vec![
            file("src/a.ts", &["./sub/b"]),
            file("src/sub/b.ts", &[]),
            file("src/other.ts", &[]),
        ]));
        let d = defs(&[("src/other.ts", 3), ("src/sub/b.ts", 4)]);
        assert_eq!(r.narrow("src/a.ts", &d), vec![1]);
    }

    #[test]
    fn the_same_folder_is_the_last_tier_and_keeps_every_definition_there() {
        let r = Resolver::new(&index(vec![file("pkg/a.ts", &[])]));
        let d = defs(&[("pkg/x.ts", 1), ("pkg/y.ts", 2), ("elsewhere/z.ts", 3)]);
        assert_eq!(r.narrow("pkg/a.ts", &d), vec![0, 1]);
    }

    #[test]
    fn with_no_tier_every_definition_stays_a_candidate() {
        let r = Resolver::new(&index(vec![file("a/a.ts", &[])]));
        let d = defs(&[("b/x.ts", 1), ("c/y.ts", 2)]);
        assert_eq!(r.narrow("a/a.ts", &d), vec![0, 1]);
        assert_eq!(r.narrow("a/a.ts", &[]), Vec::<usize>::new());
    }

    #[test]
    fn two_definitions_in_the_callers_own_file_both_stay() {
        let r = Resolver::new(&index(vec![file("a.ts", &[])]));
        let d = defs(&[("a.ts", 1), ("a.ts", 9), ("b.ts", 1)]);
        assert_eq!(r.narrow("a.ts", &d), vec![0, 1]);
    }

    #[test]
    fn each_tier_alone_returns_only_its_own_definitions_or_nothing() {
        let r = Resolver::new(&index(vec![
            file("src/a.ts", &["./b"]),
            file("src/b.ts", &[]),
        ]));
        let d = defs(&[("src/a.ts", 1), ("src/b.ts", 2), ("far/c.ts", 3)]);
        assert_eq!(r.in_tier(Tier::SameFile, "src/a.ts", &d), vec![0]);
        assert_eq!(r.in_tier(Tier::Imported, "src/a.ts", &d), vec![1]);
        assert_eq!(r.in_tier(Tier::SameFolder, "src/a.ts", &d), vec![0, 1]);
        assert!(r.in_tier(Tier::Imported, "far/c.ts", &d).is_empty());
    }

    #[test]
    fn folders_are_compared_whole_and_the_root_is_a_folder() {
        let r = Resolver::new(&index(vec![file("a.ts", &[]), file("src/a.ts", &[])]));
        let d = defs(&[("src/x.ts", 1), ("x.ts", 2), ("src/ab/y.ts", 3)]);
        assert_eq!(
            r.narrow("a.ts", &d),
            vec![1],
            "root file: only root definitions"
        );
        assert_eq!(r.narrow("src/a.ts", &d), vec![0], "src/ab is not src");
        assert_eq!(
            r.narrow("src/unknown.ts", &d),
            vec![0],
            "a caller not in the index still has a folder"
        );
        assert_eq!(
            r.narrow("nowhere/a.ts", &d),
            vec![0, 1, 2],
            "no tier matches: all candidates, no panic"
        );
    }

    #[test]
    fn a_file_that_imports_itself_does_not_count_as_imported() {
        let r = Resolver::new(&index(vec![
            file("pkg/__init__.py", &["."]),
            file("pkg/m.py", &[]),
        ]));
        let d = defs(&[("pkg/__init__.py", 1), ("pkg/m.py", 2)]);
        assert!(r.in_tier(Tier::Imported, "pkg/__init__.py", &d).is_empty());
    }

    #[test]
    fn duplicate_definitions_and_empty_input_keep_their_positions() {
        let r = Resolver::new(&index(vec![
            file("a.ts", &["./b", "./b.ts"]),
            file("b.ts", &[]),
        ]));
        let d = defs(&[("b.ts", 1), ("b.ts", 1)]);
        assert_eq!(r.narrow("a.ts", &d), vec![0, 1]);
        assert!(r.narrow("a.ts", &[]).is_empty());
    }
}
