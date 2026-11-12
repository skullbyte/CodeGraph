use super::super::{Lang, extract as extract_file};
use crate::model::{Form, Hop, Kind, Ret};

const SRC: &str = "use std::fmt;
use crate::util::helper;
use serde::Serialize;
mod net;
pub mod tests_helper { }
pub struct Greeter;
pub enum Color { Red }
pub trait Speak { fn speak(&self); }
impl Speak for Greeter {
    fn speak(&self) {
        helper();
        self.log(\"x\");
        println!(\"hi\");
    }
}
pub type Id = u32;
pub const MAX: usize = 3;
const PRIVATE: usize = 1;
fn outer() {
    fn inner() { deep(); }
    Greeter::new();
}
";

#[test]
fn use_trees_expand_to_alias_and_full_path() {
    let src = "use a::b;\npub use self::c::{D, e as f, g::{self, H}, k::{self as kk}};\nuse crate::x::*;\nuse super::sup;\n";
    let facts = extract_file(Lang::Rust, src).unwrap();
    let got: Vec<_> = facts
        .uses
        .iter()
        .map(|u| (u.alias.as_str(), u.path.as_str(), u.public))
        .collect();
    let want = vec![
        ("b", "a::b", false),
        ("D", "self::c::D", true),
        ("f", "self::c::e", true),
        ("g", "self::c::g", true),
        ("H", "self::c::g::H", true),
        ("kk", "self::c::k", true),
        ("sup", "super::sup", false),
    ];
    assert_eq!(got, want);
}

#[test]
fn uses_inside_a_mod_or_fn_are_marked_local() {
    let src = "use a::top;\nmod t { use b::inner; }\nfn f() { use c::local; }\n";
    let facts = extract_file(Lang::Rust, src).unwrap();
    let got: Vec<_> = facts
        .uses
        .iter()
        .map(|u| (u.alias.as_str(), u.local))
        .collect();
    assert_eq!(got, vec![("top", false), ("inner", true), ("local", true)]);
}

#[test]
fn qualified_type_paths_keep_their_modules() {
    let src =
        "fn f() { a::T::one(); Vec::<u8>::two(); a::Vec::<u8>::three(); Self::Item::four(); }";
    let facts = extract_file(Lang::Rust, src).unwrap();
    let got: Vec<_> = facts
        .calls
        .iter()
        .map(|c| (c.callee.as_str(), c.form.clone()))
        .collect();
    let ty = |t: &str| Form::Type(t.to_string());
    assert_eq!(got[0], ("one", ty("a::T")));
    let assoc = extract_file(Lang::Rust, "fn f() { a::Foo<T>::Assoc::g(); }").unwrap();
    assert_eq!(assoc.calls[0].form, Form::Unknown);
    let leading = extract_file(Lang::Rust, "fn f() { ::a::T::g(); }").unwrap();
    assert_eq!(leading.calls[0].form, Form::Unknown);
    assert_eq!(got[1], ("two", ty("Vec")));
    assert_eq!(got[2], ("three", ty("a::Vec")));
    assert_eq!(got[3], ("four", Form::Unknown));
}

#[test]
fn symbols() {
    let facts = extract_file(Lang::Rust, SRC).unwrap();
    let got: Vec<_> = facts
        .symbols
        .iter()
        .map(|s| (s.name.as_str(), s.kind, s.line))
        .collect();
    let want = vec![
        ("net", Kind::Mod, 4),
        ("tests_helper", Kind::Mod, 5),
        ("Greeter", Kind::Struct, 6),
        ("Color", Kind::Enum, 7),
        ("Speak", Kind::Trait, 8),
        ("Greeter", Kind::Impl, 9),
        ("speak", Kind::Fn, 10),
        ("Id", Kind::Type, 16),
        ("MAX", Kind::Const, 17),
        ("outer", Kind::Fn, 19),
        ("inner", Kind::Fn, 20),
    ];
    assert_eq!(got, want);
}

#[test]
fn items_know_their_container_and_calls_know_their_form() {
    let facts = extract_file(
            Lang::Rust,
            "struct Foo;\nimpl<T> Foo<T> {\n    fn a(&self) {\n        self.b();\n        Self::c();\n        Bar::d();\n        m::e();\n        x.f();\n        g();\n        Vec::<u8>::new();\n        a::b::Baz::q();\n    }\n}\ntrait Tr {\n    fn t(&self) { self.u(); }\n}\nimpl Tr for &Foo {}\nfn free() { h(); }\n",
        )
        .unwrap();
    let containers: Vec<_> = facts
        .symbols
        .iter()
        .filter(|s| s.kind != Kind::Impl)
        .map(|s| (s.name.as_str(), s.container.as_deref()))
        .collect();
    assert_eq!(
        containers,
        vec![
            ("Foo", None),
            ("a", Some("Foo")),
            ("Tr", None),
            ("t", Some("Tr")),
            ("free", None)
        ]
    );
    let calls: Vec<_> = facts
        .calls
        .iter()
        .map(|c| (c.callee.as_str(), c.form.clone(), c.owner.as_deref()))
        .collect();
    let ty = |t: &str| Form::Type(t.to_string());
    assert_eq!(
        calls,
        vec![
            ("b", Form::SelfRef, Some("Foo")),
            ("c", Form::SelfRef, Some("Foo")),
            ("d", ty("Bar"), Some("Foo")),
            ("e", Form::Module("m".into()), Some("Foo")),
            ("f", Form::Method, Some("Foo")),
            ("g", Form::Bare, Some("Foo")),
            ("new", ty("Vec"), Some("Foo")),
            ("q", ty("a::b::Baz"), Some("Foo")),
            ("u", Form::SelfRef, Some("Tr")),
            ("h", Form::Bare, None),
        ]
    );
}

#[test]
fn odd_paths_never_claim_a_module_or_a_method_they_cannot_know() {
    let facts = extract_file(
            Lang::Rust,
            "fn a() {\n    self::b();\n    crate::m::c();\n    <T as Tr>::d();\n    <Foo>::e();\n    u64::from(1);\n    str::len(x);\n    super::f();\n    (self.cb)();\n}\n",
        )
        .unwrap();
    let forms: Vec<_> = facts
        .calls
        .iter()
        .map(|c| (c.callee.as_str(), c.form.clone()))
        .collect();
    assert_eq!(
        forms,
        vec![
            ("b", Form::Bare),
            ("c", Form::Module("m".into())),
            ("d", Form::Unknown),
            ("e", Form::Unknown),
            ("from", Form::Unknown),
            ("len", Form::Unknown),
            ("f", Form::Bare),
        ]
    );
}

#[test]
fn a_nested_fn_belongs_to_no_type_and_a_methods_own_calls_keep_theirs() {
    let facts = extract_file(
            Lang::Rust,
            "impl Foo {\n    fn m(&self) {\n        fn helper() { deep(); }\n        struct Local;\n        self.own();\n    }\n}\n",
        )
        .unwrap();
    let containers: Vec<_> = facts
        .symbols
        .iter()
        .filter(|s| s.kind != Kind::Impl)
        .map(|s| (s.name.as_str(), s.container.as_deref()))
        .collect();
    assert_eq!(
        containers,
        vec![("m", Some("Foo")), ("helper", None), ("Local", None)]
    );
    let owners: Vec<_> = facts
        .calls
        .iter()
        .map(|c| (c.callee.as_str(), c.owner.as_deref()))
        .collect();
    assert_eq!(owners, vec![("deep", None), ("own", Some("Foo"))]);
}

#[test]
fn an_impl_inside_a_function_still_gives_its_methods_an_owner() {
    let facts = extract_file(
            Lang::Rust,
            "fn t() {\n    impl Foo {\n        fn a(&self) { self.b(); fn inner() { deep(); } }\n    }\n    trait Tr { fn d(&self) { self.e(); } }\n    free();\n}\n",
        )
        .unwrap();
    let owners: Vec<_> = facts
        .calls
        .iter()
        .map(|c| (c.callee.as_str(), c.owner.as_deref()))
        .collect();
    assert_eq!(
        owners,
        vec![
            ("b", Some("Foo")),
            ("deep", None),
            ("e", Some("Tr")),
            ("free", None)
        ]
    );
}

#[test]
fn impl_targets_lose_references_lifetimes_dyn_and_generics() {
    let facts = extract_file(
            Lang::Rust,
            "impl<'a> Tr for &'a mut Foo<'a> { fn a(&self) {} }\nimpl Tr for dyn Bar { fn b(&self) {} }\nimpl<T> Tr for a::Baz<T> { fn c(&self) {} }\n",
        )
        .unwrap();
    let got: Vec<_> = facts
        .symbols
        .iter()
        .filter(|s| s.kind == Kind::Fn)
        .map(|s| (s.name.as_str(), s.container.as_deref()))
        .collect();
    assert_eq!(
        got,
        vec![("a", Some("Foo")), ("b", Some("Bar")), ("c", Some("Baz"))]
    );
}

#[test]
fn calls_inside_macro_arguments_read_their_form_from_the_tokens() {
    let facts = extract_file(
            Lang::Rust,
            "impl Foo {\n    fn a(&self) { vec![f(), self.g(), x.h(), Bar::i(), m::j(), crate::k(), Self::l(), n!()]; }\n}\n",
        )
        .unwrap();
    let forms: Vec<_> = facts
        .calls
        .iter()
        .skip(1)
        .map(|c| (c.callee.as_str(), c.form.clone()))
        .collect();
    assert_eq!(
        forms,
        vec![
            ("f", Form::Bare),
            ("g", Form::SelfRef),
            ("h", Form::Method),
            ("i", Form::Type("Bar".into())),
            ("j", Form::Module("m".into())),
            ("k", Form::Bare),
            ("l", Form::SelfRef),
            ("n", Form::Unknown),
        ]
    );
    let turbofish = extract_file(Lang::Rust, "fn a() { vec![Vec::<u8>::new()]; }\n").unwrap();
    let forms: Vec<_> = turbofish
        .calls
        .iter()
        .skip(1)
        .map(|c| (c.callee.as_str(), c.form.clone()))
        .collect();
    assert_eq!(
        forms,
        vec![("new", Form::Unknown)],
        "`>` before `::` is no qualifier"
    );
}

#[test]
fn macro_calls_have_no_form_but_keep_their_owner() {
    let facts = extract_file(
        Lang::Rust,
        "impl Foo {\n    fn a(&self) { println!(\"x\"); vec![helper(1)]; }\n}\n",
    )
    .unwrap();
    let calls: Vec<_> = facts
        .calls
        .iter()
        .map(|c| (c.callee.as_str(), c.form.clone(), c.owner.as_deref()))
        .collect();
    assert_eq!(
        calls,
        vec![
            ("println", Form::Unknown, Some("Foo")),
            ("vec", Form::Unknown, Some("Foo")),
            ("helper", Form::Bare, Some("Foo")),
        ]
    );
}

#[test]
fn signatures() {
    let facts = extract_file(
        Lang::Rust,
        "pub fn parse(input: &str)\n    -> Option<u8> {\n    None\n}\nimpl Speak for Greeter {}\n",
    )
    .unwrap();
    assert_eq!(
        facts.symbols[0].signature,
        "pub fn parse(input: &str) -> Option<u8>"
    );
    assert_eq!(facts.symbols[1].signature, "impl Speak for Greeter");
}

#[test]
fn imports_skip_std_and_dedupe() {
    let facts = extract_file(Lang::Rust, SRC).unwrap();
    assert_eq!(facts.imports, vec!["net", "serde", "util"]);
}

#[test]
fn calls_know_their_enclosing_fn() {
    let facts = extract_file(Lang::Rust, SRC).unwrap();
    let got: Vec<_> = facts
        .calls
        .iter()
        .map(|c| (c.callee.as_str(), c.line, c.caller.as_deref()))
        .collect();
    let want = vec![
        ("helper", 11, Some("fn speak")),
        ("log", 12, Some("fn speak")),
        ("println", 13, Some("fn speak")),
        ("deep", 20, Some("fn inner")),
        ("new", 21, Some("fn outer")),
    ];
    assert_eq!(got, want);
}

#[test]
fn items_inside_cfg_macros_are_found() {
    let src = "cfg_net! {\n    pub struct Foo {}\n    impl Foo { fn a() { b(); } }\n}\n";
    let facts = extract_file(Lang::Rust, src).unwrap();
    let got: Vec<_> = facts
        .symbols
        .iter()
        .map(|s| (s.name.as_str(), s.kind, s.line))
        .collect();
    assert_eq!(
        got,
        vec![
            ("Foo", Kind::Struct, 2),
            ("Foo", Kind::Impl, 3),
            ("a", Kind::Fn, 3)
        ]
    );
    assert_eq!(facts.calls[1].callee, "b");
    assert_eq!(facts.calls[1].line, 3);
    assert_eq!(facts.calls[1].caller.as_deref(), Some("fn a"));
}

#[test]
fn nested_cfg_macros_keep_line_numbers() {
    let src = "cfg_a! {\n    cfg_b! {\n        fn deep() { x(); }\n    }\n    fn after() {}\n}\n";
    let facts = extract_file(Lang::Rust, src).unwrap();
    let got: Vec<_> = facts
        .symbols
        .iter()
        .map(|s| (s.name.as_str(), s.line))
        .collect();
    assert_eq!(got, vec![("deep", 3), ("after", 5)]);
    let x = facts.calls.iter().find(|c| c.callee == "x").unwrap();
    assert_eq!(x.line, 3);
}

#[test]
fn calls_inside_macro_arguments_are_found() {
    let src = "fn t() {
    assert!(f(), \"m\");
    let v = vec![g(1), x.h()];
    println!(\"{}\", nested!(k()));
}
";
    let facts = extract_file(Lang::Rust, src).unwrap();
    let got: Vec<_> = facts
        .calls
        .iter()
        .map(|c| (c.callee.as_str(), c.line, c.caller.as_deref()))
        .collect();
    let t = Some("fn t");
    let want = vec![
        ("assert", 2, t),
        ("f", 2, t),
        ("vec", 3, t),
        ("g", 3, t),
        ("h", 3, t),
        ("println", 4, t),
        ("nested", 4, t),
        ("k", 4, t),
    ];
    assert_eq!(got, want);
}

#[test]
fn macro_arguments_do_not_count_fn_names_or_keywords() {
    let src = "m! { fn local(a: u8) {} if (x) { y } for i in (0..3) {} }\n";
    let facts = extract_file(Lang::Rust, src).unwrap();
    let names: Vec<_> = facts.calls.iter().map(|c| c.callee.as_str()).collect();
    assert_eq!(names, vec!["m"]);
}

#[test]
fn cfg_macros_without_items_still_get_their_calls() {
    let src =
        "cfg_if! {\n    if #[cfg(unix)] {\n        f();\n    } else {\n        g();\n    }\n}\n";
    let facts = extract_file(Lang::Rust, src).unwrap();
    let names: Vec<_> = facts.calls.iter().map(|c| c.callee.as_str()).collect();
    assert!(names.contains(&"f") && names.contains(&"g"), "{names:?}");
}

#[test]
fn restricted_visibility_does_not_cut_the_caller_name() {
    let src = "pub(crate) fn a() { x(); }\npub(in crate::m) fn b<T: Fn(u8)>() { y(); }\n";
    let facts = extract_file(Lang::Rust, src).unwrap();
    let callers: Vec<_> = facts.calls.iter().map(|c| c.caller.as_deref()).collect();
    assert_eq!(
        callers,
        vec![Some("pub(crate) fn a"), Some("pub(in crate::m) fn b")]
    );
}

fn method_forms(src: &str) -> Vec<(String, Form)> {
    let facts = extract_file(Lang::Rust, src).unwrap();
    facts
        .calls
        .iter()
        .filter(|c| !matches!(c.form, Form::Bare | Form::Type(_)) || c.callee.len() > 1)
        .map(|c| (c.callee.clone(), c.form.clone()))
        .collect()
}

fn ty(t: &str) -> Form {
    Form::Type(t.to_string())
}

#[test]
fn a_receiver_with_a_known_type_is_a_call_on_that_type() {
    let src = "struct Repo { db: Db, n: u32 }
impl Repo {
    fn run(&self, x: &Item, y: &mut Other<T>, z: impl Tr) {
        let a = Thing::new(1);
        let b = Made::default();
        let c: Typed = make();
        let d = Lit { v: 1 };
        let e = Try::new()?;
        self.db.qq();
        x.go(); y.stop(); z.tt(); a.hit(); b.mm(); c.beat(); d.ll(); e.t2(); w.unknown();
    }
}
";
    let got: Vec<_> = method_forms(src)
        .into_iter()
        .filter(|(n, _)| !matches!(n.as_str(), "new" | "default" | "make"))
        .collect();
    assert_eq!(
        got,
        vec![
            ("qq".to_string(), ty("Db")),
            ("go".into(), ty("Item")),
            ("stop".into(), ty("Other")),
            ("tt".into(), ty("Tr")),
            ("hit".into(), ty("Thing")),
            ("mm".into(), ty("Made")),
            ("beat".into(), ty("Typed")),
            ("ll".into(), ty("Lit")),
            ("t2".into(), ty("Try")),
            ("unknown".into(), Form::Method),
        ]
    );
}

#[test]
fn shadowing_generics_and_patterns_make_a_receiver_unknown() {
    let src = "fn f<T: Tr>(x: Foo, g: T, p: Pair) {
    let x = other();
    x.a();
    g.b();
    let y: A = mk();
    let y: B = mk();
    y.c();
    items.iter().map(|x| x.d());
    if let Some(p) = opt { p.e(); }
    for (i, p) in it { p.f(); }
    match v { Ok(x) => x.g2(), _ => {} }
}
";
    let facts = extract_file(Lang::Rust, src).unwrap();
    let got: Vec<_> = facts
        .calls
        .iter()
        .filter(|c| matches!(c.callee.as_str(), "a" | "b" | "c" | "d" | "e" | "f" | "g2"))
        .map(|c| (c.callee.clone(), c.form.clone()))
        .collect();
    assert_eq!(got.len(), 7);
    assert!(got.iter().all(|(_, f)| *f == Form::Method), "{got:?}");
}

#[test]
fn self_types_and_inner_functions() {
    let src = "impl Foo {
    fn new() -> Foo { Foo {} }
    fn m(&self, s: Self) {
        let t = Self::new();
        s.a(); t.b();
        fn inner() { s.c(); }
    }
}
";
    let facts = extract_file(Lang::Rust, src).unwrap();
    let got: Vec<_> = facts
        .calls
        .iter()
        .map(|c| (c.callee.clone(), c.form.clone()))
        .collect();
    let find = |n: &str| got.iter().find(|c| c.0 == n).unwrap().1.clone();
    assert_eq!(find("a"), ty("Foo"));
    assert_eq!(find("b"), ty("Foo"));
    assert_eq!(
        find("c"),
        Form::Method,
        "`s` is not in reach of a nested fn"
    );
}

fn forms_of(src: &str, names: &[&str]) -> Vec<(String, Form)> {
    let facts = extract_file(Lang::Rust, src).unwrap();
    facts
        .calls
        .iter()
        .filter(|c| names.contains(&c.callee.as_str()))
        .map(|c| (c.callee.clone(), c.form.clone()))
        .collect()
}

#[test]
fn struct_pattern_shorthand_and_rebinding_hide_the_outer_type() {
    let src = "fn f(x: A, y: B, v: V) {
    let V { x, .. } = v;
    x.go();
    if let W { y: z, .. } = v { z.run(); }
    let q = Foo::new();
    let q = q.build();
    q.finish();
    y.keep();
}
";
    let got = forms_of(src, &["go", "run", "finish", "keep"]);
    assert_eq!(
        got,
        vec![
            ("go".to_string(), Form::Method),
            ("run".into(), Form::Method),
            ("finish".into(), Form::Method),
            ("keep".into(), ty("B")),
        ]
    );
}

#[test]
fn associated_types_and_enum_variants_are_not_classes() {
    let src = "impl<I: Iterator> Foo<I> {
    fn f(&self, a: I::Item, b: Self::Output, c: Vec<u8>) {
        a.go(); b.run();
        let s = Shape::Circle { r: 1 };
        s.area();
        let t = Plain { v: 1 };
        t.size();
    }
}
";
    let got = forms_of(src, &["go", "run", "area", "size"]);
    assert_eq!(
        got,
        vec![
            ("go".to_string(), Form::Method),
            ("run".into(), Form::Method),
            ("area".into(), ty("Shape")),
            ("size".into(), ty("Plain")),
        ]
    );
}

#[test]
fn same_named_structs_with_different_field_types_make_the_field_unknown() {
    let src = "mod a { struct S { f: One, g: Same } }
mod b { struct S { f: Two, g: Same } }
impl S {
    fn m(&self) { self.f.go(); self.g.stay(); self.nope.x(); }
}
";
    let got = forms_of(src, &["go", "stay", "x"]);
    assert_eq!(
        got,
        vec![
            ("go".to_string(), Form::Method),
            ("stay".into(), ty("Same")),
            ("x".into(), Form::Method),
        ]
    );
}

#[test]
fn generics_of_the_impl_and_where_clauses_wrappers_and_tuple_structs() {
    let src = "struct W<T> { inner: T, boxed: Box<Foo>, n: u32 }
struct Tup(Foo);
impl<T> W<T> {
    fn m<U>(&self, a: T, b: U) where U: Tr {
        a.one(); b.two(); self.inner.three(); self.boxed.four();
    }
}
";
    let got = forms_of(src, &["one", "two", "three", "four"]);
    assert_eq!(
        got,
        vec![
            ("one".to_string(), Form::Method),
            ("two".into(), Form::Method),
            ("three".into(), Form::Method),
            ("four".into(), Form::Method),
        ]
    );
}

#[test]
fn function_types_inferred_types_and_smart_pointers_say_nothing() {
    let src = "fn f(cb: fn(u32) -> Foo, g: impl Fn(u32) -> u32, h: Box<Foo>) {
    let d: _ = Baz::new();
    let s = Arc::new(Service::new());
    cb.one(); g.two(); d.three(); s.four(); h.five();
}
";
    let got = forms_of(src, &["one", "two", "three", "four", "five"]);
    assert_eq!(
        got,
        vec![
            ("one".to_string(), Form::Method),
            ("two".into(), Form::Method),
            ("three".into(), ty("Baz")),
            // A chain from `Arc::new`: no project type is called `Arc`, so it resolves like a
            // plain method call (see `scope`).
            ("four".into(), hop("new", Form::Type("Arc".into()), false)),
            ("five".into(), Form::Method),
        ]
    );
}

fn hop(callee: &str, form: Form, unwrapped: bool) -> Form {
    Form::Returned(Box::new(Hop {
        callee: callee.into(),
        form,
        unwrapped,
        awaited: false,
        ..Default::default()
    }))
}

#[test]
fn functions_know_what_they_return() {
    let src = "impl Builder {
    fn new() -> Self { todo!() }
    fn name(&mut self) -> &mut Self { self }
    fn build(&self) -> io::Result<Runtime> { todo!() }
    fn find(&self) -> Option<Item> { None }
    fn count(&self) -> usize { 0 }
    fn boxed(&self) -> Box<Foo> { todo!() }
    fn generic<T>(&self) -> T { todo!() }
}
fn free() -> Widget { todo!() }
fn nothing() {}
";
    let facts = extract_file(Lang::Rust, src).unwrap();
    let ret = |name: &str| {
        facts
            .symbols
            .iter()
            .find(|s| s.name == name)
            .and_then(|s| s.returns.clone())
    };
    let r = |ty: &str, fallible| {
        Some(Ret {
            ty: ty.into(),
            fallible,
            future: false,
            ..Default::default()
        })
    };
    assert_eq!(ret("new"), r("Builder", false));
    assert_eq!(ret("name"), r("Builder", false));
    assert_eq!(ret("build"), r("Runtime", true));
    assert_eq!(ret("find"), r("Item", true));
    assert_eq!(ret("count"), None);
    assert_eq!(ret("boxed"), None);
    assert_eq!(ret("generic"), None);
    assert_eq!(ret("free"), r("Widget", false));
    assert_eq!(ret("nothing"), None);
}

#[test]
fn a_method_on_a_call_result_remembers_the_chain() {
    let src = "fn f() {
    Builder::new().name().go();
    make().run();
    Builder::new().build().unwrap().start();
    Builder::new().build()?.stop();
}
";
    let got = forms_of(src, &["go", "run", "start", "stop"]);
    let builder = || Form::Type("Builder".into());
    assert_eq!(
        got,
        vec![
            (
                "go".to_string(),
                hop("name", hop("new", builder(), false), false)
            ),
            ("run".into(), hop("make", Form::Bare, false)),
            (
                "start".into(),
                hop("build", hop("new", builder(), false), true)
            ),
            (
                "stop".into(),
                hop("build", hop("new", builder(), false), true)
            ),
        ]
    );
}

#[test]
fn a_variable_built_from_a_chain_has_that_chain_as_its_type() {
    let src = "fn f() {
    let rt = Builder::new().build().unwrap();
    rt.run();
    let again = Builder::new();
    again.go();
    let rt2 = other();
    let rt2 = third();
    rt2.skip();
}
";
    let got = forms_of(src, &["run", "go", "skip"]);
    let builder = || Form::Type("Builder".into());
    assert_eq!(
        got,
        vec![
            (
                "run".to_string(),
                hop("build", hop("new", builder(), false), true)
            ),
            ("go".into(), ty("Builder")),
            ("skip".into(), Form::Method),
        ]
    );
}

#[test]
fn async_fns_and_awaited_chains() {
    let src = "impl Stream {
    async fn connect(a: A) -> io::Result<Stream> { todo!() }
    fn sync() -> Stream { todo!() }
}
async fn go() {
    let s = Stream::connect(a).await?;
    s.peer();
    Stream::connect(a).await.unwrap().split();
}
";
    let facts = extract_file(Lang::Rust, src).unwrap();
    let ret = |name: &str| {
        facts
            .symbols
            .iter()
            .find(|s| s.name == name)
            .and_then(|s| s.returns.clone())
            .map(|r| (r.fallible, r.future))
    };
    assert_eq!(ret("connect"), Some((true, true)));
    assert_eq!(ret("sync"), Some((false, false)));
    let want = Form::Returned(Box::new(Hop {
        callee: "connect".into(),
        form: Form::Type("Stream".into()),
        unwrapped: true,
        awaited: true,
        ..Default::default()
    }));
    let got = forms_of(src, &["peer", "split"]);
    assert_eq!(
        got,
        vec![("peer".to_string(), want.clone()), ("split".into(), want)]
    );
}

#[test]
fn unwrapping_a_variable_that_holds_a_chain_extends_the_chain() {
    let src = "fn f() {
    let r = build();
    r.unwrap().run();
    let q = Builder::new().build().map_err(wrap)?;
    q.go();
}
";
    let got = forms_of(src, &["run", "go"]);
    assert_eq!(
        got,
        vec![
            ("run".to_string(), hop("build", Form::Bare, true)),
            (
                "go".into(),
                hop(
                    "map_err",
                    hop("build", hop("new", ty("Builder"), false), false),
                    true
                )
            ),
        ]
    );
}

#[test]
fn a_chain_declared_twice_or_over_a_typed_parameter_is_unknown() {
    let src = "fn f(r: Foo) {
    let r = make();
    r.go();
    let s = a();
    let s = b();
    s.go();
    let t = a();
    let t = a();
    t.go();
}
";
    let got = forms_of(src, &["go"]);
    assert_eq!(
        got.iter().map(|(_, f)| f.clone()).collect::<Vec<_>>(),
        vec![Form::Method, Form::Method, hop("a", Form::Bare, false)]
    );
}

#[test]
fn a_very_long_chain_is_read_without_deep_recursion() {
    let src = format!(
        "fn f() {{ Builder::new(){}.last(); }}",
        ".step()".repeat(300)
    );
    let facts = extract_file(Lang::Rust, &src).unwrap();
    assert_eq!(
        facts.calls.iter().filter(|c| c.callee == "step").count(),
        300
    );
    let depth = |f: &Form| {
        let (mut n, mut cur) = (0, f);
        while let Form::Returned(h) = cur {
            n += 1;
            cur = &h.form;
        }
        n
    };
    let last = facts.calls.iter().find(|c| c.callee == "last").unwrap();
    assert!(depth(&last.form) <= 40, "chain depth is capped");
}

#[test]
fn a_function_type_arrow_is_not_a_closing_bracket() {
    assert_eq!(
        super::first_generic_arg("Option<Box<dyn Fn() -> Foo>>"),
        Some("Box<dyn Fn() -> Foo>")
    );
}

#[test]
fn receivers_inside_macro_arguments_are_looked_up() {
    let src = "struct Repo { db: Db }
impl Repo {
    fn run(&self) {
        let s = Stream::new();
        let u = other();
        assert_eq!(s.next(), 1);
        assert_eq!(u.skip(), 2);
        assert_eq!(self.db.query(), 3);
        assert_eq!(z.go(), 4);
        assert_eq!(a.b.go(), 5);
        assert_eq!(make().go(), 6);
        assert_eq!(s.next().await, 7);
        assert_eq!(x::y.go(), 8);
        let c = |v| v;
        assert_eq!(c.go(), 9);
    }
}
fn free() {
    assert_eq!(self.db.query(), 10);
}
";
    let got: Vec<Form> = forms_of(src, &["next", "skip", "query", "go"])
        .into_iter()
        .map(|(_, f)| f)
        .collect();
    assert_eq!(
        got,
        vec![
            ty("Stream"),
            hop("other", Form::Bare, false),
            ty("Db"),
            Form::Method,
            Form::Method,
            Form::Method,
            ty("Stream"),
            Form::Method,
            Form::Method,
            Form::Method,
        ]
    );
}

#[test]
fn tuple_returns_and_destructuring_lets() {
    let src = "fn channel<T>() -> (Sender<T>, Receiver<T>) { todo!() }
fn open() -> io::Result<(Reader, u32)> { todo!() }
fn trio() -> (A, (B, C), D) { todo!() }
fn f() {
    let (tx, rx) = channel();
    tx.send();
    rx.recv();
    let (a, _, c) = trio();
    c.go();
    let (r, n) = open().unwrap();
    r.read();
    n.skip();
}
";
    let facts = extract_file(Lang::Rust, src).unwrap();
    let ret = |name: &str| {
        facts
            .symbols
            .iter()
            .find(|s| s.name == name)
            .and_then(|s| s.returns.clone())
    };
    let r = ret("channel").unwrap();
    assert_eq!(
        (r.ty.as_str(), r.parts),
        ("", vec!["Sender".into(), "Receiver".into()])
    );
    let r = ret("open").unwrap();
    assert_eq!(
        (r.fallible, r.parts),
        (true, vec!["Reader".to_string(), String::new()])
    );
    assert_eq!(ret("trio").unwrap().parts, vec!["A", "", "D"]);
    let part = |callee: &str, n: u32, unwrapped: bool| {
        Form::Returned(Box::new(Hop {
            callee: callee.into(),
            form: Form::Bare,
            unwrapped,
            part: Some(n),
            ..Default::default()
        }))
    };
    let got = forms_of(src, &["send", "recv", "go", "read", "skip"]);
    assert_eq!(
        got,
        vec![
            ("send".to_string(), part("channel", 0, false)),
            ("recv".into(), part("channel", 1, false)),
            ("go".into(), part("trio", 2, false)),
            ("read".into(), part("open", 0, true)),
            ("skip".into(), part("open", 1, true)),
        ]
    );
}

#[test]
fn tuple_patterns_with_rest_comments_types_and_nesting() {
    let src = "fn f() {
    let (a, .., d) = quad();
    d.after_rest();
    a.before_rest();
    let (e, /* skip */ g) = pair(); // c
    g.after_comment();
    let (mut m, (n, o)) = nest();
    m.with_mut();
    n.nested();
    let (p, q): (X, Y) = typed();
    p.annotated();
    let (_, u) = two();
    u.after_underscore();
    let (z, w) = notcall;
    w.no_call();
}
";
    let part = |callee: &str, n: u32| {
        Form::Returned(Box::new(Hop {
            callee: callee.into(),
            form: Form::Bare,
            part: Some(n),
            ..Default::default()
        }))
    };
    let got = forms_of(
        src,
        &[
            "after_rest",
            "before_rest",
            "after_comment",
            "with_mut",
            "nested",
            "annotated",
            "after_underscore",
            "no_call",
        ],
    );
    let want = vec![
        ("after_rest", Form::Method), // after `..` the index from the end is not known
        ("before_rest", part("quad", 0)),
        ("after_comment", part("pair", 1)),
        ("with_mut", part("nest", 0)),
        ("nested", Form::Method),
        ("annotated", Form::Method), // the annotation, not the call, would be authoritative
        ("after_underscore", part("two", 1)),
        ("no_call", Form::Method),
    ];
    let want: Vec<(String, Form)> = want.into_iter().map(|(n, f)| (n.to_string(), f)).collect();
    assert_eq!(got, want);
}

#[test]
fn tuple_elements_edge_cases() {
    assert_eq!(super::tuple_elements("(A, B,)"), Some(vec!["A", "B"]));
    assert_eq!(super::tuple_elements("(A,)"), None);
    assert_eq!(
        super::tuple_elements("(A, Box<dyn Fn() -> B>)"),
        Some(vec!["A", "Box<dyn Fn() -> B>"])
    );
    assert_eq!(
        super::tuple_elements("(A, (B, C))"),
        Some(vec!["A", "(B, C)"])
    );
    assert_eq!(super::tuple_elements("(A, B"), None);
    assert_eq!(super::tuple_elements("A"), None);
}
