//! Rust extraction: walks the syntax tree and records items, `use`/`mod` imports and calls.

use std::collections::{HashMap, HashSet};

use tree_sitter::{Node, Parser};

use super::{Facts, caller_name, field_text, line_of, signature, text};
use crate::model::{Call, Form, Hop, Kind, Ret, Symbol, Use};

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
            if let Some(arg) = node.child_by_field_name("argument") {
                let from = facts.uses.len();
                expand_use(arg, "", is_pub(node), src, &mut facts.uses);
                // Inside `mod tests` or a fn the names are only in scope there.
                if !node.parent().is_some_and(|p| p.kind() == "source_file") {
                    facts.uses[from..].iter_mut().for_each(|u| u.local = true);
                }
            }
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
                token_form(&kids, i, src, ctx)
            };
            add_call(*kid, Some(name), form, facts, ctx);
        }
    }
}

/// The form of a call found among raw macro tokens, from the tokens just before the name:
/// `. name` is a method (`self . name` an own method), `Foo :: name` a member of Foo.
fn token_form(kids: &[Node], at: usize, src: &str, ctx: Ctx) -> Form {
    let before = |back: usize| at.checked_sub(back).map(|i| text(kids[i], src));
    match before(1) {
        Some(".") if before(2) == Some("self") => Form::SelfRef,
        Some(".") => token_receiver(kids, at, src, ctx).map_or(Form::Method, Known::form),
        Some("::") => qualified(before(2).filter(|q| !q.is_empty())),
        _ => Form::Bare,
    }
}

/// The type of `x` in the tokens `x . name (`, or of `field` in `self . field . name (`; any
/// longer path (`a . b . name`) is unknown.
fn token_receiver(kids: &[Node], at: usize, src: &str, ctx: Ctx) -> Option<Known> {
    let recv = *kids.get(at.checked_sub(2)?)?;
    if recv.kind() != "identifier" {
        return None;
    }
    let name = text(recv, src);
    let before = |back: usize| at.checked_sub(back).map(|i| text(kids[i], src));
    match before(3) {
        Some(".") if before(4) == Some("self") => ctx
            .fields?
            .get(&(ctx.owner?.to_string(), name.to_string()))
            .cloned()
            .map(Known::Class),
        Some("." | "::") => None,
        _ => ctx.locals?.get(name).cloned().flatten(),
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
    let (shown, fallible) = if matches!(last, "Result" | "Option") {
        (first_generic_arg(written)?, true)
    } else {
        (written, false)
    };
    if let Some(elements) = tuple_elements(shown) {
        // `(Sender<T>, Receiver<T>)`: the class of each element, `""` where it is not a plain class.
        let parts: Vec<String> = elements
            .iter()
            .map(|e| known_type(e, &generics, container).unwrap_or_default())
            .collect();
        return parts.iter().any(|p| !p.is_empty()).then_some(Ret {
            parts,
            fallible,
            future,
            ..Default::default()
        });
    }
    let ty = known_type(shown, &generics, container)?;
    Some(Ret {
        ty,
        fallible,
        future,
        ..Default::default()
    })
}

/// `(A, B<C, D>, (E, F))` -> `["A", "B<C, D>", "(E, F)"]`; anything that is not a tuple of two or
/// more elements gives `None`.
fn tuple_elements(ty: &str) -> Option<Vec<&str>> {
    let inner = ty.trim().strip_prefix('(')?.strip_suffix(')')?;
    let (mut parts, mut depth, mut start, mut prev) = (Vec::new(), 0i32, 0, ' ');
    for (i, c) in inner.char_indices() {
        let arrow = prev == '-' && c == '>'; // the `->` of a function type is not a bracket
        prev = c;
        match c {
            _ if arrow => {}
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(inner[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = inner[start..].trim();
    if !last.is_empty() {
        parts.push(last);
    }
    (depth == 0 && parts.len() >= 2).then_some(parts)
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
            match (qualified(qualifier), path.map(|p| type_path(text(p, src)))) {
                // Keep the module path of `io::Driver::new()`: two types may share the name.
                // `Self::Item::f()`: an associated type, not a type named `Item`.
                // `::a::T::f()` names a crate `a`; a local `mod a` could shadow it in our lookup.
                (Form::Type(_), Some(full))
                    if full.split("::").any(|s| s == "Self") || full.starts_with("::") =>
                {
                    Form::Unknown
                }
                // `a::Foo<T>::Assoc::f()`: generics mid-path, the receiver is an associated type.
                (Form::Type(_), _) if path.is_some_and(|p| text(p, src).contains(">::")) => {
                    Form::Unknown
                }
                (Form::Type(_), Some(full)) if full.contains("::") => {
                    Form::Type(full.split_whitespace().collect())
                }
                (form, _) => form,
            }
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
                part: None,
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
                if let Some(p) = pattern.filter(|p| p.kind() == "tuple_pattern") {
                    // `let (a, b) = call()`: each plain name is one element of the returned tuple.
                    let hop = n
                        .child_by_field_name("value")
                        .and_then(|v| hop_of(v, src, chain_ctx, 0));
                    let mut hop_rest = false;
                    let mut cursor = p.walk();
                    let elements = p.children(&mut cursor).filter(|c| {
                        !matches!(c.kind(), "(" | ")" | "," | "line_comment" | "block_comment")
                    });
                    // A type annotation is authoritative and not read here: names stay unknown.
                    let hop = hop.filter(|_| n.child_by_field_name("type").is_none());
                    for (i, element) in elements.enumerate() {
                        if text(element, src) == ".." {
                            // Later elements are counted from the end: their index is unknown.
                            hop_rest = true;
                        }
                        let hop = if hop_rest { &None } else { &hop };
                        let plain = match element.kind() {
                            "mut_pattern" => {
                                let mut inner = element.walk();
                                element
                                    .named_children(&mut inner)
                                    .find(|c| c.kind() == "identifier")
                            }
                            _ => Some(element),
                        }
                        .filter(|e| e.kind() == "identifier");
                        match (plain, hop) {
                            (Some(e), Some(h)) => declare(
                                text(e, src).to_string(),
                                Some(Known::Chain(Hop {
                                    part: Some(i as u32),
                                    ..h.clone()
                                })),
                            ),
                            _ => {
                                let mut names = Vec::new();
                                pattern_names(element, src, &mut names);
                                names.into_iter().for_each(|name| declare(name, None));
                            }
                        }
                    }
                } else if let Some(p) = pattern {
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

/// Flatten a `use` tree into one `Use` per name: `a::{b, c as d}` gives `b = a::b`, `d = a::c`.
/// Globs add nothing; `self` in a list names the list's own path.
fn expand_use(node: Node, prefix: &str, public: bool, src: &str, out: &mut Vec<Use>) {
    let join = |tail: &str| match prefix {
        "" => tail.to_string(),
        p => format!("{p}::{tail}"),
    };
    let path_of = |n: Node| text(n, src).split_whitespace().collect::<String>();
    match node.kind() {
        "identifier" | "scoped_identifier" | "crate" | "self" | "super" => {
            let path = join(&path_of(node));
            let mut segments = path.rsplit("::");
            let last = segments.next().unwrap_or("");
            let alias = if last == "self" {
                segments.next().unwrap_or("")
            } else {
                last
            };
            let path = path.strip_suffix("::self").unwrap_or(&path).to_string();
            if !alias.is_empty() {
                out.push(Use {
                    alias: alias.to_string(),
                    path,
                    public,
                    local: false,
                });
            }
        }
        "use_as_clause" => {
            if let (Some(p), Some(a)) = (
                node.child_by_field_name("path"),
                node.child_by_field_name("alias"),
            ) {
                let path = join(&path_of(p));
                out.push(Use {
                    alias: text(a, src).to_string(),
                    path: path.strip_suffix("::self").unwrap_or(&path).to_string(),
                    public,
                    local: false,
                });
            }
        }
        "scoped_use_list" => {
            let base = node
                .child_by_field_name("path")
                .map_or(prefix.to_string(), |p| join(&path_of(p)));
            if let Some(list) = node.child_by_field_name("list") {
                expand_use(list, &base, public, src, out);
            }
        }
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                expand_use(child, prefix, public, src, out);
            }
        }
        _ => {}
    }
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
mod tests;
