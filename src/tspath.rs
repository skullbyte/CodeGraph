//! Following TypeScript/JavaScript imports and re-exports to the file that defines a name:
//! `import * as z from "zod/v4"; z.string()` goes through `export * from "./classic/index.js"`
//! and the like to the file that really holds `string`. Ours, the TS twin of `usepath.rs`
//! (Graphify records named re-exports but does not follow them). Every answer is empty when an
//! import cannot be followed, so callers keep their wider candidates.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::lang::Lang;
use crate::lang::typescript::module_stem;
use crate::model::{Index, Use};
use crate::query::resolve_ts_import;

/// Longest chain of re-exports followed before giving up.
const MAX_HOPS: usize = 8;

pub struct TsPaths {
    /// File -> its imports and re-exports.
    uses: HashMap<String, Vec<Use>>,
    /// File -> top-level names it defines.
    names: HashMap<String, HashSet<String>>,
    /// Every project file (resolving an import needs only the keys).
    files: BTreeMap<String, ()>,
    aliases: BTreeMap<String, String>,
}

/// Is `path` a TypeScript or JavaScript file?
pub fn is_ts(path: &str) -> bool {
    matches!(
        Lang::from_path(path),
        Some(Lang::TypeScript | Lang::Tsx | Lang::JavaScript)
    )
}

impl TsPaths {
    pub fn new(index: &Index) -> TsPaths {
        let mut uses = HashMap::new();
        let mut names = HashMap::new();
        for file in index.files.values().filter(|f| is_ts(&f.path)) {
            if !file.uses.is_empty() {
                uses.insert(file.path.clone(), file.uses.clone());
            }
            let defined: HashSet<String> = file
                .symbols
                .iter()
                .filter(|s| s.kind.is_code_def() && s.container.is_none())
                .map(|s| s.name.clone())
                .collect();
            names.insert(file.path.clone(), defined);
        }
        TsPaths {
            uses,
            names,
            files: index.files.keys().map(|k| (k.clone(), ())).collect(),
            aliases: index.aliases.clone(),
        }
    }

    /// Files that define `name` when it is called as `m.name()`, `m` a module imported whole
    /// (`import * as m`) whose file name is `stem` (see `module_stem`).
    /// A `stem` written `spec::item` is a name imported from a package (`zod/v4::z`): the
    /// namespace that package exports under that name.
    pub fn module(&self, from: &str, stem: &str, name: &str) -> Vec<String> {
        if let Some((spec, item)) = stem.rsplit_once("::") {
            let mut seen = HashSet::new();
            let modules = self.namespaces(from, spec, item, 0, &mut seen);
            return union(
                modules
                    .iter()
                    .map(|m| self.lookup(m, name, 0, &mut HashSet::new())),
            );
        }
        let whole = self
            .uses_of(from)
            .filter(|u| !u.path.contains("::") && module_stem(&u.path).as_deref() == Some(stem));
        union(whole.map(|u| self.follow(from, &u.path, name)))
    }

    /// Files that define `name` when `from` imported it by name (`import { name }`) and calls it
    /// bare; the file's own definition wins over any import.
    pub fn bare(&self, from: &str, name: &str) -> Vec<String> {
        if self.names.get(from).is_some_and(|n| n.contains(name)) {
            return Vec::new();
        }
        let named = self
            .uses_of(from)
            .filter(|u| u.alias == name)
            .filter_map(|u| u.path.rsplit_once("::"));
        union(named.map(|(spec, orig)| self.follow(from, spec, orig)))
    }

    /// The non-exported imports of `from`.
    fn uses_of<'a>(&'a self, from: &str) -> impl Iterator<Item = &'a Use> {
        self.uses
            .get(from)
            .into_iter()
            .flatten()
            .filter(|u| !u.public)
    }

    /// Files that define `name` as seen from the module `spec` imported in `from`.
    fn follow(&self, from: &str, spec: &str, name: &str) -> Vec<String> {
        resolve_ts_import(&self.files, &self.aliases, from, spec)
            .map(|file| self.lookup(&file, name, 0, &mut HashSet::new()))
            .unwrap_or_default()
    }

    /// Is `stem` (`spec::item`) a namespace the package exports, whatever members it has?
    pub fn is_namespace(&self, from: &str, stem: &str) -> bool {
        stem.rsplit_once("::").is_some_and(|(spec, item)| {
            !self
                .namespaces(from, spec, item, 0, &mut HashSet::new())
                .is_empty()
        })
    }

    /// Files of the module that `item`, imported from `spec` in `file`, is a namespace of:
    /// `export * as item from`, `export { item }` of an `import * as item`, through `export *`.
    fn namespaces(
        &self,
        file: &str,
        spec: &str,
        item: &str,
        hops: usize,
        seen: &mut HashSet<(String, String)>,
    ) -> Vec<String> {
        let Some(target) = resolve_ts_import(&self.files, &self.aliases, file, spec) else {
            return Vec::new();
        };
        if hops >= MAX_HOPS || !seen.insert((target.clone(), item.to_string())) {
            return Vec::new();
        }
        let mut found = Vec::new();
        for u in self
            .uses
            .get(&target)
            .into_iter()
            .flatten()
            .filter(|u| u.public)
        {
            match u.path.rsplit_once("::") {
                None if u.alias == item => {
                    found.extend(resolve_ts_import(
                        &self.files,
                        &self.aliases,
                        &target,
                        &u.path,
                    ));
                }
                Some((s, orig)) if u.alias == item => {
                    found.extend(self.namespaces(&target, s, orig, hops + 1, seen));
                }
                None if u.alias == "*" => {
                    found.extend(self.namespaces(&target, &u.path, item, hops + 1, seen));
                }
                _ => {}
            }
        }
        found.sort();
        found.dedup();
        found
    }

    /// The name `name` as seen from `file`: defined there, or re-exported by it
    /// (`export { a as name } from`, `export * from`). Cycles and long chains end.
    fn lookup(
        &self,
        file: &str,
        name: &str,
        hops: usize,
        seen: &mut HashSet<(String, String)>,
    ) -> Vec<String> {
        if self.names.get(file).is_some_and(|n| n.contains(name)) {
            return vec![file.to_string()];
        }
        if hops >= MAX_HOPS || !seen.insert((file.to_string(), name.to_string())) {
            return Vec::new();
        }
        let mut found = Vec::new();
        for u in self
            .uses
            .get(file)
            .into_iter()
            .flatten()
            .filter(|u| u.public)
        {
            let (spec, orig) = match u.path.rsplit_once("::") {
                Some((spec, orig)) if u.alias == name => (spec, orig),
                None if u.alias == "*" => (u.path.as_str(), name),
                _ => continue,
            };
            if let Some(next) = resolve_ts_import(&self.files, &self.aliases, file, spec) {
                found.extend(self.lookup(&next, orig, hops + 1, seen));
            }
        }
        found.sort();
        found.dedup();
        found
    }
}

/// Every file of every answer, sorted, once.
fn union(answers: impl Iterator<Item = Vec<String>>) -> Vec<String> {
    let mut all: Vec<String> = answers.flatten().collect();
    all.sort();
    all.dedup();
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::extract as extract_file;
    use crate::model::FileEntry;

    fn index_of(files: &[(&str, &str)]) -> Index {
        let files = files
            .iter()
            .map(|(path, src)| {
                let lang = Lang::from_path(path).unwrap();
                let facts = extract_file(lang, src).unwrap();
                let entry = FileEntry {
                    path: path.to_string(),
                    symbols: facts.symbols,
                    uses: facts.uses,
                    ..Default::default()
                };
                (path.to_string(), entry)
            })
            .collect();
        Index {
            files,
            ..Default::default()
        }
    }

    #[test]
    fn a_namespace_call_goes_through_star_re_exports_to_the_defining_file() {
        let mut index = index_of(&[
            ("t/a.test.ts", "import * as z from 'zod/v4';\nz.string();\n"),
            ("src/v4/index.ts", "export * from './classic/index.js';\n"),
            (
                "src/v4/classic/index.ts",
                "export * from './external.js';\n",
            ),
            (
                "src/v4/classic/external.ts",
                "export * from './checks.js';\nexport * from './schemas.js';\n",
            ),
            ("src/v4/classic/checks.ts", "export function min() {}\n"),
            ("src/v4/classic/schemas.ts", "export function string() {}\n"),
            ("src/v4/mini/schemas.ts", "export function string() {}\n"),
        ]);
        index.aliases = BTreeMap::from([("zod/v4".into(), "src/v4/index.ts".into())]);
        let paths = TsPaths::new(&index);
        assert_eq!(
            paths.module("t/a.test.ts", "v4", "string"),
            vec!["src/v4/classic/schemas.ts".to_string()]
        );
        // An unknown package or name gives nothing.
        assert!(paths.module("t/a.test.ts", "react", "string").is_empty());
        assert!(paths.module("t/a.test.ts", "v4", "nope").is_empty());
    }

    #[test]
    fn a_namespace_imported_by_name_is_followed_through_star_and_local_exports() {
        let mut index = index_of(&[
            ("t.ts", "import { z } from 'zod/v4';\nz.string();\n"),
            ("src/v4/index.ts", "export * from './classic/index.js';\n"),
            (
                "src/v4/classic/index.ts",
                "import * as z from './external.js';\nexport { z };\n",
            ),
            (
                "src/v4/classic/external.ts",
                "export * from './schemas.js';\n",
            ),
            ("src/v4/classic/schemas.ts", "export function string() {}\n"),
        ]);
        index.aliases = BTreeMap::from([("zod/v4".into(), "src/v4/index.ts".into())]);
        let paths = TsPaths::new(&index);
        assert_eq!(
            paths.module("t.ts", "zod/v4::z", "string"),
            vec!["src/v4/classic/schemas.ts".to_string()]
        );
        // A name that is not a namespace there gives nothing.
        assert!(paths.module("t.ts", "zod/v4::nope", "string").is_empty());
    }

    #[test]
    fn a_namespace_can_be_exported_as_a_star_alias_or_renamed() {
        let index = index_of(&[
            ("t.ts", "import { z, y } from './lib';\n"),
            (
                "lib.ts",
                "export * as z from './schemas';\nexport { z as y } from './lib2';\n",
            ),
            ("lib2.ts", "export * as z from './schemas';\n"),
            ("schemas.ts", "export function string() {}\n"),
        ]);
        let paths = TsPaths::new(&index);
        let want = vec!["schemas.ts".to_string()];
        assert_eq!(paths.module("t.ts", "./lib::z", "string"), want);
        assert_eq!(paths.module("t.ts", "./lib::y", "string"), want);
        assert!(paths.is_namespace("t.ts", "./lib::z"));
        assert!(!paths.is_namespace("t.ts", "./lib::nope"));
    }

    #[test]
    fn a_named_import_follows_a_renamed_re_export() {
        let index = index_of(&[
            ("app.ts", "import { make } from './lib';\nmake();\n"),
            ("lib/index.ts", "export { build as make } from './build';\n"),
            ("lib/build.ts", "export function build() {}\n"),
            (
                "own.ts",
                "import { make } from './lib';\nfunction make() {}\n",
            ),
        ]);
        let paths = TsPaths::new(&index);
        assert_eq!(
            paths.bare("app.ts", "make"),
            vec!["lib/build.ts".to_string()]
        );
        // The file's own definition wins over any import.
        assert!(paths.bare("own.ts", "make").is_empty());
    }

    #[test]
    fn a_re_export_cycle_ends() {
        let index = index_of(&[
            ("app.ts", "import * as a from './a';\na.f();\n"),
            ("a.ts", "export * from './b';\n"),
            ("b.ts", "export * from './a';\n"),
        ]);
        assert!(TsPaths::new(&index).module("app.ts", "a", "f").is_empty());
    }
}
