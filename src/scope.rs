//! Choosing among several definitions of one name by where the call is made. Without a compiler
//! (SCIP) we only have names, so we use what the code says about scope: a call most likely goes to
//! a definition in its own file, else in a file it imports, else in its own folder (same package).
//! Ours; Graphify resolves per language with receiver types, which comes after this.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::lang::Lang;
use crate::model::{Call, Form, Hop, Index, Kind, Ret};
use crate::query;
use crate::usepath::{UsePaths, plain_type};

/// A project definition: file, 1-based line (as SCIP names it) and the type or trait it is in.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Def {
    pub path: String,
    pub line: u32,
    pub container: Option<String>,
    /// The class the function returns, when its signature says so.
    pub returns: Option<Ret>,
}

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
    /// Every project definition by name: what a step of a call chain can be.
    by_name: HashMap<String, Vec<Def>>,
    /// Rust `use` paths and re-exports.
    paths: UsePaths,
    /// Names of traits (any language): their default methods are what extension traits add.
    traits: HashSet<String>,
    /// Names declared as a type alias somewhere: an alias hides what its target type implements.
    aliased: HashSet<String>,
    /// Names declared as a struct, enum or class: the only kinds an extension trait is added to.
    nominal: HashSet<String>,
}

/// The class a call returns, and the files of the functions that return it.
struct Typed {
    ty: String,
    froms: Vec<String>,
}

/// Longest call chain followed; a longer one is read as a plain method call.
const MAX_CHAIN: usize = 32;

/// Indexes into `defs` whose container is `ty`.
fn members_of(defs: &[Def], ty: &str) -> Vec<usize> {
    (0..defs.len())
        .filter(|&i| defs[i].container.as_deref() == Some(ty))
        .collect()
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
        let mut by_name: HashMap<String, Vec<Def>> = HashMap::new();
        let mut traits = HashSet::new();
        let mut aliased = HashSet::new();
        let mut nominal = HashSet::new();
        for file in index.files.values() {
            for s in file.symbols.iter().filter(|s| s.kind.is_code_def()) {
                if s.kind == Kind::Trait {
                    traits.insert(s.name.clone());
                }
                if s.kind == Kind::Type {
                    aliased.insert(s.name.clone());
                }
                if matches!(s.kind, Kind::Struct | Kind::Enum | Kind::Class) {
                    nominal.insert(s.name.clone());
                }
                by_name.entry(s.name.clone()).or_default().push(Def {
                    path: file.path.clone(),
                    line: s.line,
                    container: s.container.clone(),
                    returns: s.returns.clone(),
                });
            }
        }
        Resolver {
            imports: imports.into_iter().collect(),
            by_name,
            paths: UsePaths::new(index),
            traits,
            aliased,
            nominal,
        }
    }

    /// The class an earlier call in a chain returns: resolve that call, and when every definition
    /// it can reach returns the same class, that is the receiver's type. `?` / `.unwrap()` must
    /// match a `Result`/`Option` return, and only those. A step written as a plain method call
    /// (`x.get(k)`) is never trusted: `x` may be a std type that shares the name.
    fn hop_type(&self, from: &str, hop: &Hop, owner: Option<&str>, depth: usize) -> Option<Typed> {
        if depth >= MAX_CHAIN {
            return None;
        }
        let defs = self.by_name.get(&hop.callee)?;
        let picked = match &hop.form {
            Form::Method | Form::Unknown => return None,
            Form::Returned(inner) => {
                let ty = self.hop_type(from, inner, owner, depth + 1)?;
                self.members_typed(defs, &ty)
            }
            Form::Type(ty) => {
                let plain = plain_type(ty);
                self.resolve_form(from, &hop.callee, &hop.form, owner, defs, depth + 1)
                    .into_iter()
                    .filter(|&i| defs[i].container.as_deref() == Some(plain))
                    .collect()
            }
            // The wider fallback of `resolve_form` may not decide a type: keep real matches only.
            Form::SelfRef => owner.map_or_else(Vec::new, |o| members_of(defs, o)),
            Form::Bare | Form::Module(_) => self
                .resolve_form(from, &hop.callee, &hop.form, owner, defs, depth + 1)
                .into_iter()
                .filter(|&i| defs[i].container.is_none())
                .collect(),
        };
        let first = defs.get(*picked.first()?)?.returns.as_ref()?;
        let agree = picked
            .iter()
            .all(|&i| defs.get(i).and_then(|d| d.returns.as_ref()) == Some(first));
        if !(agree && first.fallible == hop.unwrapped && first.future == hop.awaited) {
            return None;
        }
        let ty = match hop.part {
            Some(n) => first.parts.get(n as usize)?,
            None => &first.ty,
        };
        // The returned class is the one named in the file where the return type is written.
        let mut froms: Vec<String> = picked.iter().map(|&i| defs[i].path.clone()).collect();
        froms.sort();
        froms.dedup();
        (!ty.is_empty()).then(|| Typed {
            ty: ty.clone(),
            froms,
        })
    }

    /// Members of the returned class, minus those of another type of the same name (`mpsc::Sender`
    /// vs `oneshot::Sender`): a member goes only when it is a rival from the point of view of every
    /// file that returns the class. Never empties the set.
    fn members_typed(&self, defs: &[Def], typed: &Typed) -> Vec<usize> {
        let members = members_of(defs, &typed.ty);
        let rivals: Vec<(Vec<String>, Vec<String>)> = typed
            .froms
            .iter()
            .map(|f| {
                (
                    self.paths.rival_files(f, &typed.ty),
                    self.paths.rival_packages(f, &typed.ty),
                )
            })
            .collect();
        let kept: Vec<usize> = members
            .iter()
            .copied()
            .filter(|&i| {
                let path = &defs[i].path;
                let package = self.paths.package_folder_of(path);
                !rivals.iter().all(|(files, packages)| {
                    files.contains(path) || package.is_some_and(|p| packages.iter().any(|r| r == p))
                })
            })
            .collect();
        if kept.is_empty() { members } else { kept }
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

    /// Indexes into `defs` that `call`, made in file `from`, can reach, by how it is written:
    /// `self.f()` goes to a method of the enclosing type, `Foo::f()` to a member of Foo,
    /// `x.f()` to some method, `f()` to a free function. A trait's default method and an impl's
    /// override of it are different containers and are not linked to each other (known gap). Each rule falls back to the wider set
    /// when it would leave nothing, so a rule can only narrow, never invent.
    pub fn resolve(&self, from: &str, call: &Call, defs: &[Def]) -> Vec<usize> {
        self.resolve_form(
            from,
            &call.callee,
            &call.form,
            call.owner.as_deref(),
            defs,
            0,
        )
    }

    fn resolve_form(
        &self,
        from: &str,
        name: &str,
        form: &Form,
        owner: Option<&str>,
        defs: &[Def],
        depth: usize,
    ) -> Vec<usize> {
        let all = || (0..defs.len()).collect::<Vec<_>>();
        let of_type = |t: &str| members_of(defs, t);
        let narrowed = |set: Vec<usize>| if set.is_empty() { all() } else { set };
        match form {
            Form::Returned(hop) => match self.hop_type(from, hop, owner, depth) {
                Some(t) => match self.members_typed(defs, &t) {
                    members if !members.is_empty() => members,
                    // Not the trait fallback: a returned `Spawn<T>` reaches `T`'s methods by deref.
                    _ => self.resolve_form(from, name, &Form::Method, owner, defs, depth),
                },
                None => self.resolve_form(from, name, &Form::Method, owner, defs, depth),
            },
            Form::Unknown => all(),
            Form::SelfRef => owner.map_or_else(all, |o| narrowed(of_type(o))),
            Form::Type(t) => {
                let members = of_type(plain_type(t));
                // Drop members of other packages that define their own type of this name; keep
                // the rest, since a trait impl elsewhere can add members to the imported type.
                let rivals = self.paths.rival_packages(from, t);
                let rival_files = self.paths.rival_files(from, t);
                let inside: Vec<usize> = members
                    .iter()
                    .copied()
                    .filter(|&i| !rival_files.contains(&defs[i].path))
                    .filter(|&i| {
                        self.paths
                            .package_folder_of(&defs[i].path)
                            .is_none_or(|p| !rivals.iter().any(|r| r == p))
                    })
                    .collect();
                match (inside.is_empty(), members.is_empty()) {
                    (false, _) => inside,
                    (true, false) => members,
                    (true, true) => narrowed(self.trait_members(from, name, t, defs)),
                }
            }
            Form::Method => narrowed(
                (0..defs.len())
                    .filter(|&i| defs[i].container.is_some())
                    .collect(),
            ),
            Form::Bare | Form::Module(_) => {
                let mut set: Vec<usize> = (0..defs.len())
                    .filter(|&i| defs[i].container.is_none())
                    .collect();
                if *form == Form::Bare && implicit_this(from) {
                    set.extend(owner.map(of_type).unwrap_or_default());
                    set.sort_unstable();
                }
                if set.is_empty() {
                    return all();
                }
                let files = match form {
                    Form::Module(m) => self.paths.module(from, m, name),
                    _ => self.paths.bare(from, name),
                };
                let used: Vec<usize> = set
                    .iter()
                    .copied()
                    .filter(|&i| files.contains(&defs[i].path))
                    .collect();
                if !used.is_empty() {
                    return used;
                }
                // `time::advance()` lives in `time.rs` or somewhere under `time/`.
                if let Form::Module(m) = form {
                    let named: Vec<usize> = set
                        .iter()
                        .copied()
                        .filter(|&i| in_module(&defs[i].path, m))
                        .collect();
                    if !named.is_empty() {
                        return named;
                    }
                }
                self.narrow_among(from, defs, set)
            }
        }
    }

    /// A project type `ty` that has no member `f` of its own may get it from a trait (an extension
    /// trait such as `AsyncReadExt`): the definitions inside traits. Empty for a type the project
    /// does not define, where the method is the library's. Rust only (a PHP/TS class inherits
    /// statics from its parent), and only when every definition of the name sits in a trait or a
    /// project type: a blanket `impl<T> Ext for T` or `impl Ext for Box<Foo>` has a container we
    /// cannot tie to `ty`, so nothing is ruled out then.
    fn trait_members(&self, from: &str, name: &str, written: &str, defs: &[Def]) -> Vec<usize> {
        // Derives and std traits make these without any definition in the project.
        const STD_NAMES: &[&str] = &[
            "default",
            "clone",
            "from",
            "into",
            "fmt",
            "eq",
            "cmp",
            "hash",
            "partial_cmp",
            "drop",
            "try_from",
            "to_string",
            "ne",
            "lt",
            "le",
            "gt",
            "ge",
            "max",
            "min",
            "clamp",
            "clone_from",
            "to_owned",
            "borrow",
            "as_ref",
            "type_id",
            "try_into",
            "borrow_mut",
        ];
        let ty = plain_type(written);
        let known = |c: &String| self.traits.contains(c) || self.nominal.contains(c);
        let opaque = defs
            .iter()
            .any(|d| d.container.as_ref().is_some_and(|c| !known(c)));
        if !from.ends_with(".rs")
            || STD_NAMES.contains(&name)
            || self.paths.type_files(from, written).is_empty()
            || opaque
            || !self.nominal.contains(ty)
            || self.aliased.contains(ty)
        {
            return Vec::new();
        }
        (0..defs.len())
            .filter(|&i| {
                defs[i]
                    .container
                    .as_ref()
                    .is_some_and(|c| self.traits.contains(c))
            })
            .collect()
    }

    /// `narrow`, but only among the definitions at positions `subset`.
    fn narrow_among(&self, from: &str, defs: &[Def], subset: Vec<usize>) -> Vec<usize> {
        let here = folder(from);
        Tier::ALL
            .iter()
            .map(|&tier| {
                subset
                    .iter()
                    .copied()
                    .filter(|&i| self.is_in(tier, from, here, &defs[i].path))
                    .collect::<Vec<_>>()
            })
            .find(|hits| !hits.is_empty())
            .unwrap_or(subset)
    }

    fn is_in(&self, tier: Tier, from: &str, here: &str, path: &str) -> bool {
        match tier {
            Tier::SameFile => path == from,
            Tier::Imported => self.imports.get(from).is_some_and(|set| set.contains(path)),
            Tier::SameFolder => folder(path) == here,
        }
    }

    /// The definitions in one tier alone (empty when it has none). `from` is an index path.
    pub fn in_tier(&self, tier: Tier, from: &str, defs: &[Def]) -> Vec<usize> {
        let here = folder(from);
        (0..defs.len())
            .filter(|&i| self.is_in(tier, from, here, &defs[i].path))
            .collect()
    }
}

/// Is the code file `time.rs` / `time/mod.rs` / anything under a folder `time`? (`m` is a module
/// name.) Docs, JSON and other non-code files are never a module.
fn in_module(path: &str, m: &str) -> bool {
    if matches!(Lang::from_path(path), None | Some(Lang::Markdown)) {
        return false;
    }
    let (folders, file) = path.rsplit_once('/').unwrap_or(("", path));
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    stem == m || folders.split('/').any(|dir| dir == m)
}

/// Languages where a bare `f()` inside a class can mean `this.f()`.
fn implicit_this(path: &str) -> bool {
    matches!(
        Lang::from_path(path),
        Some(Lang::Kotlin | Lang::Java | Lang::CSharp | Lang::Swift | Lang::Ruby | Lang::Cpp)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileEntry, Form};

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
        places
            .iter()
            .map(|(p, l)| Def {
                path: p.to_string(),
                line: *l,
                container: None,
                returns: None,
            })
            .collect()
    }

    fn in_type(path: &str, line: u32, container: &str) -> Def {
        Def {
            path: path.into(),
            line,
            container: Some(container.into()),
            returns: None,
        }
    }

    fn call(form: Form, owner: Option<&str>) -> Call {
        Call {
            callee: "f".into(),
            line: 1,
            caller: Some("g".into()),
            form,
            owner: owner.map(String::from),
        }
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

    #[test]
    fn use_paths_pick_the_defining_file_and_fall_back_when_unknown() {
        use crate::model::{Kind, Symbol, Use};
        let sym = |n: &str, k: Kind, c: Option<&str>| Symbol {
            name: n.into(),
            kind: k,
            line: 1,
            signature: String::new(),
            container: c.map(String::from),
            returns: None,
        };
        let entry = |path: &str, uses: &[(&str, &str)], symbols: Vec<Symbol>| FileEntry {
            path: path.into(),
            uses: uses
                .iter()
                .map(|(a, p)| Use {
                    alias: a.to_string(),
                    path: p.to_string(),
                    public: true,
                    local: false,
                })
                .collect(),
            symbols,
            ..Default::default()
        };
        let r = Resolver::new(&index(vec![
            entry("tok/src/lib.rs", &[("f", "m::f")], vec![]),
            entry("tok/src/m.rs", &[], vec![sym("f", Kind::Fn, None)]),
            entry("tok/src/b.rs", &[("Bld", "crate::m::Bld")], vec![]),
            entry(
                "app/src/main.rs",
                &[("f", "tok::f"), ("Bld", "tok::m::Bld")],
                vec![],
            ),
        ]));
        let d = defs(&[("other/src/lib.rs", 1), ("tok/src/m.rs", 1)]);
        let bare = |from: &str| r.resolve(from, &call(Form::Bare, None), &d);
        assert_eq!(bare("app/src/main.rs"), vec![1], "use tok::f; f()");
        assert_eq!(bare("nowhere.rs"), vec![0, 1], "no use: the old rules");
        let module =
            |m: &str| r.resolve("app/src/main.rs", &call(Form::Module(m.into()), None), &d);
        assert_eq!(module("tok"), vec![1], "tok::f()");
        assert_eq!(module("zzz"), vec![0, 1], "unknown module: the old rules");
    }

    #[test]
    fn a_type_call_drops_rival_types_but_keeps_trait_impls_elsewhere() {
        use crate::model::{Kind, Symbol, Use};
        let strukt = Symbol {
            name: "Bld".into(),
            kind: Kind::Struct,
            line: 1,
            signature: String::new(),
            container: None,
            returns: None,
        };
        let r = Resolver::new(&index(vec![
            FileEntry {
                path: "tok/src/bld.rs".into(),
                symbols: vec![strukt],
                ..Default::default()
            },
            FileEntry {
                path: "tok/src/lib.rs".into(),
                ..Default::default()
            },
            // another package with its own `Bld`
            FileEntry {
                path: "rival/src/lib.rs".into(),
                symbols: vec![Symbol {
                    name: "Bld".into(),
                    kind: Kind::Struct,
                    line: 1,
                    signature: String::new(),
                    container: None,
                    returns: None,
                }],
                ..Default::default()
            },
            FileEntry {
                path: "app/src/main.rs".into(),
                uses: vec![Use {
                    alias: "Bld".into(),
                    path: "tok::bld::Bld".into(),
                    public: false,
                    local: false,
                }],
                ..Default::default()
            },
        ]));
        let d = vec![
            in_type("rival/src/x.rs", 1, "Bld"),
            in_type("tok/src/bld.rs", 2, "Bld"),
            in_type("app/src/main.rs", 3, "Bld"), // `impl Trait for tok::Bld` downstream
        ];
        let c = call(Form::Type("Bld".into()), None);
        assert_eq!(r.resolve("app/src/main.rs", &c, &d), vec![1, 2]);
        assert_eq!(r.resolve("elsewhere.rs", &c, &d), vec![0, 1, 2]);
    }

    #[test]
    fn nested_packages_do_not_rival_each_other_by_folder_prefix() {
        use crate::model::{Kind, Symbol, Use};
        let strukt = |path: &str| FileEntry {
            path: path.into(),
            symbols: vec![Symbol {
                name: "Bld".into(),
                kind: Kind::Struct,
                line: 1,
                signature: String::new(),
                container: None,
                returns: None,
            }],
            ..Default::default()
        };
        let r = Resolver::new(&index(vec![
            strukt("tok/src/lib.rs"),
            strukt("tok/sub/src/lib.rs"),
            FileEntry {
                path: "app/src/main.rs".into(),
                uses: vec![Use {
                    alias: "Bld".into(),
                    path: "tok::sub::Bld".into(),
                    public: false,
                    local: false,
                }],
                ..Default::default()
            },
        ]));
        let d = vec![
            in_type("tok/src/lib.rs", 1, "Bld"),
            in_type("tok/sub/src/lib.rs", 2, "Bld"),
            in_type("app/src/main.rs", 3, "Bld"),
        ];
        let c = call(Form::Type("Bld".into()), None);
        let got = r.resolve("app/src/main.rs", &c, &d);
        assert!(
            got.contains(&1),
            "the imported package's member stays: {got:?}"
        );
    }

    #[test]
    fn a_project_type_without_the_member_gets_it_from_a_trait() {
        use crate::model::{Kind, Symbol};
        let sym = |name: &str, kind: Kind| Symbol {
            name: name.into(),
            kind,
            line: 1,
            signature: String::new(),
            container: None,
            returns: None,
        };
        let r = Resolver::new(&index(vec![FileEntry {
            path: "t.rs".into(),
            symbols: vec![
                sym("Ext", Kind::Trait),
                sym("TcpStream", Kind::Struct),
                sym("Other", Kind::Struct),
                sym("Cursor", Kind::Struct),
            ],
            ..Default::default()
        }]));
        let d = vec![
            in_type("ext.rs", 1, "Ext"),
            in_type("other.rs", 2, "Other"),
            in_type("tcp.rs", 3, "Cursor"),
        ];
        let go = |ty: &str| r.resolve("t.rs", &call(Form::Type(ty.into()), None), &d);
        assert_eq!(go("TcpStream"), vec![0], "extension trait method");
        let blanket = vec![in_type("ext.rs", 1, "Ext"), in_type("blanket.rs", 2, "T")];
        let c = call(Form::Type("TcpStream".into()), None);
        assert_eq!(
            r.resolve("t.rs", &c, &blanket),
            vec![0, 1],
            "`impl<T> Ext for T`: nothing is ruled out"
        );
        assert_eq!(
            r.resolve("t.php", &c, &d),
            vec![0, 1, 2],
            "not Rust: no trait fallback"
        );
        assert_eq!(
            go("Vec"),
            vec![0, 1, 2],
            "a library type: nothing is ruled out"
        );
        assert_eq!(go("Other"), vec![1], "its own member wins");
        let c = call(Form::Type("TcpStream".into()), None);
        let mut std = c.clone();
        std.callee = "default".into();
        assert_eq!(r.resolve("t.rs", &std, &d), vec![0, 1, 2], "derivable name");
        let foreign = Resolver::new(&index(vec![FileEntry {
            path: "t.rs".into(),
            symbols: vec![sym("Ext", Kind::Trait), sym("Other", Kind::Struct)],
            uses: vec![crate::model::Use {
                alias: "Cursor".into(),
                path: "serde::Cursor".into(),
                public: false,
                local: false,
            }],
            ..Default::default()
        }]));
        let c = call(Form::Type("Cursor".into()), None);
        assert_eq!(
            foreign.resolve("t.rs", &c, &d[..2]),
            vec![0, 1],
            "unfollowable import"
        );
        let aliased = Resolver::new(&index(vec![FileEntry {
            path: "t.rs".into(),
            symbols: vec![sym("Ext", Kind::Trait), sym("Drv", Kind::Type)],
            ..Default::default()
        }]));
        let c = call(Form::Type("Drv".into()), None);
        assert_eq!(
            aliased.resolve("t.rs", &c, &d),
            vec![0, 1, 2],
            "a type alias"
        );
    }

    #[test]
    fn qualified_type_paths_narrow_by_file_and_unfollowable_ones_keep_all() {
        use crate::model::{Kind, Symbol, Use};
        let strukt = |path: &str, uses: Vec<Use>| FileEntry {
            path: path.into(),
            uses,
            symbols: vec![Symbol {
                name: "D".into(),
                kind: Kind::Struct,
                line: 1,
                signature: String::new(),
                container: None,
                returns: None,
            }],
            ..Default::default()
        };
        let io = Use {
            alias: "io".into(),
            path: "crate::io".into(),
            public: false,
            local: false,
        };
        let r = Resolver::new(&index(vec![
            FileEntry {
                path: "p/src/lib.rs".into(),
                ..Default::default()
            },
            strukt("p/src/io.rs", vec![]),
            strukt("p/src/d.rs", vec![io]),
        ]));
        let d = vec![
            in_type("p/src/io.rs", 1, "D"),
            in_type("p/src/d.rs", 2, "D"),
        ];
        let go = |t: &str| r.resolve("p/src/d.rs", &call(Form::Type(t.into()), None), &d);
        assert_eq!(go("io::D"), vec![0], "the module path picks the imported D");
        assert_eq!(go("D"), vec![1], "unqualified: the file's own D");
        assert_eq!(go("nope::D"), vec![0, 1], "unfollowable: nothing ruled out");
        assert_eq!(
            go("Self::Item"),
            vec![0, 1],
            "an associated type says nothing"
        );
    }

    #[test]
    fn a_returned_class_means_the_type_of_the_file_that_returns_it() {
        use crate::model::{Kind, Ret, Symbol};
        let sym = |name: &str, kind: Kind, returns: Option<Ret>| Symbol {
            name: name.into(),
            kind,
            line: 1,
            signature: String::new(),
            container: None,
            returns,
        };
        let ret = Ret {
            ty: "Sender".into(),
            ..Default::default()
        };
        let r = Resolver::new(&index(vec![
            FileEntry {
                path: "a.rs".into(),
                symbols: vec![
                    sym("Sender", Kind::Struct, None),
                    sym("channel", Kind::Fn, Some(ret)),
                ],
                ..Default::default()
            },
            FileEntry {
                path: "b.rs".into(),
                symbols: vec![sym("Sender", Kind::Struct, None)],
                ..Default::default()
            },
            FileEntry {
                path: "use.rs".into(),
                ..Default::default()
            },
        ]));
        let d = vec![in_type("a.rs", 1, "Sender"), in_type("b.rs", 2, "Sender")];
        let hop = Hop {
            callee: "channel".into(),
            form: Form::Bare,
            ..Default::default()
        };
        let c = call(Form::Returned(Box::new(hop)), None);
        // `channel` is defined in a.rs, whose `Sender` is the one returned: b.rs's is a rival.
        assert_eq!(r.resolve("a.rs", &c, &d), vec![0]);
    }

    #[test]
    fn a_member_is_dropped_only_when_it_is_a_rival_from_every_returning_file() {
        use crate::model::{Kind, Symbol};
        let strukt = |path: &str| FileEntry {
            path: path.into(),
            symbols: vec![Symbol {
                name: "Sender".into(),
                kind: Kind::Struct,
                line: 1,
                signature: String::new(),
                container: None,
                returns: None,
            }],
            ..Default::default()
        };
        let r = Resolver::new(&index(vec![strukt("a.rs"), strukt("b.rs"), strukt("c.rs")]));
        let d = vec![
            in_type("a.rs", 1, "Sender"),
            in_type("b.rs", 2, "Sender"),
            in_type("c.rs", 3, "Sender"),
        ];
        let typed = Typed {
            ty: "Sender".into(),
            froms: vec!["a.rs".into(), "c.rs".into()],
        };
        // b.rs is a rival from both returning files; a.rs and c.rs each belong to one of them.
        assert_eq!(r.members_typed(&d, &typed), vec![0, 2]);
        // From a file that returns nothing known, every member is a rival: the set never empties.
        let lost = Typed {
            ty: "Sender".into(),
            froms: vec!["nowhere.rs".into()],
        };
        assert_eq!(r.members_typed(&d, &lost), vec![0, 1, 2]);
    }

    #[test]
    fn self_calls_go_to_methods_of_the_enclosing_type() {
        let r = Resolver::new(&index(vec![file("a.rs", &[])]));
        let d = vec![
            in_type("a.rs", 1, "A"),
            in_type("b.rs", 2, "B"),
            in_type("c.rs", 3, "A"),
        ];
        assert_eq!(
            r.resolve("a.rs", &call(Form::SelfRef, Some("A")), &d),
            vec![0, 2]
        );
        // No such method in the enclosing type (it may come from a trait): nothing is ruled out.
        assert_eq!(
            r.resolve("a.rs", &call(Form::SelfRef, Some("Z")), &d),
            vec![0, 1, 2]
        );
        assert_eq!(
            r.resolve("a.rs", &call(Form::SelfRef, None), &d),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn type_calls_go_to_members_of_that_type() {
        let r = Resolver::new(&index(vec![file("a.rs", &[])]));
        let d = vec![
            in_type("a.rs", 1, "A"),
            in_type("b.rs", 2, "B"),
            in_type("c.rs", 3, "B"),
        ];
        let b = call(Form::Type("B".into()), Some("A"));
        assert_eq!(r.resolve("a.rs", &b, &d), vec![1, 2]);
        let unknown = call(Form::Type("Q".into()), Some("A"));
        assert_eq!(r.resolve("a.rs", &unknown, &d), vec![0, 1, 2]);
    }

    #[test]
    fn value_dot_method_calls_skip_free_functions_and_ignore_the_caller_s_file() {
        let r = Resolver::new(&index(vec![file("a.rs", &[])]));
        let mut d = defs(&[("a.rs", 1)]);
        d.push(in_type("a.rs", 5, "A"));
        d.push(in_type("far/b.rs", 9, "B"));
        assert_eq!(
            r.resolve("a.rs", &call(Form::Method, Some("A")), &d),
            vec![1, 2]
        );
    }

    #[test]
    fn bare_and_module_calls_go_to_free_functions_best_tier_first() {
        let r = Resolver::new(&index(vec![file("src/a.rs", &[])]));
        let mut d = defs(&[("src/z/b.rs", 1), ("src/a.rs", 4)]);
        d.push(in_type("src/a.rs", 7, "A"));
        assert_eq!(
            r.resolve("src/a.rs", &call(Form::Bare, Some("A")), &d),
            vec![1]
        );
        assert_eq!(
            r.resolve("src/a.rs", &call(Form::Module("a".into()), None), &d),
            vec![1]
        );
        // Only methods exist: a free-function call has nothing to narrow to.
        let only_methods = vec![in_type("x.rs", 1, "A"), in_type("y.rs", 2, "B")];
        assert_eq!(
            r.resolve("a.rs", &call(Form::Bare, None), &only_methods),
            vec![0, 1]
        );
    }

    #[test]
    fn module_calls_prefer_files_of_that_module() {
        let r = Resolver::new(&index(vec![file("src/main.rs", &[])]));
        let d = defs(&[
            ("src/main.rs", 1),
            ("src/time.rs", 2),
            ("src/io/time/mod.rs", 3),
            ("src/sleep.rs", 4),
        ]);
        let time = call(Form::Module("time".into()), None);
        assert_eq!(r.resolve("src/main.rs", &time, &d), vec![1, 2]);
        // A module name no file carries: the tiers decide (own file first).
        let none = call(Form::Module("net".into()), None);
        assert_eq!(r.resolve("src/main.rs", &none, &d), vec![0]);
    }

    #[test]
    fn languages_with_an_implicit_this_also_let_bare_calls_reach_the_enclosing_type() {
        let r = Resolver::new(&index(vec![file("A.kt", &[])]));
        let d = vec![in_type("A.kt", 1, "A"), in_type("B.kt", 2, "B")];
        assert_eq!(r.resolve("A.kt", &call(Form::Bare, Some("A")), &d), vec![0]);
        assert_eq!(
            r.resolve("a.rs", &call(Form::Bare, Some("A")), &d),
            vec![0, 1],
            "Rust has no implicit self"
        );
    }

    #[test]
    fn unknown_calls_are_not_narrowed() {
        let r = Resolver::new(&index(vec![file("a.rs", &[])]));
        let d = defs(&[("a.rs", 1), ("b.rs", 2)]);
        assert_eq!(
            r.resolve("a.rs", &call(Form::Unknown, None), &d),
            vec![0, 1]
        );
    }

    #[test]
    fn module_names_match_whole_stems_of_code_files_only() {
        assert!(in_module("src/time.rs", "time"));
        assert!(in_module("src/time/mod.rs", "time"));
        assert!(in_module("a/time/x/y.rs", "time"));
        assert!(!in_module("src/time.md", "time"));
        assert!(!in_module("src/time.json", "time"));
        assert!(!in_module("src/timer.rs", "time"));
        assert!(!in_module("v1.2.rs", "v1"));
        assert!(!in_module("time", "time"), "no extension: not a code file");
    }

    #[test]
    fn bare_calls_with_an_implicit_this_keep_sorted_unique_candidates() {
        let r = Resolver::new(&index(vec![file("A.kt", &[])]));
        let mut d = defs(&[("A.kt", 1), ("far/free.kt", 2)]);
        d.push(in_type("A.kt", 3, "A"));
        d.push(in_type("B.kt", 4, "B"));
        // Free functions (0, 1) plus the owner's methods (2); the tiers then keep the own file's.
        assert_eq!(
            r.resolve("A.kt", &call(Form::Bare, Some("A")), &d),
            vec![0, 2]
        );
    }

    fn hop(callee: &str, form: Form, unwrapped: bool) -> Box<Hop> {
        Box::new(Hop {
            callee: callee.into(),
            form,
            unwrapped,
            awaited: false,
            ..Default::default()
        })
    }

    /// (name, container, (returned class, fallible, async))
    type Sym<'a> = (&'a str, Option<&'a str>, Option<(&'a str, bool, bool)>);

    fn resolver_with(symbols: &[Sym]) -> Resolver {
        use crate::model::{Kind, Symbol};
        let mut f = file("src/a.rs", &[]);
        f.symbols = symbols
            .iter()
            .enumerate()
            .map(|(i, (name, container, returns))| Symbol {
                name: name.to_string(),
                kind: Kind::Fn,
                line: i as u32 + 1,
                signature: String::new(),
                container: container.map(String::from),
                returns: returns.map(|(ty, fallible, future)| {
                    // "A,B" stands for a tuple return `(A, B)`.
                    let tuple = ty.contains(',');
                    Ret {
                        ty: if tuple { String::new() } else { ty.into() },
                        parts: if tuple {
                            ty.split(',').map(String::from).collect()
                        } else {
                            Vec::new()
                        },
                        fallible,
                        future,
                    }
                }),
            })
            .collect();
        Resolver::new(&index(vec![f]))
    }

    fn chained(form: Form) -> Call {
        call(form, None)
    }

    #[test]
    fn a_call_chain_is_followed_through_the_return_types() {
        // Builder::new() -> Builder; Builder::enable() -> Builder; build exists on two types.
        let r = resolver_with(&[
            ("new", Some("Builder"), Some(("Builder", false, false))),
            ("enable", Some("Builder"), Some(("Builder", false, false))),
            ("new", Some("Other"), Some(("Other", false, false))),
        ]);
        let d = vec![
            in_type("src/a.rs", 90, "Builder"),
            in_type("src/b.rs", 91, "Other"),
        ];
        let one = chained(Form::Returned(hop(
            "new",
            Form::Type("Builder".into()),
            false,
        )));
        assert_eq!(r.resolve("x.rs", &one, &d), vec![0]);
        let two = chained(Form::Returned(hop(
            "enable",
            Form::Returned(hop("new", Form::Type("Builder".into()), false)),
            false,
        )));
        assert_eq!(r.resolve("x.rs", &two, &d), vec![0]);
    }

    #[test]
    fn unwrapping_must_match_a_fallible_return() {
        let r = resolver_with(&[
            ("build", Some("Builder"), Some(("Runtime", true, false))),
            ("make", Some("Builder"), Some(("Runtime", false, false))),
        ]);
        let d = vec![in_type("a.rs", 1, "Runtime"), in_type("b.rs", 2, "Other")];
        let builder = || Form::Type("Builder".into());
        let unwrapped = chained(Form::Returned(hop("build", builder(), true)));
        assert_eq!(r.resolve("x.rs", &unwrapped, &d), vec![0]);
        let forgot = chained(Form::Returned(hop("build", builder(), false)));
        assert_eq!(
            r.resolve("x.rs", &forgot, &d),
            vec![0, 1],
            "a Result's own method"
        );
        let needless = chained(Form::Returned(hop("make", builder(), true)));
        assert_eq!(r.resolve("x.rs", &needless, &d), vec![0, 1]);
    }

    #[test]
    fn a_chain_with_unknown_or_disagreeing_steps_is_a_plain_method_call() {
        let r = resolver_with(&[
            ("a", Some("X"), Some(("One", false, false))),
            ("a", Some("Y"), Some(("Two", false, false))),
            ("untyped", None, None),
        ]);
        let d = vec![
            in_type("a.rs", 1, "One"),
            in_type("b.rs", 2, "Two"),
            defs(&[("free.rs", 3)]).remove(0),
        ];
        // `a` is on X and Y, which return different classes: unknown.
        let split = chained(Form::Returned(hop("a", Form::Method, false)));
        assert_eq!(r.resolve("x.rs", &split, &d), vec![0, 1], "methods only");
        let no_return = chained(Form::Returned(hop("untyped", Form::Bare, false)));
        assert_eq!(r.resolve("x.rs", &no_return, &d), vec![0, 1]);
        let nobody = chained(Form::Returned(hop("missing", Form::Bare, false)));
        assert_eq!(r.resolve("x.rs", &nobody, &d), vec![0, 1]);
    }

    #[test]
    fn a_hop_with_an_uncertain_form_is_not_trusted() {
        // `map.get(k).unwrap().run()` on a std map must not pin to the project's `get -> Foo`.
        let r = resolver_with(&[("get", Some("Cache"), Some(("Foo", true, false)))]);
        let d = vec![in_type("a.rs", 1, "Foo"), in_type("b.rs", 2, "Other")];
        for form in [Form::Method, Form::Unknown] {
            let c = chained(Form::Returned(hop("get", form, true)));
            assert_eq!(r.resolve("x.rs", &c, &d), vec![0, 1]);
        }
        // The same call written `Cache::get(..)` is certain.
        let sure = chained(Form::Returned(hop("get", Form::Type("Cache".into()), true)));
        assert_eq!(r.resolve("x.rs", &sure, &d), vec![0]);
    }

    #[test]
    fn a_step_that_did_not_resolve_poisons_the_rest_of_the_chain() {
        let r = resolver_with(&[
            ("get", Some("Cache"), Some(("Foo", false, false))),
            ("next", Some("Foo"), Some(("Foo", false, false))),
        ]);
        let d = vec![in_type("a.rs", 1, "Foo"), in_type("b.rs", 2, "Other")];
        let inner = Form::Returned(hop("get", Form::Method, false));
        let c = chained(Form::Returned(hop("next", inner, false)));
        assert_eq!(r.resolve("x.rs", &c, &d), vec![0, 1]);
    }

    #[test]
    fn a_returned_class_with_no_members_falls_back_to_methods_only() {
        // `-> Vec<Foo>`: the class `Vec` has no project members; free functions must not join in.
        let r = resolver_with(&[("items", Some("Cache"), Some(("Vec", false, false)))]);
        let d = vec![in_type("a.rs", 1, "Foo"), defs(&[("free.rs", 2)]).remove(0)];
        let c = chained(Form::Returned(hop(
            "items",
            Form::Type("Cache".into()),
            false,
        )));
        assert_eq!(r.resolve("x.rs", &c, &d), vec![0]);
    }

    #[test]
    fn a_bare_or_module_step_needs_a_real_free_function() {
        // `open()` may be an imported external fn; the project only has a method `open`.
        let r = resolver_with(&[("open", Some("Foo"), Some(("Foo", false, false)))]);
        let d = vec![in_type("a.rs", 1, "Foo"), in_type("b.rs", 2, "Other")];
        for form in [Form::Bare, Form::Module("fs".into())] {
            let c = chained(Form::Returned(hop("open", form, false)));
            assert_eq!(r.resolve("x.rs", &c, &d), vec![0, 1]);
        }
        let own = Call {
            owner: Some("Bar".into()),
            ..chained(Form::Returned(hop("open", Form::SelfRef, false)))
        };
        assert_eq!(r.resolve("x.rs", &own, &d), vec![0, 1], "Bar has no `open`");
    }

    #[test]
    fn a_tuple_part_of_a_return_is_the_receiver_type() {
        let r = resolver_with(&[("channel", None, Some(("Sender,Receiver", false, false)))]);
        let d = vec![in_type("a.rs", 1, "Sender"), in_type("b.rs", 2, "Receiver")];
        let part = |n: Option<u32>| {
            let mut h = hop("channel", Form::Bare, false);
            h.part = n;
            chained(Form::Returned(h))
        };
        assert_eq!(r.resolve("x.rs", &part(Some(0)), &d), vec![0]);
        assert_eq!(r.resolve("x.rs", &part(Some(1)), &d), vec![1]);
        assert_eq!(
            r.resolve("x.rs", &part(Some(7)), &d),
            vec![0, 1],
            "no such part"
        );
        assert_eq!(
            r.resolve("x.rs", &part(None), &d),
            vec![0, 1],
            "a tuple has no methods"
        );
    }

    #[test]
    fn a_very_long_chain_resolves_without_blowing_up() {
        let r = resolver_with(&[
            ("new", Some("B"), Some(("B", false, false))),
            ("step", Some("B"), Some(("B", false, false))),
        ]);
        let d = vec![in_type("a.rs", 1, "B"), in_type("b.rs", 2, "Other")];
        let mut form = Form::Returned(hop("new", Form::Type("B".into()), false));
        for _ in 0..300 {
            form = Form::Returned(hop("step", form, false));
        }
        let started = std::time::Instant::now();
        // Past the depth cap the chain is just a method call: still contains the truth.
        assert!(r.resolve("x.rs", &chained(form), &d).contains(&0));
        assert!(started.elapsed().as_secs() < 2);
    }

    #[test]
    fn awaiting_matches_an_async_fn_only() {
        let r = resolver_with(&[
            ("connect", Some("Stream"), Some(("Stream", true, true))),
            ("sync", Some("Stream"), Some(("Stream", false, false))),
        ]);
        let d = vec![in_type("a.rs", 1, "Stream"), in_type("b.rs", 2, "Other")];
        let stream = || Form::Type("Stream".into());
        let step = |callee: &str, unwrapped, awaited| {
            let mut h = hop(callee, stream(), unwrapped);
            h.awaited = awaited;
            chained(Form::Returned(h))
        };
        assert_eq!(r.resolve("x.rs", &step("connect", true, true), &d), vec![0]);
        assert_eq!(
            r.resolve("x.rs", &step("connect", true, false), &d),
            vec![0, 1]
        );
        assert_eq!(
            r.resolve("x.rs", &step("sync", false, true), &d),
            vec![0, 1]
        );
        assert_eq!(r.resolve("x.rs", &step("sync", false, false), &d), vec![0]);
    }
}
