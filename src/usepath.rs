//! Following Rust `use` paths and `pub use` re-exports to the files that define a name.
//! `use tokio::task::spawn;` then `spawn()` goes to the file that really holds `spawn`, even when
//! `tokio::task` only re-exports it. Ours (no Graphify equivalent). A crate is a `src/lib.rs` or
//! `src/main.rs`, named after its folder (`tokio-stream/` is `tokio_stream`): Cargo.toml is not read.
//! Every answer is `None` when a path cannot be followed, so callers keep their wider candidates.

use std::collections::{HashMap, HashSet};

use crate::model::{Index, Kind, Use};

/// Longest chain of `pub use` followed before giving up.
const MAX_HOPS: usize = 4;

struct Crate {
    name: String,
    /// The package folder with a trailing `/` (`tokio/`), `""` at the project root.
    package: String,
    /// `tokio/src/`.
    src: String,
    /// `tokio/src/lib.rs`.
    root: String,
}

/// A module: the file that holds it and the folder its child modules live in.
#[derive(Clone)]
struct Module {
    file: String,
    dir: String,
}

pub struct UsePaths {
    crates: Vec<Crate>,
    uses: HashMap<String, Vec<Use>>,
    /// File -> names it defines (modules excluded: `mod spawn;` is not the function `spawn`).
    names: HashMap<String, HashSet<String>>,
    files: HashSet<String>,
    /// Top-level name -> package folders that define it.
    homes: HashMap<String, Vec<String>>,
}

fn is_rust(path: &str) -> bool {
    path.ends_with(".rs")
}

fn parent_dir(dir: &str) -> &str {
    dir.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

impl UsePaths {
    pub fn new(index: &Index) -> UsePaths {
        let mut crates = Vec::new();
        let (mut uses, mut names, mut files) = (HashMap::new(), HashMap::new(), HashSet::new());
        for file in index.files.values().filter(|f| is_rust(&f.path)) {
            files.insert(file.path.clone());
            if !file.uses.is_empty() {
                uses.insert(file.path.clone(), file.uses.clone());
            }
            let defined: HashSet<String> = file
                .symbols
                .iter()
                .filter(|s| s.kind.is_code_def() && s.kind != Kind::Mod && s.container.is_none())
                .map(|s| s.name.clone())
                .collect();
            names.insert(file.path.clone(), defined);
            let src = file
                .path
                .strip_suffix("lib.rs")
                .or_else(|| file.path.strip_suffix("main.rs"))
                .filter(|dir| dir.is_empty() || dir.ends_with('/'));
            if let Some(src) = src.filter(|s| s.is_empty() || s.ends_with("src/")) {
                let package = src.strip_suffix("src/").unwrap_or("").to_string();
                let name = package
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .replace('-', "_");
                crates.push(Crate {
                    name,
                    package,
                    src: src.to_string(),
                    root: file.path.clone(),
                });
            }
        }
        // A library is what other crates name, so it comes before a binary of the same package.
        crates.sort_by_key(|c| (c.root.ends_with("main.rs"), c.root.clone()));
        let mut homes: HashMap<String, Vec<String>> = HashMap::new();
        let mut paths = UsePaths {
            crates,
            uses,
            names,
            files,
            homes: HashMap::new(),
        };
        for (file, defined) in &paths.names {
            let Some(package) = paths.package_of(file).map(|c| c.package.clone()) else {
                continue;
            };
            for name in defined {
                homes.entry(name.clone()).or_default().push(package.clone());
            }
        }
        homes.values_mut().for_each(|v| {
            v.sort();
            v.dedup();
        });
        paths.homes = homes;
        paths
    }

    /// The package that holds `file` (the longest folder prefix).
    fn package_of(&self, file: &str) -> Option<&Crate> {
        self.crates
            .iter()
            .filter(|c| file.starts_with(&c.package))
            .max_by_key(|c| c.package.len())
    }

    /// The crate a file under `src/` belongs to: its package's library, unless the file is that
    /// package's `main.rs` or sits under `src/bin/` (then the binary, whose root is `main.rs`).
    fn crate_of(&self, file: &str) -> Option<&Crate> {
        let package = self.package_of(file).filter(|c| file.starts_with(&c.src))?;
        // `src/bin/x.rs` is a crate of its own that we do not model: no answer.
        if file.starts_with(&format!("{}bin/", package.src)) {
            return None;
        }
        let in_bin = file == package.src.clone() + "main.rs";
        self.crates
            .iter()
            .filter(|c| c.package == package.package)
            .find(|c| c.root.ends_with("main.rs") == in_bin)
            .or(Some(package))
    }

    /// The crate other crates call `name`; none when two packages share the folder name.
    fn crate_named(&self, name: &str) -> Option<&Crate> {
        let mut same = self
            .crates
            .iter()
            .filter(|c| !c.name.is_empty() && c.name == name);
        let first = same.next()?;
        same.all(|c| c.package == first.package).then_some(first)
    }

    fn root_of(&self, c: &Crate) -> Module {
        Module {
            file: c.root.clone(),
            dir: c.src.trim_end_matches('/').to_string(),
        }
    }

    /// The module a file is: `a/b.rs` holds `a::b` with children in `a/b/`; `a/mod.rs`, `lib.rs`
    /// and `main.rs` hold their folder's module.
    fn module_of(&self, file: &str) -> Module {
        let (dir, name) = file.rsplit_once('/').unwrap_or(("", file));
        let stem = name.strip_suffix(".rs").unwrap_or(name);
        let own = if matches!(stem, "mod" | "lib" | "main") {
            dir.to_string()
        } else {
            join(dir, stem)
        };
        Module {
            file: file.to_string(),
            dir: own,
        }
    }

    /// The module with children folder `dir`, if its file is indexed.
    fn module_at(&self, from: &str, dir: &str) -> Option<Module> {
        let file = [format!("{dir}/mod.rs"), format!("{dir}.rs")]
            .into_iter()
            .find(|f| self.files.contains(f))
            .or_else(|| {
                let c = self.crate_of(from)?;
                (c.src.trim_end_matches('/') == dir).then(|| c.root.clone())
            })?;
        Some(Module {
            file,
            dir: dir.to_string(),
        })
    }

    fn child(&self, parent: &Module, name: &str) -> Option<Module> {
        let dir = join(&parent.dir, name);
        [format!("{dir}.rs"), format!("{dir}/mod.rs")]
            .into_iter()
            .find(|f| self.files.contains(f))
            .map(|file| Module { file, dir })
    }

    /// Where a path written in `from` starts, and the segments left to walk.
    fn start<'a>(&self, from: &str, segs: &'a [&'a str]) -> Option<(Module, &'a [&'a str])> {
        let (&first, rest) = segs.split_first()?;
        match first {
            "crate" => {
                let c = self.crate_of(from)?;
                Some((self.root_of(c), rest))
            }
            "self" => Some((self.module_of(from), rest)),
            "super" => {
                let mut module = self.module_at(from, parent_dir(&self.module_of(from).dir))?;
                let mut rest = rest;
                while let Some((&"super", more)) = rest.split_first() {
                    module = self.module_at(from, parent_dir(&module.dir))?;
                    rest = more;
                }
                Some((module, rest))
            }
            name => match self.child(&self.module_of(from), name) {
                Some(local) => Some((local, rest)),
                None => Some((self.root_of(self.crate_named(name)?), rest)),
            },
        }
    }

    /// Files that define the item a path names, following `pub use` re-exports.
    fn item_files(&self, from: &str, path: &str, hops: usize) -> Vec<String> {
        let segs: Vec<&str> = path.split("::").collect();
        let Some((item, modules)) = segs.split_last() else {
            return Vec::new();
        };
        let Some((mut module, rest)) = self.start(from, modules) else {
            return Vec::new();
        };
        for seg in rest {
            match self.child(&module, seg) {
                Some(next) => module = next,
                None => return Vec::new(),
            }
        }
        self.lookup(&module, item, hops)
    }

    /// The item `name` as seen from module `module`: defined in its file, or re-exported by it.
    fn lookup(&self, module: &Module, name: &str, hops: usize) -> Vec<String> {
        if self
            .names
            .get(&module.file)
            .is_some_and(|n| n.contains(name))
        {
            return vec![module.file.clone()];
        }
        if hops >= MAX_HOPS {
            return Vec::new();
        }
        let mut found: Vec<String> = self
            .uses
            .get(&module.file)
            .into_iter()
            .flatten()
            .filter(|u| u.public && !u.local && u.alias == name)
            .flat_map(|u| self.item_files(&module.file, &u.path, hops + 1))
            .collect();
        found.sort();
        found.dedup();
        found
    }

    /// Every `use` of `alias` in the file (`cfg` variants, function-local ones).
    fn uses_of<'a>(&'a self, from: &str, alias: &str) -> Vec<&'a Use> {
        self.uses
            .get(from)
            .into_iter()
            .flatten()
            .filter(|u| u.alias == alias)
            .collect()
    }

    /// Files that define `name` when `from` brought it in with `use` and calls it bare. A renamed
    /// import (`use a::f as g`) is not followed: the callee is no longer `f`.
    pub fn bare(&self, from: &str, name: &str) -> Vec<String> {
        if self.names.get(from).is_some_and(|n| n.contains(name)) {
            return Vec::new(); // the file's own definition wins over any import
        }
        let uses = self.uses_of(from, name);
        if uses.iter().any(|u| u.local) {
            return Vec::new(); // a use in a mod or fn may shadow the file-level one
        }
        let unrenamed = |u: &&Use| u.path.rsplit("::").next() == Some(name);
        if uses.is_empty() || !uses.iter().all(unrenamed) {
            return Vec::new();
        }
        union_of_all(uses.iter().map(|u| self.item_files(from, &u.path, 0)))
    }

    /// Files that define `name` when it is called as `m::name()`: `m` is a module brought in by
    /// `use`, a crate, or a child module of `from`.
    pub fn module(&self, from: &str, m: &str, name: &str) -> Vec<String> {
        let uses = self.uses_of(from, m);
        if uses.iter().any(|u| u.local) {
            return Vec::new();
        }
        if uses.is_empty() {
            let module = self
                .child(&self.module_of(from), m)
                .or_else(|| self.crate_named(m).map(|c| self.root_of(c)));
            return module.map_or_else(Vec::new, |module| self.lookup(&module, name, 0));
        }
        union_of_all(uses.iter().map(|u| {
            self.module_by_path(from, &u.path)
                .map_or_else(Vec::new, |module| self.lookup(&module, name, 0))
        }))
    }

    fn module_by_path(&self, from: &str, path: &str) -> Option<Module> {
        let segs: Vec<&str> = path.split("::").collect();
        let (mut module, rest) = self.start(from, &segs)?;
        for seg in rest {
            module = self.child(&module, seg)?;
        }
        Some(module)
    }

    /// Package folders that define a different type named `ty` than the one `from` imported.
    /// Other packages may still add members to the imported one (`impl Trait for Bld`).
    pub fn rival_packages(&self, from: &str, ty: &str) -> Vec<String> {
        let mine = self.type_packages(from, ty);
        if mine.is_empty() {
            return Vec::new();
        }
        let all = self.homes.get(ty).map_or(&[][..], Vec::as_slice);
        all.iter().filter(|p| !mine.contains(p)).cloned().collect()
    }

    /// The package folder (`tokio/`) that holds a file: the longest one, so nested packages differ.
    pub fn package_folder_of(&self, file: &str) -> Option<&str> {
        self.package_of(file).map(|c| c.package.as_str())
    }

    /// The package folders (`tokio/`) that define type `ty` when `from` imported it.
    pub fn type_packages(&self, from: &str, ty: &str) -> Vec<String> {
        let mut packages: Vec<String> = self
            .bare(from, ty)
            .iter()
            .filter_map(|file| Some(self.package_of(file)?.package.clone()))
            .collect();
        packages.sort();
        packages.dedup();
        packages
    }
}

/// The files of every answer, or nothing when any answer is empty: one unfollowable `use` of a
/// name means the others cannot rule anything out.
fn union_of_all(answers: impl Iterator<Item = Vec<String>>) -> Vec<String> {
    let mut all = Vec::new();
    for files in answers {
        if files.is_empty() {
            return Vec::new();
        }
        all.extend(files);
    }
    all.sort();
    all.dedup();
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileEntry, Symbol};

    fn symbol(name: &str, kind: Kind) -> Symbol {
        Symbol {
            name: name.into(),
            kind,
            line: 1,
            signature: String::new(),
            container: None,
            returns: None,
        }
    }

    fn file(path: &str, uses: &[(&str, &str, bool)], defs: &[(&str, Kind)]) -> FileEntry {
        FileEntry {
            path: path.into(),
            uses: uses
                .iter()
                .map(|(alias, path, public)| Use {
                    alias: alias.to_string(),
                    path: path.to_string(),
                    public: *public,
                    local: false,
                })
                .collect(),
            symbols: defs.iter().map(|(n, k)| symbol(n, *k)).collect(),
            ..Default::default()
        }
    }

    fn paths(files: Vec<FileEntry>) -> UsePaths {
        UsePaths::new(&Index {
            files: files.into_iter().map(|f| (f.path.clone(), f)).collect(),
            ..Default::default()
        })
    }

    /// A tokio-shaped workspace: `tok::spawn` is re-exported twice before its definition.
    fn workspace() -> UsePaths {
        paths(vec![
            file(
                "tok/src/lib.rs",
                &[("spawn", "task::spawn", true)],
                &[("task", Kind::Mod)],
            ),
            file(
                "tok/src/task/mod.rs",
                &[("spawn", "self::spawn::spawn", true)],
                &[("spawn", Kind::Mod)],
            ),
            file("tok/src/task/spawn.rs", &[], &[("spawn", Kind::Fn)]),
            file(
                "tok/src/runtime/mod.rs",
                &[("Builder", "self::builder::Builder", true)],
                &[],
            ),
            file(
                "tok/src/runtime/builder.rs",
                &[],
                &[("Builder", Kind::Struct)],
            ),
            file("tok/src/util.rs", &[], &[("run", Kind::Fn)]),
            file("tok/src/a.rs", &[("run", "crate::util::run", false)], &[]),
            file("other/src/lib.rs", &[], &[("spawn", Kind::Fn)]),
            file(
                "app/src/main.rs",
                &[
                    ("spawn", "tok::task::spawn", false),
                    ("task", "tok::task", false),
                    ("Builder", "tok::runtime::Builder", false),
                    ("serde_thing", "serde::thing", false),
                ],
                &[],
            ),
        ])
    }

    #[test]
    fn a_crate_name_reaches_a_function_through_two_re_exports() {
        assert_eq!(
            workspace().module("app/src/main.rs", "tok", "spawn"),
            vec!["tok/src/task/spawn.rs"]
        );
    }

    #[test]
    fn a_bare_call_follows_its_use_path() {
        assert_eq!(
            workspace().bare("app/src/main.rs", "spawn"),
            vec!["tok/src/task/spawn.rs"]
        );
    }

    #[test]
    fn a_module_alias_looks_inside_that_module() {
        assert_eq!(
            workspace().module("app/src/main.rs", "task", "spawn"),
            vec!["tok/src/task/spawn.rs"]
        );
    }

    #[test]
    fn crate_self_and_super_paths_are_relative_to_the_file() {
        let w = workspace();
        assert_eq!(w.bare("tok/src/a.rs", "run"), vec!["tok/src/util.rs"]);
        let w = paths(vec![
            file("p/src/lib.rs", &[], &[]),
            file("p/src/m/mod.rs", &[], &[("up", Kind::Fn)]),
            file("p/src/m/n.rs", &[("up", "super::up", false)], &[]),
        ]);
        assert_eq!(w.bare("p/src/m/n.rs", "up"), vec!["p/src/m/mod.rs"]);
    }

    #[test]
    fn a_type_names_the_package_that_defines_it() {
        assert_eq!(
            workspace().type_packages("app/src/main.rs", "Builder"),
            vec!["tok/"]
        );
    }

    #[test]
    fn what_cannot_be_followed_gives_nothing() {
        let w = workspace();
        // an external crate, a name never imported, a renamed import, a private `use` re-export
        assert!(w.bare("app/src/main.rs", "serde_thing").is_empty());
        assert!(w.bare("app/src/main.rs", "missing").is_empty());
        assert!(w.module("app/src/main.rs", "serde", "thing").is_empty());
        let renamed = paths(vec![
            file("p/src/lib.rs", &[("g", "f", false)], &[]),
            file("p/src/f.rs", &[], &[("f", Kind::Fn)]),
        ]);
        assert!(renamed.bare("p/src/lib.rs", "g").is_empty());
        let private = paths(vec![
            file(
                "p/src/lib.rs",
                &[("f", "inner::f", false)],
                &[("inner", Kind::Mod)],
            ),
            file("p/src/inner.rs", &[("f", "deep::f", false)], &[]),
            file("p/src/inner/deep.rs", &[], &[("f", Kind::Fn)]),
        ]);
        assert!(private.module("p/src/lib.rs", "inner", "f").is_empty());
    }

    #[test]
    fn a_re_export_loop_stops() {
        let w = paths(vec![
            file("p/src/lib.rs", &[("f", "a::f", true)], &[]),
            file("p/src/a.rs", &[("f", "crate::f", true)], &[]),
        ]);
        assert!(w.module("p/src/lib.rs", "a", "f").is_empty());
    }

    #[test]
    fn two_uses_of_one_name_both_count_and_one_unfollowable_cancels_all() {
        let w = paths(vec![
            file(
                "p/src/lib.rs",
                &[("f", "a::f", false), ("f", "b::f", false)],
                &[],
            ),
            file("p/src/a.rs", &[], &[("f", Kind::Fn)]),
            file("p/src/b.rs", &[], &[("f", Kind::Fn)]),
        ]);
        assert_eq!(
            w.bare("p/src/lib.rs", "f"),
            vec!["p/src/a.rs", "p/src/b.rs"]
        );
        let w = paths(vec![
            file(
                "p/src/lib.rs",
                &[("f", "a::f", false), ("f", "serde::f", false)],
                &[],
            ),
            file("p/src/a.rs", &[], &[("f", Kind::Fn)]),
        ]);
        assert!(w.bare("p/src/lib.rs", "f").is_empty());
    }

    #[test]
    fn crate_is_the_library_unless_the_file_is_a_binary() {
        let w = paths(vec![
            file("p/src/lib.rs", &[], &[("lib_fn", Kind::Fn)]),
            file(
                "p/src/main.rs",
                &[("lib_fn", "crate::bin_fn", false)],
                &[("bin_fn", Kind::Fn)],
            ),
            file("p/src/x.rs", &[("lib_fn", "crate::lib_fn", false)], &[]),
            file("p/src/main_user.rs", &[], &[]),
            file(
                "p/src/bin/tool.rs",
                &[("bin_fn", "crate::bin_fn", false)],
                &[],
            ),
        ]);
        assert_eq!(w.bare("p/src/x.rs", "lib_fn"), vec!["p/src/lib.rs"]);
        let root = |f: &str| w.crate_of(f).map(|c| c.root.clone());
        assert_eq!(root("p/src/main.rs").as_deref(), Some("p/src/main.rs"));
        assert_eq!(root("p/src/main_user.rs").as_deref(), Some("p/src/lib.rs"));
        assert_eq!(root("p/src/bin/tool.rs"), None);
    }

    #[test]
    fn a_local_module_shadows_a_crate_and_ambiguous_crate_names_give_nothing() {
        let w = paths(vec![
            file("config/src/lib.rs", &[], &[("load", Kind::Fn)]),
            file("app/src/lib.rs", &[], &[("config", Kind::Mod)]),
            file("app/src/config.rs", &[], &[("load", Kind::Fn)]),
            file("app/src/a.rs", &[], &[]),
        ]);
        // `a.rs` has no child `config`; lib.rs does
        assert_eq!(
            w.module("app/src/lib.rs", "config", "load"),
            vec!["app/src/config.rs"]
        );
        let twin = paths(vec![
            file("a/app/src/lib.rs", &[], &[("f", Kind::Fn)]),
            file("b/app/src/lib.rs", &[], &[("f", Kind::Fn)]),
            file("c/src/lib.rs", &[], &[]),
        ]);
        assert!(twin.module("c/src/lib.rs", "app", "f").is_empty());
    }

    #[test]
    fn methods_do_not_stop_the_search_for_a_re_export() {
        let mut m = file(
            "p/src/m.rs",
            &[("new", "crate::other::new", true)],
            &[("new", Kind::Fn)],
        );
        m.symbols[0].container = Some("X".into());
        let w = paths(vec![
            file("p/src/lib.rs", &[], &[]),
            m,
            file("p/src/other.rs", &[], &[("new", Kind::Fn)]),
        ]);
        assert_eq!(w.module("p/src/lib.rs", "m", "new"), vec!["p/src/other.rs"]);
    }

    #[test]
    fn bins_without_a_modelled_root_and_own_definitions_give_nothing() {
        let w = paths(vec![
            file("p/src/lib.rs", &[], &[("f", Kind::Fn)]),
            file("p/src/bin/tool.rs", &[("f", "crate::f", false)], &[]),
            file(
                "p/src/x.rs",
                &[("f", "crate::f", false)],
                &[("f", Kind::Fn)],
            ),
        ]);
        assert!(w.bare("p/src/bin/tool.rs", "f").is_empty());
        assert!(w.bare("p/src/x.rs", "f").is_empty());
    }

    #[test]
    fn a_local_use_of_the_name_cancels_the_answer() {
        let mut shadow = file("p/src/lib.rs", &[("f", "a::f", false)], &[]);
        shadow.uses.push(Use {
            alias: "f".into(),
            path: "b::f".into(),
            public: false,
            local: true,
        });
        let w = paths(vec![shadow, file("p/src/a.rs", &[], &[("f", Kind::Fn)])]);
        assert!(w.bare("p/src/lib.rs", "f").is_empty());
    }

    #[test]
    fn nested_packages_are_told_apart_by_their_longest_folder() {
        let w = paths(vec![
            file("tok/src/lib.rs", &[], &[]),
            file("tok/sub/src/lib.rs", &[], &[]),
        ]);
        assert_eq!(w.package_folder_of("tok/sub/src/x.rs"), Some("tok/sub/"));
        assert_eq!(w.package_folder_of("tok/src/x.rs"), Some("tok/"));
        assert_eq!(w.package_folder_of("elsewhere/x.rs"), None);
    }
}
