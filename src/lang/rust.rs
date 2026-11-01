//! Rust extraction: walks the syntax tree and records items, `use`/`mod` imports and calls.

use std::collections::{HashMap, HashSet};

use tree_sitter::{Node, Parser};

use super::{Facts, caller_name, field_text, line_of, signature, text};
use crate::model::{Call, Form, Hop, Kind, Ret, Symbol};

/// Where a walk is: the innermost `fn` (the caller of what it finds), the type or trait whose
/// method that `fn` is (the `owner` of its calls) and the type or trait that items declared here
/// are members of (their `container`). A `fn` nested in a method has neither: it has no `self`.
#[derive(Clone, Copy, Default)]
struct Ctx<'a> {
    enclosing: Option<&'a str>,
    owner: Option<&'a str>,
    container: Option<&'a str>,
    /// The nearest enclosing body is an `impl` or `trait` (not a function): its `fn`s are methods.
    in_type_body: bool,
    /// Declared names of the current `fn`, with their type when it is known.
    locals: Option<&'a Locals>,
    /// Field types of the structs in this file, by (struct, field).
    fields: Option<&'a Fields>,
}

/// Name -> `Some(type)`, or `None` when the name is declared but its type is not known (declared
/// twice, a pattern, a closure parameter, a generic).
type Locals = HashMap<String, Option<Known>>;

/// What is known of a name's type: its class, or the call chain that produced it (resolved later,
/// when every file is in view).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Known {
    Class(String),
    Chain(Hop),
}

impl Known {
    fn form(self) -> Form {
        match self {
            Known::Class(t) => Form::Type(t),
            Known::Chain(hop) => Form::Returned(Box::new(hop)),
        }
    }
}
type Fields = HashMap<(String, String), String>;

pub fn extract(root: Node, src: &str) -> Facts {
    let mut facts = Facts::default();
    let fields = struct_fields(root, src);
    let ctx = Ctx {
        fields: Some(&fields),
        ..Ctx::default()
    };
    visit(root, src, &mut facts, ctx);
    facts
}

/// Depth-first walk.
fn visit(node: Node, src: &str, facts: &mut Facts, ctx: Ctx) {
    let mut own = None;
    let mut inner = ctx;
    let impl_type = (node.kind() == "impl_item")
        .then(|| field_text(node, "type", src).map(type_name))
        .flatten();
    match node.kind() {
        "function_item" => {
            own = add_item(node, Kind::Fn, src, facts, ctx.container).map(|s| caller_name(&s));
            // What is declared inside a function belongs to no type; a nested `fn` has no `self`.
            inner.container = None;
            if !ctx.in_type_body {
                inner.owner = None;
            }
            inner.in_type_body = false;
        }
        "struct_item" => _ = add_item(node, Kind::Struct, src, facts, ctx.container),
        "enum_item" => _ = add_item(node, Kind::Enum, src, facts, ctx.container),
        "trait_item" => {
            add_item(node, Kind::Trait, src, facts, ctx.container);
            let name = field_text(node, "name", src);
            (inner.owner, inner.container) = (name, name);
            inner.in_type_body = true;
        }
        "type_item" => _ = add_item(node, Kind::Type, src, facts, ctx.container),
        "mod_item" => {
            add_item(node, Kind::Mod, src, facts, ctx.container);
            if node.child_by_field_name("body").is_none() {
                facts
                    .imports
                    .extend(field_text(node, "name", src).map(String::from));
            }
        }
        "impl_item" => {
            add_impl(node, src, facts);
            (inner.owner, inner.container) = (impl_type, impl_type);
            inner.in_type_body = true;
        }
        "const_item" if is_pub(node) => {
            _ = add_item(node, Kind::Const, src, facts, ctx.container);
        }
        "use_declaration" => {
            facts.imports.extend(
                node.child_by_field_name("argument")
                    .and_then(|a| use_root(text(a, src))),
            );
        }
        "call_expression" => {
            let function = node.child_by_field_name("function");
            let callee = function.and_then(|f| callee_name(f, src));
            let form = function.map_or(Form::Unknown, |f| form_of(f, src, ctx, 0));
            add_call(node, callee, form, facts, ctx);
        }
        "macro_invocation" => {
            let callee = node
                .child_by_field_name("macro")
                .and_then(|f| callee_name(f, src));
            add_call(node, callee, Form::Unknown, facts, ctx);
            let before = (facts.symbols.len(), facts.calls.len());
            if callee.is_some_and(|name| name.starts_with("cfg_")) {
                visit_macro_items(node, src, facts, ctx);
            }
            // Not items (e.g. `cfg_if!`) or not a cfg macro: scan the raw tokens instead.
            if before == (facts.symbols.len(), facts.calls.len())
                && let Some(args) = macro_args(node)
            {
                visit_token_calls(args, src, facts, ctx);
            }
        }
        _ => {}
    }
    inner.enclosing = own.as_deref().or(ctx.enclosing);
    let locals =
        (node.kind() == "function_item").then(|| local_types(node, src, ctx.owner, ctx.fields));
    if let Some(l) = &locals {
        inner.locals = Some(l);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, src, facts, inner);
    }
}

/// Macros like tokio's `cfg_net! { ... }` wrap whole items, which tree-sitter only sees as
/// raw tokens. Re-parse the macro body as ordinary Rust and shift its line numbers back.
fn visit_macro_items(node: Node, src: &str, facts: &mut Facts, ctx: Ctx) {
    let mut cursor = node.walk();
    let Some(body) = node
        .named_children(&mut cursor)
        .find(|c| c.kind() == "token_tree")
    else {
        return;
    };
    // Skip the opening and closing delimiter.
    let Some(inner) = src.get(body.start_byte() + 1..body.end_byte().saturating_sub(1)) else {
        return;
    };
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .is_err()
    {
        return;
    }
    let Some(tree) = parser.parse(inner, None) else {
        return;
    };
    let (symbols, calls) = (facts.symbols.len(), facts.calls.len());
    visit(tree.root_node(), inner, facts, ctx);
    let shift = u32::try_from(body.start_position().row).unwrap_or(0);
    facts.symbols[symbols..]
        .iter_mut()
        .for_each(|s| s.line += shift);
    facts.calls[calls..]
        .iter_mut()
        .for_each(|c| c.line += shift);
}

/// The `( ... )` / `[ ... ]` / `{ ... }` part of a macro call, as raw tokens.
fn macro_args(node: Node) -> Option<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|c| c.kind() == "token_tree")
}

/// Words that can be followed by `(` without being a call.
const NOT_CALLS: [&str; 15] = [
    "fn", "if", "while", "match", "for", "return", "loop", "move", "async", "unsafe", "in", "else",
    "dyn", "ref", "yield",
];

/// Macro arguments are raw tokens, not a syntax tree, so look for the shapes of a call:
/// `name (...)` is a function or method call, `name ! (...)` a nested macro call.
fn visit_token_calls(tokens: Node, src: &str, facts: &mut Facts, ctx: Ctx) {
    let mut cursor = tokens.walk();
    let kids: Vec<Node> = tokens.children(&mut cursor).collect();
    for (i, kid) in kids.iter().enumerate() {
        if kid.kind() == "token_tree" {
            visit_token_calls(*kid, src, facts, ctx);
            continue;
        }
        let name = text(*kid, src);
        let after_fn = i > 0 && text(kids[i - 1], src) == "fn";
        if kid.kind() != "identifier" || after_fn || NOT_CALLS.contains(&name) {
            continue;
        }
        let paren_next = kids
            .get(i + 1)
            .is_some_and(|n| n.kind() == "token_tree" && text(*n, src).starts_with('('));
        let macro_next = kids.get(i + 1).is_some_and(|n| n.kind() == "!")
            && kids.get(i + 2).is_some_and(|n| n.kind() == "token_tree");
        if paren_next || macro_next {
            let form = if macro_next {
                Form::Unknown
            } else {
                token_form(&kids, i, src)
            };
            add_call(*kid, Some(name), form, facts, ctx);
        }
    }
}

/// The form of a call found among raw macro tokens, from the tokens just before the name:
/// `. name` is a method (`self . name` an own method), `Foo :: name` a member of Foo.
fn token_form(kids: &[Node], at: usize, src: &str) -> Form {
    let before = |back: usize| at.checked_sub(back).map(|i| text(kids[i], src));
    match before(1) {
        Some(".") if before(2) == Some("self") => Form::SelfRef,
        Some(".") => Form::Method,
        Some("::") => qualified(before(2).filter(|q| !q.is_empty())),
        _ => Form::Bare,
    }
}

/// Record a named item; returns its signature.
fn add_item(
    node: Node,
    kind: Kind,
    src: &str,
    facts: &mut Facts,
    container: Option<&str>,
) -> Option<String> {
    let name = field_text(node, "name", src)?;
    let sig = signature(node, node.child_by_field_name("body"), src);
    facts.symbols.push(Symbol {
        name: name.to_string(),
        kind,
        line: line_of(node),
        signature: sig.clone(),
        container: container.map(String::from),
        returns: (kind == Kind::Fn)
            .then(|| returns_of(node, src, container))
            .flatten(),
    });
    Some(sig)
}

/// What a `fn` returns, when the signature names one plain class: `-> Self` is the container,
/// `-> Result<T, _>` / `-> io::Result<T>` / `-> Option<T>` is `T` (fallible), a generic or a
/// primitive says nothing.
fn returns_of(func: Node, src: &str, container: Option<&str>) -> Option<Ret> {
    let written = text(func.child_by_field_name("return_type")?, src);
    let generics = generic_names(func, src);
    let future = {
        let mut cursor = func.walk();
        func.children(&mut cursor)
            .any(|c| c.kind() == "function_modifiers" && text(c, src).contains("async"))
    };
    let path = type_path(written);
    let last = path.rsplit("::").next().unwrap_or(path);
    if matches!(last, "Result" | "Option") {
        let inner = first_generic_arg(written)?;
        let ty = known_type(inner, &generics, container)?;
        return Some(Ret {
            ty,
            fallible: true,
            future,
        });
    }
    let ty = known_type(written, &generics, container)?;
    Some(Ret {
        ty,
        fallible: false,
        future,
    })
}

/// `io::Result<Foo>` -> `Foo`, `Result<Foo, E>` -> `Foo`, `Option<Vec<u8>>` -> `Vec<u8>`.
fn first_generic_arg(ty: &str) -> Option<&str> {
    let start = ty.find('<')? + 1;
    let mut depth = 0;
    let mut prev = ' ';
    for (i, c) in ty[start..].char_indices() {
        let arrow = prev == '-' && c == '>'; // the `->` of a function type is not a bracket
        prev = c;
        if arrow {
            continue;
        }
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' if depth == 0 => return Some(ty[start..start + i].trim()),
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => return Some(ty[start..start + i].trim()),
            _ => {}
        }
    }
    None
}

/// An `impl` has no name of its own, so it is named after the type it implements.
fn add_impl(node: Node, src: &str, facts: &mut Facts) {
    let Some(ty) = field_text(node, "type", src) else {
        return;
    };
    facts.symbols.push(Symbol {
        name: ty.to_string(),
        kind: Kind::Impl,
        line: line_of(node),
        signature: signature(node, node.child_by_field_name("body"), src),
        container: None,
        returns: None,
    });
}

fn add_call(node: Node, callee: Option<&str>, form: Form, facts: &mut Facts, ctx: Ctx) {
    if let Some(callee) = callee {
        facts.calls.push(Call {
            callee: callee.to_string(),
            line: line_of(node),
            caller: ctx.enclosing.map(String::from),
            form,
            owner: ctx.owner.map(String::from),
        });
    }
}

/// `Foo<T>`, `&'a mut Foo`, `dyn Foo`, `a::b::Foo<T>`, `Vec::<u8>` -> `a::b::Foo` and so on: the
/// path of a written type without references, lifetimes, `dyn`/`impl` and generic arguments.
fn type_path(ty: &str) -> &str {
    let mut ty = ty.trim();
    while let Some(rest) = ty
        .strip_prefix('&')
        .or_else(|| ty.strip_prefix("mut "))
        .or_else(|| ty.strip_prefix("dyn "))
        .or_else(|| ty.strip_prefix("impl "))
        .or_else(|| {
            ty.starts_with('\'')
                .then(|| ty.split_once(char::is_whitespace).map(|(_, rest)| rest))
                .flatten()
        })
    {
        ty = rest.trim_start();
    }
    ty.split('<').next().unwrap_or(ty).trim_end_matches("::")
}

/// `Foo<T>`, `&'a mut Foo`, `dyn Foo`, `a::b::Foo<T>`, `Vec::<u8>` -> `Foo`, `Foo`, `Foo`, `Foo`, `Vec`.
fn type_name(ty: &str) -> &str {
    let path = type_path(ty);
    path.rsplit("::").next().unwrap_or(path).trim()
}

/// Primitive types: `u64::from(x)` is a method of a std type, not a function of a module `u64`.
const PRIMITIVES: [&str; 17] = [
    "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32",
    "f64", "bool", "char", "str",
];

/// `Self::f` own method, `Foo::f` member of Foo, `crate::f` / `self::f` / `super::f` a plain call,
/// `m::f` a function of module m. A qualifier that is no plain name (`<T as Tr>`, `>` after a
/// turbofish) or a primitive type says nothing we can use.
fn qualified(qualifier: Option<&str>) -> Form {
    let word = |q: &str| q.starts_with(|c: char| c.is_alphabetic() || c == '_');
    match qualifier {
        Some("Self") => Form::SelfRef,
        Some("crate" | "super" | "self") => Form::Bare,
        Some(q) if !word(q) || PRIMITIVES.contains(&q) => Form::Unknown,
        Some(q) if q.starts_with(|c: char| c.is_uppercase()) => Form::Type(q.to_string()),
        Some(q) => Form::Module(q.to_string()),
        None => Form::Unknown,
    }
}

/// Longest call chain read; a longer one is a plain method call.
const MAX_CHAIN: usize = 32;

/// How a call is written: `f()` bare, `self.f()` / `Self::f()` own method, `Foo::f()` a member of
/// type Foo, `m::f()` a function of module m, `x.f()` a method of an unseen type.
fn form_of(f: Node, src: &str, ctx: Ctx, depth: usize) -> Form {
    match f.kind() {
        "identifier" => Form::Bare,
        "generic_function" => f
            .child_by_field_name("function")
            .map_or(Form::Unknown, |inner| form_of(inner, src, ctx, depth)),
        "field_expression" => match f.child_by_field_name("value") {
            Some(v) if text(v, src) == "self" => Form::SelfRef,
            Some(v) => receiver_type(v, src, ctx)
                .or_else(|| hop_of(v, src, ctx, depth + 1).map(Known::Chain))
                .map_or(Form::Method, Known::form),
            None => Form::Unknown,
        },
        "scoped_identifier" => {
            let path = f.child_by_field_name("path");
            if path.is_some_and(|p| p.kind() == "bracketed_type") {
                return Form::Unknown; // `<T as Trait>::f`, `<Foo>::f`
            }
            let qualifier = path.map(|p| type_name(text(p, src)));
            qualified(qualifier)
        }
        _ => Form::Unknown,
    }
}

/// The declared type of a method call's receiver: a local with a known type, or `self.field`.
fn receiver_type(value: Node, src: &str, ctx: Ctx) -> Option<Known> {
    match value.kind() {
        "identifier" => ctx.locals?.get(text(value, src)).cloned().flatten(),
        "field_expression" => {
            let base = value.child_by_field_name("value")?;
            let field = field_text(value, "field", src)?;
            if text(base, src) != "self" {
                return None;
            }
            ctx.fields?
                .get(&(ctx.owner?.to_string(), field.to_string()))
                .cloned()
                .map(Known::Class)
        }
        _ => None,
    }
}

/// The call whose result `expr` is: `a()`, `T::new()`, `x.f()`, optionally followed by `?`,
/// `.unwrap()` or `.expect(..)`. Anything that is not such a call has no hop.
fn hop_of(expr: Node, src: &str, ctx: Ctx, depth: usize) -> Option<Hop> {
    if depth >= MAX_CHAIN {
        return None;
    }
    match expr.kind() {
        "await_expression" => {
            let mut hop = hop_of(expr.named_child(0)?, src, ctx, depth)?;
            hop.awaited = true;
            Some(hop)
        }
        "try_expression" | "parenthesized_expression" => {
            let mut hop = hop_of(expr.named_child(0)?, src, ctx, depth)?;
            hop.unwrapped |= expr.kind() == "try_expression";
            Some(hop)
        }
        "call_expression" => {
            let function = expr.child_by_field_name("function")?;
            let callee = callee_name(function, src)?;
            if function.kind() == "field_expression" && matches!(callee, "unwrap" | "expect") {
                let value = function.child_by_field_name("value")?;
                // `r.unwrap()` where `let r = build();` continues the chain stored in `r`.
                let mut hop = match receiver_type(value, src, ctx) {
                    Some(Known::Chain(hop)) => hop,
                    _ => hop_of(value, src, ctx, depth)?,
                };
                hop.unwrapped = true;
                return Some(hop);
            }
            Some(Hop {
                callee: callee.to_string(),
                form: form_of(function, src, ctx, depth + 1),
                unwrapped: false,
                awaited: false,
            })
        }
        _ => None,
    }
}

/// A written type as a class we can look up: `&mut Foo<T>` -> `Foo`; primitives, tuples, arrays,
/// function types, type parameters and anything that is not a plain name give `None`.
fn known_type(ty: &str, generics: &HashSet<String>, owner: Option<&str>) -> Option<String> {
    let name = type_name(ty);
    // `I::Item`, `Self::Output`, `T::Error`: an associated type, not a class named like its tail.
    let path = type_path(ty);
    let head = path.split("::").next().unwrap_or(path);
    if path.contains("::") && (head == "Self" || generics.contains(head)) {
        return None;
    }
    let plain = name.starts_with(|c: char| c.is_alphabetic() || c == '_')
        && name != "_"
        && name.chars().all(|c| c.is_alphanumeric() || c == '_');
    // Smart pointers hand their method calls to what they wrap: their own name says nothing.
    const WRAPPERS: [&str; 8] = [
        "Box", "Rc", "Arc", "RefCell", "Mutex", "RwLock", "Cow", "Pin",
    ];
    if !plain || PRIMITIVES.contains(&name) || generics.contains(name) || WRAPPERS.contains(&name) {
        return None;
    }
    match name {
        "Self" => owner.map(String::from),
        _ => Some(name.to_string()),
    }
}

/// The class a value expression builds: `Foo::new(..)`, `Foo::default()`, `Foo { .. }`, each
/// optionally followed by `?` or `.await`. Anything else (a function that returns who knows what)
/// says nothing.
fn built_type(
    value: Node,
    src: &str,
    generics: &HashSet<String>,
    owner: Option<&str>,
) -> Option<String> {
    match value.kind() {
        "try_expression" | "await_expression" => {
            built_type(value.named_child(0)?, src, generics, owner)
        }
        "struct_expression" => {
            // `Shape::Circle { .. }` builds an enum variant of `Shape`.
            let path = type_path(field_text(value, "name", src)?);
            let mut segments = path.rsplit("::");
            let last = segments.next()?;
            let class = match segments.next() {
                Some(parent) if parent.starts_with(|c: char| c.is_uppercase()) => parent,
                _ => last,
            };
            known_type(class, generics, owner)
        }
        "call_expression" => {
            let f = value.child_by_field_name("function")?;
            if f.kind() != "scoped_identifier"
                || !matches!(field_text(f, "name", src)?, "new" | "default")
            {
                return None;
            }
            let class = type_name(text(f.child_by_field_name("path")?, src));
            class
                .starts_with(|c: char| c.is_uppercase())
                .then(|| known_type(class, generics, owner))
                .flatten()
        }
        _ => None,
    }
}

/// The type parameters (`<T: Tr, U>`) of `node` and of the items around it.
fn generic_names(node: Node, src: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut up = Some(node);
    while let Some(n) = up {
        if let Some(list) = n.child_by_field_name("type_parameters") {
            let mut cursor = list.walk();
            for param in list.named_children(&mut cursor) {
                let name = match param.kind() {
                    "type_parameter" => field_text(param, "name", src),
                    _ => None,
                };
                names.extend(name.map(String::from));
            }
        }
        up = n.parent();
    }
    names
}

/// Every plain name a pattern binds (and some paths such as `Some`, which do no harm).
fn pattern_names(pattern: Node, src: &str, out: &mut Vec<String>) {
    if matches!(pattern.kind(), "identifier" | "shorthand_field_identifier") {
        out.push(text(pattern, src).to_string());
        return;
    }
    let mut cursor = pattern.walk();
    for child in pattern.named_children(&mut cursor) {
        pattern_names(child, src, out);
    }
}

/// The declared types of the names a `fn` introduces (parameters, `let`, patterns, closure
/// parameters), not looking into nested items. A name declared twice with different types, in a
/// pattern or as a closure parameter is unknown: no type is better than a wrong one.
fn local_types(func: Node, src: &str, owner: Option<&str>, fields: Option<&Fields>) -> Locals {
    let generics = generic_names(func, src);
    // Chains in a `let` are read before the names of this fn are known: only their roots count.
    let chain_ctx = Ctx {
        owner,
        fields,
        ..Ctx::default()
    };
    let mut seen: Locals = HashMap::new();
    let mut declare = |name: String, ty: Option<Known>| {
        seen.entry(name)
            .and_modify(|old| {
                if *old != ty {
                    *old = None;
                }
            })
            .or_insert(ty);
    };
    let mut stack = vec![func];
    while let Some(n) = stack.pop() {
        let typed = |decl: Node, pattern: Option<Node>| -> Option<Known> {
            pattern.filter(|p| p.kind() == "identifier")?;
            decl.child_by_field_name("type")
                .and_then(|t| known_type(text(t, src), &generics, owner))
                .map(Known::Class)
        };
        match n.kind() {
            "parameter" | "let_declaration" => {
                let pattern = n.child_by_field_name("pattern");
                let ty = typed(n, pattern).or_else(|| {
                    let value = n.child_by_field_name("value")?;
                    // `let x = T::new()` shows the class; `let x = a().b()?` shows a chain.
                    built_type(value, src, &generics, owner)
                        .map(Known::Class)
                        .or_else(|| hop_of(value, src, chain_ctx, 0).map(Known::Chain))
                });
                if let Some(p) = pattern {
                    let mut names = Vec::new();
                    pattern_names(p, src, &mut names);
                    let single = p.kind() == "identifier";
                    for name in names {
                        declare(name, if single { ty.clone() } else { None });
                    }
                }
            }
            "for_expression" | "let_condition" | "match_arm" => {
                if let Some(p) = n.child_by_field_name("pattern") {
                    let mut names = Vec::new();
                    pattern_names(p, src, &mut names);
                    names.into_iter().for_each(|name| declare(name, None));
                }
            }
            "closure_parameters" => {
                let mut cursor = n.walk();
                for param in n
                    .named_children(&mut cursor)
                    .filter(|c| c.kind() != "parameter")
                {
                    let mut names = Vec::new();
                    pattern_names(param, src, &mut names);
                    names.into_iter().for_each(|name| declare(name, None));
                }
            }
            _ => {}
        }
        let mut cursor = n.walk();
        let children: Vec<_> = n.named_children(&mut cursor).collect();
        stack.extend(children.into_iter().rev().filter(|c| {
            !matches!(
                c.kind(),
                "function_item" | "impl_item" | "trait_item" | "mod_item"
            )
        }));
    }
    seen
}

/// Field types of every `struct` in a file, by (struct name, field name). Two structs of the
/// same name (in different modules) that disagree about a field leave it out.
fn struct_fields(root: Node, src: &str) -> Fields {
    let mut seen: HashMap<(String, String), Option<String>> = HashMap::new();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.kind() == "struct_item"
            && let (Some(name), Some(body)) =
                (field_text(n, "name", src), n.child_by_field_name("body"))
        {
            let generics = generic_names(n, src);
            let mut cursor = body.walk();
            for field in body.named_children(&mut cursor) {
                let Some(fname) = field_text(field, "name", src) else {
                    continue;
                };
                let known = field
                    .child_by_field_name("type")
                    .and_then(|ty| known_type(text(ty, src), &generics, Some(name)));
                seen.entry((name.to_string(), fname.to_string()))
                    .and_modify(|old| {
                        if *old != known {
                            *old = None;
                        }
                    })
                    .or_insert(known);
            }
        }
        let mut cursor = n.walk();
        stack.extend(n.named_children(&mut cursor));
    }
    seen.into_iter()
        .filter_map(|(key, ty)| Some((key, ty?)))
        .collect()
}

/// The called name: `f()` -> f, `a::f()` -> f, `x.f()` -> f, `f::<T>()` -> f.
fn callee_name<'a>(f: Node, src: &'a str) -> Option<&'a str> {
    match f.kind() {
        "identifier" => Some(text(f, src)),
        "scoped_identifier" => field_text(f, "name", src),
        "field_expression" => field_text(f, "field", src),
        "generic_function" => callee_name(f.child_by_field_name("function")?, src),
        _ => None,
    }
}

fn is_pub(node: Node) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|c| c.kind() == "visibility_modifier")
}

/// Local-crate name behind a `use` path: `crate::util::x` -> util, `serde::X` -> serde, `std::..` -> none.
fn use_root(path: &str) -> Option<String> {
    let mut parts = path.trim_start_matches("::").split("::");
    let mut first = parts.next()?;
    if matches!(first, "crate" | "super" | "self") {
        first = parts.next()?;
    }
    let name: String = first
        .chars()
        .take_while(|c| c.is_ascii_lowercase() || *c == '_')
        .collect();
    let skip = matches!(
        name.as_str(),
        "" | "std" | "core" | "alloc" | "self" | "super"
    );
    (!skip && name.len() == first.len()).then_some(name)
}

#[cfg(test)]
mod tests {
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
                ("q", ty("Baz"), Some("Foo")),
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
        let src =
            "cfg_a! {\n    cfg_b! {\n        fn deep() { x(); }\n    }\n    fn after() {}\n}\n";
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
        let src = "cfg_if! {\n    if #[cfg(unix)] {\n        f();\n    } else {\n        g();\n    }\n}\n";
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
}
