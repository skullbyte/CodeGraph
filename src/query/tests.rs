use super::*;
use crate::lang::{self, Lang};
use crate::model::{FileEntry, ResolvedCall};

fn index_of(files: &[(&str, &str)]) -> Index {
    let mut index = Index {
        aliases: BTreeMap::from([("@/".to_string(), "src/".to_string())]),
        ..Index::default()
    };
    for (path, src) in files {
        let lang = Lang::from_path(path).unwrap();
        let facts = lang::extract(lang, src).unwrap();
        index.files.insert(
            path.to_string(),
            FileEntry {
                path: path.to_string(),
                symbols: facts.symbols,
                imports: facts.imports,
                calls: facts.calls,
                ..FileEntry::default()
            },
        );
    }
    index
}

#[test]
fn rust_module_fallback_needs_exactly_one_suffix_match() {
    let files: BTreeMap<String, ()> = ["a/x.rs", "b/y/mod.rs", "c/z.rs", "d/z.rs", "main.rs"]
        .iter()
        .map(|p| (p.to_string(), ()))
        .collect();
    let find = |imp: &str| resolve_rust_module(&files, "main.rs", imp);
    assert_eq!(find("x"), Some("a/x.rs".to_string()));
    assert_eq!(find("y"), Some("b/y/mod.rs".to_string()));
    assert_eq!(find("z"), None);
    assert_eq!(find("w"), None);
}

#[test]
fn docs_never_count_as_code() {
    let index = index_of(&[
        ("a.rs", "pub fn foo() {}\npub fn bar() { foo(); }\n"),
        (
            "README.md",
            "# foo\n\n## Usage\nCall `foo()` and `nothing_here`.\nSee `bar`.\n",
        ),
    ]);
    // A heading is not a definition.
    assert_eq!(def(&index, "foo").len(), 1);
    assert_eq!(definition_counts(&index)["foo"], 1);
    // A doc mention is listed and marked, only for a name with one code definition.
    let got: Vec<_> = callers(&index, "foo").iter().map(CallerHit::text).collect();
    assert_eq!(
        got,
        vec!["a.rs:2  in pub fn bar", "README.md:4  in Usage  [doc]"]
    );
    assert!(callers(&index, "nothing_here").is_empty());
    // And it stays out of the code graph.
    assert!(!call_edges(&index).contains_key("Usage"));
    let hit: Vec<_> = affected(&index, "foo", None)
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(hit, vec!["bar"]);
    assert!(hubs(&index, 10).iter().all(|h| h.name != "Usage"));
}

#[test]
fn def_lists_all_candidates_and_hides_impls() {
    let index = index_of(&[
        ("a.rs", "pub struct Foo;\nimpl Foo { fn new() {} }\n"),
        ("b.ts", "export class Foo {}\n"),
    ]);
    let got: Vec<_> = def(&index, "Foo").iter().map(DefHit::text).collect();
    assert_eq!(
        got,
        vec!["a.rs:1 pub struct Foo;", "b.ts:1 export class Foo"]
    );
}

#[test]
fn def_falls_back_to_impl() {
    let index = index_of(&[("a.rs", "impl Foo {}\n")]);
    assert_eq!(def(&index, "Foo")[0].text(), "a.rs:1 impl Foo");
}

#[test]
fn callers_report_enclosing_fn() {
    let index = index_of(&[(
        "a.rs",
        "fn a() {}\nfn b() {\n    a();\n}\nfn c() { x.a(); }\n",
    )]);
    let got: Vec<_> = callers(&index, "a").iter().map(CallerHit::text).collect();
    assert_eq!(got, vec!["a.rs:3  in fn b", "a.rs:5  in fn c"]);
}

#[test]
fn callers_say_which_definition_a_typed_call_reaches() {
    let index = index_of(&[(
        "a.rs",
        "struct A;\nstruct B;\nimpl A { fn new() {} }\nimpl B { fn new() {} }\n\
             fn one() { A::new(); }\nfn two() { B::new(); }\nfn three() { x.new(); }\n",
    )]);
    let got: Vec<_> = callers(&index, "new").iter().map(CallerHit::text).collect();
    assert_eq!(
        got,
        vec![
            "a.rs:5  in fn one  -> a.rs:3",
            "a.rs:6  in fn two  -> a.rs:4",
            "a.rs:7  in fn three",
        ]
    );
}

const TWO_NEWS: &str = "struct A;\nstruct B;\nimpl A { fn new() {} }\nimpl B { fn new() {} }\n\
         fn one() { A::new(); }\nfn two() { B::new(); }\nfn three() { x.new(); }\n";

#[test]
fn a_scip_answer_that_names_no_current_definition_is_not_shown() {
    let mut index = index_of(&[("a.rs", TWO_NEWS)]);
    // Call 0 is `A::new()`; SCIP (an old run) says line 99, where nothing is defined now.
    resolve(&mut index, "a.rs", 0, def_at("a.rs", 99));
    let got: Vec<_> = callers(&index, "new").iter().map(CallerHit::text).collect();
    assert_eq!(got[0], "a.rs:5  in fn one  -> a.rs:3", "the form decides");
}

#[test]
fn a_def_selector_names_one_definition_or_lists_the_choices() {
    let index = index_of(&[("a.rs", TWO_NEWS)]);
    assert_eq!(select_def(&index, "new", "a.rs:4").unwrap().line, 4);
    let err = select_def(&index, "new", "a.rs").unwrap_err();
    assert!(err.contains("a.rs:3") && err.contains("a.rs:4"), "{err}");
    assert!(select_def(&index, "new", "a.rs:9").is_err());
    assert!(select_def(&index, "new", "b.rs").is_err());
    assert!(select_def(&index, "nothing", "a.rs").is_err());
}

#[test]
fn callers_of_one_definition_keep_its_sites_and_the_unsure_ones() {
    let index = index_of(&[("a.rs", TWO_NEWS)]);
    let only = select_def(&index, "new", "a.rs:3").unwrap();
    let got: Vec<_> = callers_of(&index, "new", &only)
        .iter()
        .map(CallerHit::text)
        .collect();
    // `B::new()` is pinned to the other definition and goes; `x.new()` could be either.
    assert_eq!(
        got,
        vec!["a.rs:5  in fn one  -> a.rs:3", "a.rs:7  in fn three"]
    );
}

#[test]
fn explain_of_one_definition_reads_the_body_of_that_owner_only() {
    let index = index_of(&[(
        "a.rs",
        "struct A;\nstruct B;\nimpl A { fn run(&self) { one(); } }\n\
             impl B { fn run(&self) { two(); } }\nfn one() {}\nfn two() {}\n",
    )]);
    let only = select_def(&index, "run", "a.rs:3").unwrap();
    let out: Vec<_> = explain_of(&index, "run", &only)
        .outgoing
        .iter()
        .map(|l| l.name.clone())
        .collect();
    assert_eq!(out, vec!["one"]);
}

#[test]
fn a_def_selector_accepts_a_leading_dot_slash() {
    let index = index_of(&[("a.rs", TWO_NEWS)]);
    assert_eq!(select_def(&index, "new", "./a.rs:3").unwrap().line, 3);
}

#[test]
fn explain_of_one_definition_counts_only_its_callers() {
    let index = index_of(&[("a.rs", TWO_NEWS)]);
    let only = select_def(&index, "new", "a.rs:4").unwrap();
    let e = explain_of(&index, "new", &only);
    assert_eq!(e.defs.len(), 1);
    let who: Vec<_> = e.incoming.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(who, vec!["three", "two"]);
}

#[test]
fn path_follows_calls() {
    let index = index_of(&[(
        "a.rs",
        "fn a() { b(); }\nfn b() { c(); }\nfn c() {}\nfn d() {}\n",
    )]);
    let names: Vec<_> = path(&index, "a", "c").into_iter().map(|h| h.name).collect();
    assert_eq!(names, vec!["a", "b", "c"]);
    assert_eq!(path(&index, "a", "c")[2].text(), "c  a.rs:3");
    assert!(path(&index, "a", "d").is_empty());
}

#[test]
fn path_follows_imports() {
    let index = index_of(&[
        ("src/app.ts", "import { x } from './lib/x';\n"),
        ("src/lib/x.ts", "import { y } from '../util';\n"),
        ("src/util.ts", "export const y = 1;\n"),
        ("src/lone.ts", "export const z = 1;\n"),
    ]);
    let got: Vec<_> = path(&index, "src/app.ts", "src/util.ts")
        .into_iter()
        .map(|h| h.name)
        .collect();
    assert_eq!(got, vec!["src/app.ts", "src/lib/x.ts", "src/util.ts"]);
    assert!(path(&index, "src/app.ts", "src/lone.ts").is_empty());
}

#[test]
fn path_follows_rust_module_imports() {
    let index = index_of(&[
        ("src/main.rs", "mod walk;\n"),
        ("src/walk.rs", "fn w() {}\n"),
    ]);
    let got: Vec<_> = path(&index, "src/main.rs", "src/walk.rs")
        .into_iter()
        .map(|h| h.name)
        .collect();
    assert_eq!(got, vec!["src/main.rs", "src/walk.rs"]);
}

#[test]
fn path_survives_bounded_generics() {
    let index = index_of(&[(
        "a.rs",
        "pub async fn a<T: Into<String>>(x: T) { b(); }\nfn b<F: Fn(u8)>(f: F) { c(); }\nfn c() {}\n",
    )]);
    let names: Vec<_> = path(&index, "a", "c").into_iter().map(|h| h.name).collect();
    assert_eq!(names, vec!["a", "b", "c"]);
}

#[test]
fn path_to_unknown_name_is_empty() {
    let index = index_of(&[("a.rs", "fn a() {}\n")]);
    assert!(path(&index, "nope", "nope").is_empty());
}

#[test]
fn ts_imports_resolve_js_extension_and_alias() {
    let index = index_of(&[
        (
            "src/app.ts",
            "import { x } from './x.js';\nimport { y } from '@/lib/y';\n",
        ),
        ("src/x.ts", "export const x = 1;\n"),
        ("src/lib/y.ts", "export const y = 1;\n"),
    ]);
    assert_eq!(path(&index, "src/app.ts", "src/x.ts").len(), 2);
    assert_eq!(path(&index, "src/app.ts", "src/lib/y.ts").len(), 2);
}

#[test]
fn js_files_are_parsed_and_their_imports_resolve() {
    let index = index_of(&[
        (
            "src/app.js",
            "import { x } from './x.js';\nimport y from './y';\nimport z from './z.mjs';\nexport function run() { x(); }\n",
        ),
        ("src/x.js", "export function x() {}\n"),
        ("src/y/index.jsx", "export default function y() {}\n"),
        ("src/z.mjs", "export default function z() {}\n"),
    ]);
    assert_eq!(def(&index, "run").len(), 1);
    for target in ["src/x.js", "src/y/index.jsx", "src/z.mjs"] {
        assert_eq!(path(&index, "src/app.js", target).len(), 2, "{target}");
    }
}

#[test]
fn js_suffix_order_depends_on_the_importer() {
    let files = [
        ("src/x.ts", "export const x = 1;\n"),
        ("src/x.js", "export const x = 2;\n"),
        ("src/lib/index.js", "export const l = 1;\n"),
    ];
    let ts = index_of(
        &[
            &files[..],
            &[("src/a.ts", "import './x.js';\nimport '@/lib';\n")],
        ]
        .concat(),
    );
    assert_eq!(path(&ts, "src/a.ts", "src/x.ts").len(), 2);
    assert!(path(&ts, "src/a.ts", "src/x.js").is_empty());
    assert_eq!(path(&ts, "src/a.ts", "src/lib/index.js").len(), 2);
    let js = index_of(
        &[
            &files[..],
            &[("src/a.js", "import './x.js';\nimport './x.config';\n")],
        ]
        .concat(),
    );
    assert_eq!(path(&js, "src/a.js", "src/x.js").len(), 2);
}

#[test]
fn python_imports_resolve_relative_dotted_and_package_modules() {
    let index = index_of(&[
        (
            "app/main.py",
            "from .util import u\nfrom . import pkg\nfrom ..top import t\nimport app.models.user\nimport os\n",
        ),
        ("app/util.py", "def u(): pass\n"),
        ("app/pkg/__init__.py", "def p(): pass\n"),
        ("top.py", "def t(): pass\n"),
        ("app/models/user.py", "def user(): pass\n"),
    ]);
    for target in [
        "app/util.py",
        "app/pkg/__init__.py",
        "top.py",
        "app/models/user.py",
    ] {
        assert_eq!(path(&index, "app/main.py", target).len(), 2, "{target}");
    }
}

#[test]
fn python_relative_imports_cannot_climb_above_the_root() {
    let index = index_of(&[
        ("app/main.py", "from ...x import y\nfrom . import z\n"),
        ("x.py", "def y(): pass\n"),
        ("app/z.py", "def z(): pass\n"),
    ]);
    assert!(path(&index, "app/main.py", "x.py").is_empty());
    assert_eq!(path(&index, "app/main.py", "app/z.py").len(), 2);
}

#[test]
fn python_local_imports_show_only_project_files() {
    let index = index_of(&[
        (
            "app/main.py",
            "import os\nfrom .util import u\nfrom pkg import sub\n",
        ),
        ("app/util.py", "def u(): pass\n"),
        ("pkg/sub.py", "def s(): pass\n"),
    ]);
    let entry = &index.files["app/main.py"];
    assert_eq!(local_imports(&index, entry), vec![".util", "pkg.sub"]);
    assert_eq!(path(&index, "app/main.py", "pkg/sub.py").len(), 2);
}

#[test]
fn java_imports_resolve_under_any_source_root() {
    let index = index_of(&[
        (
            "src/main/java/app/Store.java",
            "package app;\nimport com.shop.Cart;\nimport static com.shop.Util.fmt;\nimport java.util.List;\nclass Store {}\n",
        ),
        (
            "src/main/java/com/shop/Cart.java",
            "package com.shop;\nclass Cart {}\n",
        ),
        (
            "src/main/java/com/shop/Util.java",
            "package com.shop;\nclass Util {}\n",
        ),
    ]);
    let from = "src/main/java/app/Store.java";
    assert_eq!(
        path(&index, from, "src/main/java/com/shop/Cart.java").len(),
        2
    );
    assert_eq!(
        path(&index, from, "src/main/java/com/shop/Util.java").len(),
        2
    );
    assert_eq!(local_imports(&index, &index.files[from]).len(), 2);
}

#[test]
fn kotlin_imports_resolve_under_any_source_root() {
    let index = index_of(&[
        (
            "src/main/kotlin/app/Store.kt",
            "package app\nimport com.shop.Cart\nimport com.shop.*\nimport kotlin.collections.List\nclass Store\n",
        ),
        (
            "src/main/kotlin/com/shop/Cart.kt",
            "package com.shop\nclass Cart\n",
        ),
    ]);
    let from = "src/main/kotlin/app/Store.kt";
    assert_eq!(
        path(&index, from, "src/main/kotlin/com/shop/Cart.kt").len(),
        2
    );
    assert_eq!(local_imports(&index, &index.files[from]).len(), 1);
}

#[test]
fn php_imports_resolve_with_or_without_the_root_namespace_folder() {
    let index = index_of(&[
        (
            "src/Controller/Home.php",
            "<?php\nnamespace App\\Controller;\nuse App\\Model\\Cart;\nuse App\\Model\\Missing;\nuse Vendor\\Lib\\Thing;\nclass Home {}\n",
        ),
        (
            "src/Model/Cart.php",
            "<?php\nnamespace App\\Model;\nclass Cart {}\n",
        ),
    ]);
    let from = "src/Controller/Home.php";
    assert_eq!(path(&index, from, "src/Model/Cart.php").len(), 2);
    assert_eq!(local_imports(&index, &index.files[from]).len(), 1);
}

#[test]
fn php_ambiguous_single_part_and_function_imports_make_no_edge() {
    let index = index_of(&[
        ("a/Model/Cart.php", "<?php\nclass Cart {}\n"),
        ("b/Model/Cart.php", "<?php\nclass Cart {}\n"),
        ("lib/Foo.php", "<?php\nclass Foo {}\n"),
        (
            "main.php",
            "<?php\nuse App\\Model\\Cart;\nuse Foo;\nuse function App\\Lib\\Foo;\nuse App\\{Foo as F};\n",
        ),
    ]);
    assert!(local_imports(&index, &index.files["main.php"]).is_empty());
}

#[test]
fn php_composer_roots_decide_what_is_a_project_class() {
    let files: BTreeMap<String, ()> = [
        "src/Model/Cart.php",
        "lib/Thing.php",
        "src/Illuminate/Support/Facades/Cache.php",
        "src/Illuminate/Collections/Collection.php",
    ]
    .iter()
    .map(|p| (p.to_string(), ()))
    .collect();
    let roots = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    let aliases = roots(&[
        ("App\\", "src/"),
        ("Illuminate\\", "src/Illuminate/"),
        (
            "Illuminate\\Support\\",
            "src/Illuminate/Macroable/|src/Illuminate/Collections/",
        ),
        ("@/", "src/"),
    ]);
    let hit = |imp: &str| resolve_php_class(&files, &aliases, imp);
    assert_eq!(
        hit("App\\Model\\Cart").as_deref(),
        Some("src/Model/Cart.php")
    );
    // The vendor class shares its last two parts with a project file, but no root owns it.
    assert_eq!(hit("Vendor\\Lib\\Thing"), None);
    // The longest root has no such file; the shorter one does (Composer tries them in turn).
    assert_eq!(
        hit("Illuminate\\Support\\Facades\\Cache").as_deref(),
        Some("src/Illuminate/Support/Facades/Cache.php")
    );
    // Several folders under one root.
    assert_eq!(
        hit("Illuminate\\Support\\Collection").as_deref(),
        Some("src/Illuminate/Collections/Collection.php")
    );
    assert_eq!(hit("App\\Missing"), None);
}

#[test]
fn java_ambiguous_and_missing_classes_make_no_edge() {
    let index = index_of(&[
        ("x/a/Foo.java", "class Foo {}\n"),
        ("y/a/Foo.java", "class Foo {}\n"),
        (
            "Main.java",
            "import a.Foo;\nimport java.util.List;\nimport a.*;\nclass Main {}\n",
        ),
    ]);
    assert!(path(&index, "Main.java", "x/a/Foo.java").is_empty());
    assert!(local_imports(&index, &index.files["Main.java"]).is_empty());
}

#[test]
fn c_includes_resolve_near_then_by_unique_path() {
    let index = index_of(&[
        (
            "src/main.c",
            "#include \"util.h\"\n#include \"net/sock.h\"\n#include <stdio.h>\nint main(void) { return 0; }\n",
        ),
        ("src/util.h", "int u(void);\n"),
        ("include/net/sock.h", "int s(void);\n"),
    ]);
    assert_eq!(path(&index, "src/main.c", "src/util.h").len(), 2);
    assert_eq!(path(&index, "src/main.c", "include/net/sock.h").len(), 2);
    assert_eq!(local_imports(&index, &index.files["src/main.c"]).len(), 2);
}

#[test]
fn c_includes_with_dotdot_work_and_ambiguous_names_make_no_edge() {
    let index = index_of(&[
        (
            "src/a/main.c",
            "#include \"../x.h\"\n#include \"util.h\"\nint main(void) { return 0; }\n",
        ),
        ("src/x.h", "int x(void);\n"),
        ("one/util.h", "int u(void);\n"),
        ("two/util.h", "int v(void);\n"),
    ]);
    assert_eq!(path(&index, "src/a/main.c", "src/x.h").len(), 2);
    assert!(path(&index, "src/a/main.c", "one/util.h").is_empty());
}

fn resolve(index: &mut Index, file: &str, call: u32, target: Resolution) {
    let entry = index.files.get_mut(file).unwrap();
    entry.resolved.push(ResolvedCall { call, target });
}

fn def_at(path: &str, line: u32) -> Resolution {
    Resolution::Def {
        path: path.into(),
        line,
    }
}

#[test]
fn scip_resolutions_drop_external_calls_and_disambiguate_callers() {
    let mut index = index_of(&[
        (
            "a.rs",
            "fn new() {}\nfn run() { new(); }\nfn run2() { new(); }\n",
        ),
        (
            "b.rs",
            "fn new() {}\nfn go() { new(); }\nfn plain() { new(); }\n",
        ),
    ]);
    resolve(&mut index, "a.rs", 0, def_at("a.rs", 1));
    resolve(&mut index, "a.rs", 1, Resolution::External);
    resolve(&mut index, "b.rs", 0, def_at("b.rs", 1));
    let hits: Vec<_> = callers(&index, "new").iter().map(CallerHit::text).collect();
    assert_eq!(
        hits,
        vec![
            "a.rs:2  in fn run  -> a.rs:1",
            "b.rs:2  in fn go  -> b.rs:1",
            // No SCIP answer: the resolver pins a bare call to the definition in its own file.
            "b.rs:3  in fn plain  -> b.rs:1",
        ]
    );
    // One definition among the hits: nothing to disambiguate, no arrow.
    let mut single = index_of(&[("a.rs", "fn new() {}\nfn run() { new(); }\n")]);
    resolve(&mut single, "a.rs", 0, def_at("a.rs", 1));
    assert_eq!(callers(&single, "new")[0].text(), "a.rs:2  in fn run");
}

#[test]
fn external_calls_are_not_graph_edges() {
    let mut index = index_of(&[("a.rs", "fn b() {}\nfn run() { b(); }\nfn two() { b(); }\n")]);
    resolve(&mut index, "a.rs", 0, Resolution::External);
    let who: Vec<_> = affected(&index, "b", None)
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(who, vec!["two"]);
    assert_eq!(explain(&index, "b").incoming.len(), 1);
}

fn hub_rows(index: &Index) -> Vec<(String, String, usize, usize)> {
    hubs(index, 20)
        .into_iter()
        .map(|h| (h.name, h.location, h.incoming, h.outgoing))
        .collect()
}

fn two_news() -> Index {
    index_of(&[
        ("a.rs", "fn new() {}\nfn run_a() { new(); }\n"),
        ("b.rs", "fn new() {}\nfn run_b() { new(); }\n"),
        ("c.rs", "fn run_c() { new(); }\n"),
    ])
}

#[test]
fn hubs_split_same_named_definitions_once_scip_knows_which_is_which() {
    let mut index = two_news();
    // Without SCIP data the three `new` calls are one name: legacy behaviour.
    let legacy = hub_rows(&index);
    assert_eq!(legacy[0], ("new".into(), "a.rs:1".into(), 3, 0));
    resolve(&mut index, "a.rs", 0, def_at("a.rs", 1));
    resolve(&mut index, "b.rs", 0, def_at("b.rs", 1));
    // run_c's call is unresolved and `new` has two definitions: ambiguous, left out.
    assert_eq!(
        hub_rows(&index),
        vec![
            ("new".into(), "a.rs:1".into(), 1, 0),
            ("new".into(), "b.rs:1".into(), 1, 0),
            ("run_a".into(), "a.rs:2".into(), 0, 1),
            ("run_b".into(), "b.rs:2".into(), 0, 1),
        ]
    );
}

#[test]
fn a_language_without_scip_data_keeps_name_level_hubs_in_a_mixed_project() {
    let mut index = two_news();
    resolve(&mut index, "a.rs", 0, def_at("a.rs", 1));
    resolve(&mut index, "b.rs", 0, def_at("b.rs", 1));
    let py = index_of(&[("p.py", "def go():\n    new()\n")]);
    index.files.insert("p.py".into(), py.files["p.py"].clone());
    let rows = hub_rows(&index);
    // Rust has SCIP data, so run_c's unresolved call is still left out ...
    assert!(rows.iter().all(|r| r.0 != "run_c"), "{rows:?}");
    // ... but the Python caller of the same-named `new` is counted by name, as before.
    assert!(
        rows.contains(&("go".into(), "p.py:1".into(), 0, 1)),
        "{rows:?}"
    );
}

#[test]
fn hubs_keep_counting_unresolved_calls_to_a_name_with_one_definition() {
    let mut index = index_of(&[(
        "a.rs",
        "fn helper() {}\nfn go() { helper(); }\nfn other() { x(); }\nfn x() {}\n",
    )]);
    resolve(&mut index, "a.rs", 1, def_at("a.rs", 4));
    let rows = hub_rows(&index);
    assert!(
        rows.contains(&("helper".into(), "a.rs:1".into(), 1, 0)),
        "{rows:?}"
    );
}

#[test]
fn ambiguous_python_module_names_make_no_edge() {
    let index = index_of(&[
        ("main.py", "import util\n"),
        ("a/util.py", "def u(): pass\n"),
        ("b/util.py", "def u(): pass\n"),
    ]);
    assert!(path(&index, "main.py", "a/util.py").is_empty());
}

#[test]
fn ambiguous_rust_module_names_make_no_edge() {
    let index = index_of(&[
        ("src/main.rs", "mod error;\n"),
        ("a/error.rs", "fn e() {}\n"),
        ("b/error.rs", "fn e() {}\n"),
    ]);
    assert!(path(&index, "src/main.rs", "a/error.rs").is_empty());
}

#[test]
fn alias_folder_comes_from_the_index() {
    let mut index = index_of(&[
        ("app/page.ts", "import { y } from '@/lib/y';\n"),
        ("app/lib/y.ts", "export const y = 1;\n"),
    ]);
    index.aliases = BTreeMap::from([("@/".to_string(), "app/".to_string())]);
    assert_eq!(path(&index, "app/page.ts", "app/lib/y.ts").len(), 2);
}

#[test]
fn every_configured_alias_resolves_and_packages_do_not() {
    let mut index = index_of(&[
        (
            "src/app.ts",
            "import a from '~/util';\nimport b from '@components/Btn';\nimport r from 'react';\n",
        ),
        ("src/util.ts", "export const a = 1;\n"),
        ("src/components/Btn.tsx", "export const b = 1;\n"),
        ("react.ts", "export const r = 1;\n"),
    ]);
    index.aliases = BTreeMap::from([
        ("~/".to_string(), "src/".to_string()),
        ("@components/".to_string(), "src/components/".to_string()),
    ]);
    assert_eq!(path(&index, "src/app.ts", "src/util.ts").len(), 2);
    assert_eq!(
        path(&index, "src/app.ts", "src/components/Btn.tsx").len(),
        2
    );
    // `react` is an external package, not the local file that happens to share its name.
    assert!(path(&index, "src/app.ts", "react.ts").is_empty());
}

#[test]
fn local_imports_hide_packages_in_ts_but_not_in_rust() {
    let index = index_of(&[
        (
            "a.ts",
            "import r from 'react';\nimport x from './x';\nimport y from '@/y';\n",
        ),
        ("b.rs", "use serde::Serialize;\n"),
    ]);
    assert_eq!(
        local_imports(&index, &index.files["a.ts"]),
        vec!["./x", "@/y"]
    );
    assert_eq!(local_imports(&index, &index.files["b.rs"]), vec!["serde"]);
}

#[test]
fn rust_modules_resolve_by_file_style() {
    let index = index_of(&[
        ("src/main.rs", "mod foo;\nmod error;\n"),
        ("src/foo.rs", "mod a;\n"),
        ("src/foo/a.rs", "fn a() {}\n"),
        ("src/other/a.rs", "fn a() {}\n"),
        ("error.rs", "fn e() {}\n"),
    ]);
    // 2018-style nesting wins over the project-wide ambiguity of `a.rs`.
    assert_eq!(path(&index, "src/foo.rs", "src/foo/a.rs").len(), 2);
    // A root-level module is found when it is the only `error.rs`.
    assert_eq!(path(&index, "src/main.rs", "error.rs").len(), 2);
}

const GRAPH: &str = "fn a() { b(); c(); }\nfn b() { c(); }\nfn c() {}\nfn d() { a(); }\n";

#[test]
fn explain_lists_defs_callers_and_callees() {
    let index = index_of(&[("a.rs", GRAPH)]);
    let e = explain(&index, "a");
    assert_eq!(e.defs[0].text(), "a.rs:1 fn a()");
    let names = |links: &[Link]| links.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&e.incoming), vec!["d"]);
    assert_eq!(names(&e.outgoing), vec!["b", "c"]);
    assert_eq!(e.outgoing[1].text(), "c  a.rs:3 (1 site)");
    let lines = e.lines();
    assert_eq!(lines[0], "def a.rs:1 fn a()");
    assert_eq!(lines[1], "<- d  a.rs:4 (1 site)");
    assert_eq!(lines[2], "-> b  a.rs:2 (1 site)");
}

#[test]
fn explain_unknown_name_is_empty() {
    let index = index_of(&[("a.rs", GRAPH)]);
    assert!(explain(&index, "nope").lines().is_empty());
}

#[test]
fn affected_walks_callers_nearest_first() {
    let index = index_of(&[("a.rs", GRAPH)]);
    let all: Vec<_> = affected(&index, "c", None)
        .iter()
        .map(|a| (a.name.clone(), a.depth))
        .collect();
    assert_eq!(all, vec![("a".into(), 1), ("b".into(), 1), ("d".into(), 2)]);
    let near = affected(&index, "c", Some(1));
    assert_eq!(near.len(), 2);
    assert_eq!(near[0].text(), "a  a.rs:1  depth 1");
    assert!(affected(&index, "d", None).is_empty());
}

#[test]
fn hubs_rank_project_symbols_by_connections() {
    let index = index_of(&[(
        "a.rs",
        &format!("{GRAPH}fn e() {{ println!(\"x\"); a(); }}\n"),
    )]);
    let top: Vec<_> = hubs(&index, 3)
        .iter()
        .map(|h| (h.name.clone(), h.incoming, h.outgoing))
        .collect();
    // a: callers d,e / callees b,c; println is external and never listed.
    assert_eq!(
        top,
        vec![("a".into(), 2, 2), ("b".into(), 1, 1), ("c".into(), 2, 0)]
    );
    assert_eq!(hubs(&index, 1)[0].text(), "a  a.rs:1  in=2 out=2");
}

#[test]
fn hubs_ignore_self_calls() {
    let index = index_of(&[("a.rs", "fn r() { r(); }\nfn s() { r(); }\n")]);
    let r = &hubs(&index, 5)[0];
    assert_eq!((r.name.as_str(), r.incoming, r.outgoing), ("r", 1, 0));
}

#[test]
fn explain_handles_ambiguous_names_and_undefined_callees() {
    let index = index_of(&[
        (
            "a.rs",
            "fn dup() {}\nfn user() { dup(); println!(\"x\"); }\n",
        ),
        ("b.ts", "export function dup() {}\n"),
    ]);
    assert_eq!(explain(&index, "dup").defs.len(), 2);
    let external = explain(&index, "println");
    assert!(external.defs.is_empty());
    assert_eq!(external.incoming[0].name, "user");
}

#[test]
fn explain_leaves_out_top_level_callers() {
    let index = index_of(&[(
        "a.rs",
        "fn target() {}\nfn user() { target(); }\ntarget();\n",
    )]);
    let names: Vec<_> = explain(&index, "target")
        .incoming
        .iter()
        .map(|l| l.name.clone())
        .collect();
    assert_eq!(names, vec!["user"]);
}

fn link(name: &str, loc: Option<&str>, sites: usize) -> Link {
    Link {
        name: name.into(),
        location: loc.map(String::from),
        sites,
    }
}

fn crowded() -> Explain {
    let long = "x".repeat(120);
    Explain {
        defs: (1..=7)
            .map(|n| DefHit {
                path: format!("src/d{n}.rs"),
                line: n,
                signature: long.clone(),
            })
            .collect(),
        incoming: vec![
            link("t1", Some("tests/a.rs:1"), 9),
            link("c1", Some("src/c1.rs:1"), 3),
            link("c2", Some("src/c2.rs:1"), 1),
            link("c3", None, 1),
            link("c4", Some("src/c4.rs:1"), 1),
            link("c5", Some("src/c5.rs:1"), 1),
            link("c6", Some("src/c6.rs:1"), 1),
            link("t2", Some("src/x_test.go:1"), 1),
        ],
        outgoing: vec![link("o1", Some("src/o.rs:1"), 2), link("o2", None, 1)],
    }
}

#[test]
fn brief_caps_definitions_cuts_signatures_and_says_how_to_see_more() {
    let lines = crowded().lines_in(Mode::Brief, |_| Vec::new());
    let defs: Vec<_> = lines.iter().filter(|l| l.starts_with("def ")).collect();
    assert_eq!(defs.len(), 6, "5 definitions and one more-line");
    assert!(defs[0].len() < "def src/d1.rs:1 ".len() + 85, "{}", defs[0]);
    assert!(defs[0].ends_with("..."));
    assert_eq!(defs[5].as_str(), "def ... +2 more (mode=defs)");
}

#[test]
fn brief_lists_five_neighbours_tests_last_and_sites_only_above_one() {
    let lines = crowded().lines_in(Mode::Brief, |_| Vec::new());
    let incoming: Vec<_> = lines.iter().filter(|l| l.starts_with("<-")).collect();
    assert_eq!(
        incoming,
        vec![
            "<- c1  src/c1.rs:1 (3 sites)",
            "<- c2  src/c2.rs:1",
            "<- c3",
            "<- c4  src/c4.rs:1",
            "<- c5  src/c5.rs:1",
            "<- ... +3 more (mode=callers)",
        ]
    );
    let outgoing: Vec<_> = lines.iter().filter(|l| l.starts_with("->")).collect();
    assert_eq!(outgoing, vec!["-> o1  src/o.rs:1 (2 sites)", "-> o2"]);
}

#[test]
fn full_is_the_old_output_and_modes_pick_one_part() {
    let e = crowded();
    assert_eq!(e.lines_in(Mode::Full, |_| Vec::new()), e.lines());
    let only = |mode, prefix: &str| {
        let lines = e.lines_in(mode, |_| Vec::new());
        !lines.is_empty() && lines.iter().all(|l| l.starts_with(prefix))
    };
    assert!(only(Mode::Defs, "def "));
    assert!(only(Mode::Callers, "<-"));
    assert!(only(Mode::Callees, "->"));
}

#[test]
fn modes_parse_from_their_names() {
    for (text, mode) in [
        ("brief", Mode::Brief),
        ("full", Mode::Full),
        ("callers", Mode::Callers),
        ("callees", Mode::Callees),
        ("defs", Mode::Defs),
    ] {
        assert_eq!(text.parse::<Mode>(), Ok(mode));
    }
    assert!("everything".parse::<Mode>().is_err());
    assert_eq!(Mode::default(), Mode::Brief);
}

#[test]
fn brief_hands_each_original_definition_to_the_teach_hook() {
    let same_place = |sig: &str| DefHit {
        path: "A.kt".into(),
        line: 1,
        signature: sig.into(),
    };
    let e = Explain {
        defs: vec![same_place("class A"), same_place("constructor")],
        incoming: vec![],
        outgoing: vec![],
    };
    let lines = e.lines_in(Mode::Brief, |d| vec![format!("  u:{}", d.signature)]);
    assert_eq!(
        lines,
        vec![
            "def A.kt:1 class A",
            "  u:class A",
            "def A.kt:1 constructor",
            "  u:constructor"
        ]
    );
}

#[test]
fn cutting_text_never_splits_a_character_and_an_empty_list_says_none() {
    let long = "é".repeat(90);
    let cut_text = cut(&long, 80);
    assert_eq!(cut_text.chars().count(), 83);
    assert!(cut_text.ends_with("..."));
    assert_eq!(cut("short", 80), "short");
    let nobody = Explain {
        defs: vec![],
        incoming: vec![],
        outgoing: vec![],
    };
    assert_eq!(
        nobody.lines_in(Mode::Callers, |_| Vec::new()),
        ["<- (none)"]
    );
    assert_eq!(
        nobody.lines_in(Mode::Callees, |_| Vec::new()),
        ["-> (none)"]
    );
    assert!(nobody.lines_in(Mode::Brief, |_| Vec::new()).is_empty());
}

#[test]
fn a_package_alias_to_a_file_matches_only_that_exact_name() {
    let mut index = index_of(&[
        (
            "app.ts",
            "import a from 'zod/v4';\nimport b from 'zod/v4-mini';\nimport c from 'zod/v4/x';\n",
        ),
        ("src/v4/index.ts", "export const a = 1;\n"),
        ("src/v4/x.ts", "export const c = 1;\n"),
    ]);
    index.aliases = BTreeMap::from([
        ("zod/".to_string(), "src/".to_string()),
        ("zod/v4".to_string(), "src/v4/index.ts".to_string()),
    ]);
    let got: Vec<_> = ["zod/v4", "zod/v4-mini", "zod/v4/x"]
        .iter()
        .map(|imp| resolve_import(&index.files, &index.aliases, "app.ts", imp))
        .collect();
    let some = |p: &str| Some(p.to_string());
    // `zod/v4/x` is under the folder alias `zod/`, not inside the file `zod/v4` names.
    assert_eq!(
        got,
        vec![some("src/v4/index.ts"), None, some("src/v4/x.ts")]
    );
}
