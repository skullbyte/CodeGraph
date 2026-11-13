//! TypeScript/TSX extraction: walks the syntax tree and records declarations, imports and calls.

use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use super::{Facts, caller_name, field_text, line_of, signature, text};
use crate::model::{Call, Form, Hop, Kind, Ret, Symbol, Use};

pub fn extract(root: Node, src: &str) -> Facts {
    let mut facts = Facts::default();
    let modules = module_names(root, src);
    let top = local_types(root, src, &Locals::new());
    let ctx = Ctx {
        enclosing: None,
        container: None,
        modules: &modules,
        locals: &top,
        fields: None,
    };
    visit(root, src, &mut facts, ctx);
    facts
}

/// Where the walk is: the innermost function (`enclosing`), the class or interface whose members
/// we are reading (`container`, also the owner of the calls inside), and which local names are
/// imported from another file (name -> module name, see `module_stem`). `locals` are the declared
/// types of parameters and variables in reach, `fields` those of the current class's members
/// (both name -> type name), which tell what `x.f()` and `this.x.f()` are called on.
#[derive(Clone, Copy)]
struct Ctx<'a> {
    enclosing: Option<&'a str>,
    container: Option<&'a str>,
    modules: &'a HashMap<String, String>,
    locals: &'a Locals<'a>,
    fields: Option<&'a Types>,
}

type Types = HashMap<String, String>;
/// Declared names in reach: what each holds, or `None` when the name is declared but unknown.
type Locals<'a> = HashMap<String, Option<Known<'a>>>;

/// What a declared name holds: an instance of a class (`x: Foo`, `new Foo()`), or the value of
/// `const x = <expression>`, read as a call chain only where `x` is used.
#[derive(Clone, PartialEq)]
enum Known<'a> {
    Class(String),
    Value(Node<'a>),
}

const FUNCTIONS: [&str; 5] = [
    "function_declaration",
    "generator_function_declaration",
    "method_definition",
    "arrow_function",
    "function_expression",
];

/// Depth-first walk.
fn visit(node: Node, src: &str, facts: &mut Facts, ctx: Ctx) {
    let mut own = None;
    let mut inner = ctx;
    match node.kind() {
        "function_declaration" | "generator_function_declaration" => {
            own = add_item(node, Kind::Fn, src, facts, None).map(|s| caller_name(&s));
            inner.container = None; // a nested function has no `this`
            inner.fields = None;
        }
        "function_signature" => _ = add_item(node, Kind::Fn, src, facts, None),
        "method_definition" => {
            own = add_item(node, Kind::Fn, src, facts, ctx.container).map(|s| caller_name(&s));
        }
        "method_signature" | "abstract_method_signature" if in_member_body(node) => {
            _ = add_item(node, Kind::Fn, src, facts, ctx.container);
        }
        "class_declaration" | "abstract_class_declaration" => {
            add_item(node, Kind::Class, src, facts, None);
            inner.container = field_text(node, "name", src);
        }
        "interface_declaration" => {
            add_item(node, Kind::Interface, src, facts, None);
            inner.container = field_text(node, "name", src);
        }
        // `this` inside these is not the surrounding class.
        "object" | "function_expression" => (inner.container, inner.fields) = (None, None),
        "class" => (inner.container, inner.fields) = (declarator_name(node, src), None),
        "type_alias_declaration" => _ = add_item(node, Kind::Type, src, facts, None),
        "enum_declaration" => _ = add_item(node, Kind::Enum, src, facts, None),
        "variable_declarator" => own = add_variable(node, src, facts),
        "import_statement" | "export_statement" => add_import(node, src, facts),
        "call_expression" => add_call(node, src, facts, ctx),
        "new_expression" => {
            if let Some(class) = node.child_by_field_name("constructor") {
                add_class_use(node, class, src, facts, ctx);
            }
        }
        "jsx_opening_element" | "jsx_self_closing_element" => add_jsx(node, src, facts, ctx),
        _ => {}
    }
    inner.enclosing = own.as_deref().or(ctx.enclosing);
    let locals = FUNCTIONS
        .contains(&node.kind())
        .then(|| local_types(node, src, ctx.locals));
    if let Some(l) = &locals {
        inner.locals = l;
    }
    let fields = matches!(
        node.kind(),
        "class_declaration" | "abstract_class_declaration" | "class"
    )
    .then(|| field_types(node, src));
    if let Some(f) = &fields {
        inner.fields = Some(f);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, src, facts, inner);
    }
}

/// Local names bound by relative imports (`import * as z from './core/index.js'`,
/// `import { helper } from './util'`) -> the module name a call through them can be found in.
/// Aliased specifiers (`a as b`) are left out: the local name is not the definition's name.
/// Known gaps, on purpose: only top-level imports are read, a local variable that shadows an
/// import is not noticed, and a namespace from a package becomes a module name too (`z` from
/// `zod/v3` -> `v3`, needed to find `z.custom()`); `super.f()` stays a plain method call.
fn module_names(root: Node, src: &str) -> HashMap<String, String> {
    let mut names = HashMap::new();
    let mut cursor = root.walk();
    for stmt in root.named_children(&mut cursor) {
        if stmt.kind() != "import_statement" {
            continue;
        }
        let Some(spec) = field_text(stmt, "source", src) else {
            continue;
        };
        let Some(stem) = module_stem(spec) else {
            continue;
        };
        let relative = spec.trim_matches(['\'', '"', '`']).starts_with('.');
        let mut c = stmt.walk();
        for clause in stmt
            .named_children(&mut c)
            .filter(|n| n.kind() == "import_clause")
        {
            let mut cc = clause.walk();
            for part in clause.named_children(&mut cc) {
                match part.kind() {
                    "identifier" if relative => {
                        names.insert(text(part, src).to_string(), stem.clone());
                    }
                    "namespace_import" => {
                        if let Some(id) = part.named_child(0) {
                            names.insert(text(id, src).to_string(), stem.clone());
                        }
                    }
                    "named_imports" if relative => collect_specifiers(part, src, &stem, &mut names),
                    // `import { z } from "zod"`: `z` may be a namespace the package exports.
                    "named_imports" => collect_namespaces(part, src, spec, &mut names),
                    _ => {}
                }
            }
        }
    }
    names
}

/// Names imported from a package, as `spec::name` (`zod/v4::z`): read only as the receiver of
/// a call (`z.string()`), where `src/tspath.rs` follows them to the namespace they may be.
fn collect_namespaces(named: Node, src: &str, spec: &str, names: &mut HashMap<String, String>) {
    let spec = spec.trim_matches(['\'', '"', '`']);
    let mut cursor = named.walk();
    for one in named.named_children(&mut cursor) {
        let Some(name) = field_text(one, "name", src).filter(|_| one.kind() == "import_specifier")
        else {
            continue;
        };
        let local = field_text(one, "alias", src).unwrap_or(name);
        names.insert(local.to_string(), format!("{spec}::{name}"));
    }
}

fn collect_specifiers(named: Node, src: &str, stem: &str, names: &mut HashMap<String, String>) {
    let mut cursor = named.walk();
    for spec in named.named_children(&mut cursor) {
        let plain =
            spec.kind() == "import_specifier" && spec.child_by_field_name("alias").is_none();
        if let Some(name) = field_text(spec, "name", src).filter(|_| plain) {
            names.insert(name.to_string(), stem.to_string());
        }
    }
}

/// `./price` -> `price`, `../core/index.js` -> `core`, `zod/v3` -> `v3`, `@scope/pkg` -> `pkg`:
/// the last name of the path that is not `index`. A path with no such name (`../../index.js`)
/// is its own key: it matches no file name, only the import itself.
pub(crate) fn module_stem(specifier: &str) -> Option<String> {
    let path = specifier.trim_matches(['\'', '"', '`']);
    let mut parts = path.rsplit('/');
    let file = parts.next()?;
    let stem = file.split('.').next().unwrap_or("");
    let name = if stem == "index" {
        parts
            .find(|p| !matches!(*p, "." | ".." | ""))
            .unwrap_or(path)
    } else {
        stem
    };
    (!name.is_empty()).then(|| name.to_string())
}

/// A signature that is a member of an interface or class (not of an inline `{ f(): void }` type).
fn in_member_body(node: Node) -> bool {
    node.parent().is_some_and(|p| match p.kind() {
        "class_body" | "interface_body" => true,
        "object_type" => p
            .parent()
            .is_some_and(|g| g.kind() == "interface_declaration"),
        _ => false,
    })
}

/// `const B = class {...}`: the name the class expression is bound to.
fn declarator_name<'a>(class: Node, src: &'a str) -> Option<&'a str> {
    let parent = class
        .parent()
        .filter(|p| p.kind() == "variable_declarator")?;
    field_text(parent, "name", src)
}

/// The node whose text starts the declaration: the `export ...` wrapper when there is one.
/// Names of the classes and interfaces `node` extends or implements: `extends ns.A<T>` gives
/// `A`; a computed base (`extends mixin(D)`) gives nothing.
fn bases_of(node: Node, src: &str) -> Vec<String> {
    let none = HashSet::new();
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let mut inner = child.walk();
        match child.kind() {
            "class_heritage" => {
                for clause in child.named_children(&mut inner) {
                    let mut c = clause.walk();
                    match clause.kind() {
                        "extends_clause" => out.extend(
                            clause
                                .children_by_field_name("value", &mut c)
                                .filter_map(|v| class_name(v, src)),
                        ),
                        "implements_clause" => out.extend(
                            clause
                                .named_children(&mut c)
                                .filter_map(|t| type_name(t, src, &none)),
                        ),
                        _ => {}
                    }
                }
            }
            "extends_type_clause" => out.extend(
                child
                    .children_by_field_name("type", &mut inner)
                    .filter_map(|t| type_name(t, src, &none)),
            ),
            _ => {}
        }
    }
    out
}

/// `A` or `ns.A` written as a value: the class name `A`.
fn class_name(node: Node, src: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(text(node, src).to_string()),
        "member_expression" => field_text(node, "property", src).map(String::from),
        _ => None,
    }
}

fn decl_start(node: Node) -> Node {
    let mut top = node;
    while let Some(p) = top
        .parent()
        .filter(|p| matches!(p.kind(), "export_statement" | "ambient_declaration"))
    {
        top = p;
    }
    top
}

/// The declaration inside `export` / `declare` wrappers.
fn unwrap_decl(mut node: Node) -> Node {
    while matches!(node.kind(), "export_statement" | "ambient_declaration") {
        let inner = node
            .child_by_field_name("declaration")
            .or_else(|| node.named_child(0));
        match inner {
            Some(i) => node = i,
            None => break,
        }
    }
    node
}

/// Record a named declaration; returns its signature.
fn add_item(
    node: Node,
    kind: Kind,
    src: &str,
    facts: &mut Facts,
    container: Option<&str>,
) -> Option<String> {
    let name = field_text(node, "name", src)?;
    let start = decl_start(node);
    let sig = signature(start, node.child_by_field_name("body"), src);
    // Overloads (`f(a): A; f(b): B; f(c) {}`) are one definition, at the first signature.
    let mut before = start.prev_named_sibling();
    while let Some(c) = before.filter(|p| p.kind() == "comment") {
        before = c.prev_named_sibling(); // a doc comment between overloads
    }
    let overload = before.map(unwrap_decl).is_some_and(|prev| {
        matches!(prev.kind(), "function_signature" | "method_signature")
            && field_text(prev, "name", src) == Some(name)
    });
    if !overload {
        facts.symbols.push(Symbol {
            name: name.to_string(),
            kind,
            line: line_of(start),
            signature: sig.clone(),
            container: container.map(String::from),
            returns: returns_of(node, src, container),
            bases: bases_of(node, src),
        });
    } else {
        // Overloads that return different classes: which one a call gets is unknown, unless the
        // implementation names its type (TypeScript makes it fit every overload; user, 2026-11-14).
        let returns = returns_of(node, src, container);
        let signature = matches!(node.kind(), "function_signature" | "method_signature");
        let group = facts
            .symbols
            .iter_mut()
            .rev()
            .find(|s| s.name == name && s.container.as_deref() == container);
        if let Some(first) = group {
            if !signature && returns.is_some() {
                first.returns = returns;
            } else if signature && first.returns != returns {
                first.returns = None;
            }
        }
    }
    Some(sig)
}

/// What a function returns, when its annotation names one plain class: `this` is the container,
/// `Promise<T>` is `T` (callers get it after `await`); primitives, unions, arrays and type
/// parameters say nothing.
fn returns_of(func: Node, src: &str, container: Option<&str>) -> Option<Ret> {
    let annotation = func.child_by_field_name("return_type")?;
    let mut ty = annotation.named_child(0)?;
    let promised = ty.kind() == "generic_type" && field_text(ty, "name", src) == Some("Promise");
    if promised {
        ty = ty.child_by_field_name("type_arguments")?.named_child(0)?;
    }
    let name = match ty.kind() {
        "this_type" => container?.to_string(),
        _ => type_name(ty, src, &generic_names(func, src))?,
    };
    Some(Ret {
        ty: name,
        future: promised,
        ..Default::default()
    })
}

/// `const f = () => {}` is a function (even when not exported, so calls inside get a caller);
/// other consts are listed only when exported. Returns the caller name for function values.
fn add_variable(node: Node, src: &str, facts: &mut Facts) -> Option<String> {
    let name = node
        .child_by_field_name("name")
        .filter(|n| n.kind() == "identifier")?;
    let decl = node.parent()?;
    let start = decl_start(decl);
    let exported = start.kind() == "export_statement";
    let value = node.child_by_field_name("value");
    let function = value.filter(|v| matches!(v.kind(), "arrow_function" | "function_expression"));
    let sig = signature(
        start,
        function.and_then(|f| f.child_by_field_name("body")),
        src,
    );
    if exported {
        let kind = if function.is_some() {
            Kind::Fn
        } else {
            Kind::Const
        };
        facts.symbols.push(Symbol {
            name: text(name, src).to_string(),
            kind,
            line: line_of(start),
            signature: sig.clone(),
            container: None,
            returns: function.and_then(|f| returns_of(f, src, None)),
            bases: Vec::new(),
        });
    }
    function.map(|_| caller_name(&sig))
}

/// Keep every import specifier as written; whether it is a local file, an alias or a package
/// is decided later, when the graph knows the project's tsconfig aliases.
fn add_import(node: Node, src: &str, facts: &mut Facts) {
    let Some(path) = field_text(node, "source", src) else {
        add_local_exports(node, src, facts);
        return;
    };
    let path = path.trim_matches(['\'', '"', '`']);
    if !path.is_empty() {
        facts.imports.push(path.to_string());
        uses_of(node, path, src, &mut facts.uses);
    }
}

/// `export { z, z as default }` of names this file imported: other files reach the import
/// through it (zod: `import * as z from "./external.js"; export { z }`). Only imports written
/// above the export are seen (a hoisted one below is missed, never misread).
fn add_local_exports(node: Node, src: &str, facts: &mut Facts) {
    let mut cursor = node.walk();
    let clauses: Vec<Node> = node
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "export_clause")
        .collect();
    for clause in clauses {
        let mut c = clause.walk();
        for one in clause.named_children(&mut c) {
            let Some(name) = field_text(one, "name", src) else {
                continue;
            };
            let alias = field_text(one, "alias", src).unwrap_or(name);
            let imported = facts
                .uses
                .iter()
                .find(|u| !u.public && u.alias == name)
                .map(|u| u.path.clone());
            if let Some(path) = imported {
                facts.uses.push(Use {
                    alias: alias.trim_matches(['\'', '"']).to_string(),
                    path,
                    public: true,
                    local: false,
                });
            }
        }
    }
}

/// What an import or a re-export brings in, as `uses`: alias -> `spec` for a whole module
/// (`* as z`, `export *` under the alias `*`), `spec::name` for one name (`{ a as b }`, a default
/// import is `spec::default`). Exports are `public`: other files reach the names through them.
/// Named re-exports follow Graphify's `re_exports` (`extract.py` `_import_js`, MIT, safishamsi/graphify).
fn uses_of(stmt: Node, spec: &str, src: &str, out: &mut Vec<Use>) {
    let public = stmt.kind() == "export_statement";
    let mut push = |alias: &str, path: String| {
        out.push(Use {
            alias: alias.trim_matches(['\'', '"']).to_string(),
            path,
            public,
            local: false,
        });
    };
    let item = |name: &str| format!("{spec}::{}", name.trim_matches(['\'', '"']));
    let mut whole = public; // `export * from` has no clause
    let mut cursor = stmt.walk();
    for child in stmt.named_children(&mut cursor) {
        let mut inner = child.walk();
        let parts: Vec<Node> = child.named_children(&mut inner).collect();
        match child.kind() {
            "import_clause" => {
                for part in parts {
                    match part.kind() {
                        "identifier" => push(text(part, src), item("default")),
                        "namespace_import" => {
                            if let Some(id) = part.named_child(0) {
                                push(text(id, src), spec.to_string());
                            }
                        }
                        "named_imports" => {
                            let mut c = part.walk();
                            for one in part.named_children(&mut c) {
                                if let Some(name) = field_text(one, "name", src) {
                                    let alias = field_text(one, "alias", src).unwrap_or(name);
                                    push(alias, item(name));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            "export_clause" => {
                whole = false;
                for one in parts {
                    if let Some(name) = field_text(one, "name", src) {
                        let alias = field_text(one, "alias", src).unwrap_or(name);
                        push(alias, item(name));
                    }
                }
            }
            "namespace_export" => {
                whole = false;
                if let Some(id) = parts.first() {
                    push(text(*id, src), spec.to_string());
                }
            }
            _ => {}
        }
    }
    if whole {
        push("*", spec.to_string());
    }
}

/// `f()` -> f, `a.f()` -> f; anything fancier (`(x)()`, `import()`) is skipped.
fn add_call(node: Node, src: &str, facts: &mut Facts, ctx: Ctx) {
    let Some(f) = node.child_by_field_name("function") else {
        return;
    };
    let callee = callee_name(f, src);
    if callee == "require" && f.kind() == "identifier" {
        add_require(node, src, facts);
    }
    if !callee.is_empty() {
        facts.calls.push(Call {
            callee: callee.to_string(),
            line: line_of(node),
            caller: ctx.enclosing.map(String::from),
            form: call_form(f, src, ctx, 0),
            owner: ctx.container.map(String::from),
        });
    }
}

/// `f` -> f, `a.f` -> f; anything else is `""`.
fn callee_name<'a>(function: Node, src: &'a str) -> &'a str {
    match function.kind() {
        "identifier" => text(function, src),
        "member_expression" => field_text(function, "property", src).unwrap_or(""),
        _ => "",
    }
}

/// Longest call chain followed; a longer one is read as a plain method call.
const MAX_CHAIN: usize = 32;

/// The call whose result `expr` is: `a()`, `x.f()`, `(await a())`, `a()!`. Anything that is not
/// such a call has no hop.
fn hop_of(expr: Node, src: &str, ctx: Ctx, depth: usize) -> Option<Hop> {
    if depth >= MAX_CHAIN {
        return None;
    }
    match expr.kind() {
        "parenthesized_expression" | "non_null_expression" => {
            hop_of(expr.named_child(0)?, src, ctx, depth)
        }
        "await_expression" => {
            let mut hop = hop_of(expr.named_child(0)?, src, ctx, depth)?;
            hop.awaited = true;
            Some(hop)
        }
        "call_expression" => {
            let function = expr.child_by_field_name("function")?;
            let callee = callee_name(function, src);
            (!callee.is_empty()).then(|| Hop {
                callee: callee.to_string(),
                form: call_form(function, src, ctx, depth + 1),
                ..Default::default()
            })
        }
        _ => None,
    }
}

/// `new A()`, `new ns.A()`, `<A />`, `<ns.A />`: a call to the class or component `A`.
fn add_class_use(node: Node, class: Node, src: &str, facts: &mut Facts, ctx: Ctx) {
    let mut path = Vec::new();
    segments(class, &mut path);
    let (Some(&root), Some(&last)) = (path.first(), path.last()) else {
        return;
    };
    let callee = text(last, src);
    if callee.is_empty() || !matches!(last.kind(), "identifier" | "property_identifier") {
        return;
    }
    let form = if path.len() == 1 {
        call_form(last, src, ctx, 0)
    } else {
        // `ns.A` and `ns.sub.A` are looked up in a module; through a value or `this`, A is only a name.
        let name = text(root, src);
        match ctx
            .modules
            .get(name)
            .filter(|m| !ctx.locals.contains_key(name) && !m.contains("::"))
        {
            Some(stem) if path.len() == 2 => Form::Module(stem.clone()),
            Some(_) => Form::Module(text(path[path.len() - 2], src).to_string()),
            None => Form::Bare,
        }
    };
    facts.calls.push(Call {
        callee: callee.to_string(),
        line: line_of(node),
        caller: ctx.enclosing.map(String::from),
        form,
        owner: ctx.container.map(String::from),
    });
}

/// The names of a dotted path (`a.b.C`, also JSX `<a.b.C>`), root first.
fn segments<'a>(node: Node<'a>, out: &mut Vec<Node<'a>>) {
    match node.kind() {
        "member_expression" => {
            if let Some(object) = node.child_by_field_name("object") {
                segments(object, out);
            }
            out.extend(node.child_by_field_name("property"));
        }
        "nested_identifier" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                segments(child, out);
            }
        }
        _ => out.push(node),
    }
}

/// `<Button />`: a component is a call to the function or class of that name; a lowercase tag is HTML.
fn add_jsx(node: Node, src: &str, facts: &mut Facts, ctx: Ctx) {
    let Some(tag) = node.child_by_field_name("name") else {
        return;
    };
    let component = match tag.kind() {
        "identifier" => text(tag, src).starts_with(|c: char| c.is_ascii_uppercase()),
        _ => matches!(tag.kind(), "member_expression" | "nested_identifier"),
    };
    if component {
        add_class_use(node, tag, src, facts, ctx);
    }
}

/// How a call is written: `f()`, `this.f()`, `Foo.f()`, `ns.f()` or `value.f()`.
fn call_form(function: Node, src: &str, ctx: Ctx, depth: usize) -> Form {
    let modules = ctx.modules;
    if function.kind() == "identifier" {
        let name = text(function, src);
        if ctx.locals.contains_key(name) {
            return Form::Bare; // a parameter or local, not the import of the same name
        }
        return match modules.get(name) {
            // `make()` imported by name is the function itself, not a namespace member.
            Some(m) if !m.contains("::") => Form::Module(m.clone()),
            _ => Form::Bare,
        };
    }
    let Some(object) = function.child_by_field_name("object").map(skip_non_null) else {
        return Form::Unknown;
    };
    match object.kind() {
        "this" => Form::SelfRef,
        "identifier" => {
            let name = text(object, src);
            if let Some(declared) = ctx.locals.get(name) {
                match declared {
                    Some(Known::Class(t)) => Form::Type(t.clone()),
                    // `const s = a.b(); s.f()` is `a.b().f()`; `const a = a.f()` stops at MAX_CHAIN.
                    Some(Known::Value(value)) => hop_of(*value, src, ctx, depth + 1)
                        .map_or(Form::Method, |h| Form::Returned(Box::new(h))),
                    None => Form::Method,
                }
            } else if name.starts_with(|c: char| c.is_ascii_uppercase()) {
                Form::Type(name.to_string()) // a class, even when imported
            } else if let Some(m) = modules.get(name) {
                Form::Module(m.clone())
            } else {
                Form::Method
            }
        }
        "member_expression" => {
            if let Some(t) = ctx.fields.and_then(|f| this_field_type(object, src, f)) {
                Form::Type(t.to_string()) // `this.repo.find()`
            } else if let Some(sub) = namespace_root(object, src, modules) {
                Form::Module(sub.to_string()) // `z.util.f()`: a sub-module of a namespace
            } else {
                Form::Method
            }
        }
        // `a().f()`: a method of what `a()` returns.
        "call_expression" | "await_expression" | "parenthesized_expression" => {
            hop_of(object, src, ctx, depth).map_or(Form::Method, |h| Form::Returned(Box::new(h)))
        }
        // `new Foo().f()`
        "new_expression" => object
            .child_by_field_name("constructor")
            .filter(|c| c.kind() == "identifier")
            .map_or(Form::Method, |c| Form::Type(text(c, src).to_string())),
        _ => Form::Method,
    }
}

/// The declared type of `this.name`, for the object of a call `this.name.f()`.
fn this_field_type<'a>(object: Node, src: &str, fields: &'a Types) -> Option<&'a str> {
    let base = skip_non_null(object.child_by_field_name("object")?);
    if base.kind() != "this" {
        return None;
    }
    let name = field_text(object, "property", src)?;
    fields.get(name).map(String::as_str)
}

/// Syntax that only describes types: parameters in it are not declarations of the function.
fn is_type(kind: &str) -> bool {
    matches!(
        kind,
        "type_annotation"
            | "function_type"
            | "object_type"
            | "type_alias_declaration"
            | "interface_declaration"
    )
}

/// `x!` is `x` for our purposes.
fn skip_non_null(node: Node) -> Node {
    match node.kind() {
        "non_null_expression" => node.named_child(0).map_or(node, skip_non_null),
        _ => node,
    }
}

/// A type written as a plain or generic or qualified name: `T`, `Box<T>`, `ns.T` -> `T`, `Box`;
/// unions, arrays, functions, literals and the type parameters in `generics` give `None`.
fn type_name(node: Node, src: &str, generics: &HashSet<String>) -> Option<String> {
    let node = if node.kind() == "type_annotation" {
        node.named_child(0)?
    } else {
        node
    };
    let name = match node.kind() {
        "type_identifier" => text(node, src).to_string(),
        "generic_type" => return type_name(node.child_by_field_name("name")?, src, generics),
        "nested_type_identifier" => field_text(node, "name", src)?.to_string(),
        _ => return None,
    };
    (!generics.contains(&name)).then_some(name)
}

/// The type of a declared name: its annotation, else the class it is built with (`new T()`).
fn declared_type(node: Node, src: &str, generics: &HashSet<String>) -> Option<String> {
    let annotated = node.child_by_field_name("type");
    if let Some(t) = annotated.and_then(|t| type_name(t, src, generics)) {
        return Some(t);
    }
    let value = node
        .child_by_field_name("value")
        .filter(|_| annotated.is_none())?;
    if value.kind() != "new_expression" {
        return None;
    }
    let ctor = value.child_by_field_name("constructor")?;
    (ctor.kind() == "identifier").then(|| text(ctor, src).to_string())
}

/// Names of the type parameters (`<T, R>`) of `node` and everything around it.
fn generic_names(node: Node, src: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut up = Some(node);
    while let Some(n) = up {
        if let Some(list) = n.child_by_field_name("type_parameters") {
            let mut cursor = list.walk();
            for param in list.named_children(&mut cursor) {
                names.extend(field_text(param, "name", src).map(String::from));
            }
        }
        up = n.parent();
    }
    names
}

/// Every name a binding pattern introduces (`x`, `{ a, b: c }`, `[d, ...e]`, `{ f = 1 }`).
fn bound_names(pattern: Node, src: &str, out: &mut Vec<String>) {
    match pattern.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => {
            out.push(text(pattern, src).to_string());
        }
        // `{ a: c }` binds `c`, not the key `a`.
        "pair_pattern" => {
            if let Some(value) = pattern.child_by_field_name("value") {
                bound_names(value, src, out);
            }
        }
        // `x = dflt` binds `x`; the default value is an expression.
        "assignment_pattern" | "object_assignment_pattern" => {
            if let Some(left) = pattern.child_by_field_name("left") {
                bound_names(left, src, out);
            }
        }
        _ => {
            let mut cursor = pattern.walk();
            for child in pattern.named_children(&mut cursor) {
                bound_names(child, src, out);
            }
        }
    }
}

/// Types of the parameters and variables a function declares itself (not in nested functions),
/// on top of those of the functions around it; for the whole file (`program`), the top-level ones.
/// A name declared again without a type, with another type or value, or inside a destructuring
/// pattern is unknown: better no type than a wrong one.
fn local_types<'a>(func: Node<'a>, src: &str, outer: &Locals<'a>) -> Locals<'a> {
    let generics = generic_names(func, src);
    let mut own: Locals = HashMap::new();
    let mut declare = |name: String, ty: Option<Known<'a>>| {
        own.entry(name)
            .and_modify(|seen| {
                if *seen != ty {
                    *seen = None;
                }
            })
            .or_insert(ty);
    };
    let mut stack = vec![func];
    while let Some(n) = stack.pop() {
        let pattern = match n.kind() {
            "required_parameter" | "optional_parameter" => n.child_by_field_name("pattern"),
            "variable_declarator" => n.child_by_field_name("name"),
            _ => None,
        };
        let untyped = match n.kind() {
            "arrow_function" => n.child_by_field_name("parameter"), // `x => ...`
            "catch_clause" => n.child_by_field_name("parameter"),
            // `for (const x of xs)` declares x; a bare `for (x of xs)` only assigns it.
            "for_in_statement" if n.child_by_field_name("kind").is_some() => {
                n.child_by_field_name("left")
            }
            _ => None,
        };
        if let Some(untyped) = untyped {
            let mut names = Vec::new();
            bound_names(untyped, src, &mut names);
            names.into_iter().for_each(|name| declare(name, None));
        }
        // `const Foo = require('./foo')` is a module, not a value.
        let required = n
            .child_by_field_name("value")
            .is_some_and(|v| is_require(v, src));
        if let Some(pattern) = pattern {
            if pattern.kind() == "identifier" {
                let name = text(pattern, src);
                // A required module that shadows nothing is left out, so the class rule reads it.
                if !(required && !outer.contains_key(name)) {
                    let held = known(n, src, &generics).filter(|_| !required);
                    declare(name.to_string(), held);
                }
            } else {
                let mut names = Vec::new();
                bound_names(pattern, src, &mut names);
                names.into_iter().for_each(|name| declare(name, None));
            }
        }
        let mut cursor = n.walk();
        let children: Vec<_> = n.named_children(&mut cursor).collect();
        stack.extend(children.into_iter().rev().filter(|c| {
            (n == func && n.kind() != "program")
                || !(FUNCTIONS.contains(&c.kind()) || is_type(c.kind()))
        }));
    }
    let mut all = outer.clone();
    // An outer value read in here would see this function's names instead of its own.
    for known in all.values_mut() {
        if let Some(Known::Value(value)) = known
            && mentions(*value, src, &own)
        {
            *known = None;
        }
    }
    all.extend(own);
    all
}

/// Whether `node` uses any of `names` as a plain identifier.
fn mentions(node: Node, src: &str, names: &Locals) -> bool {
    if names.is_empty() {
        return false;
    }
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "identifier" && names.contains_key(text(n, src)) {
            return true;
        }
        let mut cursor = n.walk();
        stack.extend(n.named_children(&mut cursor));
    }
    false
}

/// `require(...)`, the CommonJS import.
fn is_require(node: Node, src: &str) -> bool {
    node.kind() == "call_expression"
        && node
            .child_by_field_name("function")
            .is_some_and(|f| f.kind() == "identifier" && text(f, src) == "require")
}

/// What a declared name holds: its type (see `declared_type`), else the value of an unannotated
/// `const` (a `let` may be reassigned).
fn known<'a>(node: Node<'a>, src: &str, generics: &HashSet<String>) -> Option<Known<'a>> {
    if let Some(t) = declared_type(node, src, generics) {
        return Some(Known::Class(t));
    }
    let constant = node
        .parent()
        .and_then(|d| d.child_by_field_name("kind"))
        .is_some_and(|k| k.kind() == "const");
    node.child_by_field_name("value")
        .filter(|_| constant && node.child_by_field_name("type").is_none())
        .map(Known::Value)
}

/// Types of a class's fields (`repo: Repo`, `log = new Logger()`) and constructor parameter
/// properties (`constructor(private db: Db)`), by name.
fn field_types(class: Node, src: &str) -> Types {
    let mut fields = Types::new();
    let Some(body) = class.child_by_field_name("body") else {
        return fields;
    };
    let generics = generic_names(class, src);
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        match member.kind() {
            "public_field_definition" => {
                if let (Some(name), Some(t)) = (
                    field_text(member, "name", src),
                    declared_type(member, src, &generics),
                ) {
                    fields.entry(name.to_string()).or_insert(t);
                }
            }
            "method_definition" if field_text(member, "name", src) == Some("constructor") => {
                add_parameter_properties(member, src, &generics, &mut fields);
            }
            _ => {}
        }
    }
    fields
}

fn add_parameter_properties(
    constructor: Node,
    src: &str,
    generics: &HashSet<String>,
    fields: &mut Types,
) {
    let Some(params) = constructor.child_by_field_name("parameters") else {
        return;
    };
    let mut cursor = params.walk();
    for p in params.named_children(&mut cursor) {
        let property = p.children(&mut p.walk()).any(|c| {
            matches!(
                c.kind(),
                "accessibility_modifier" | "override_modifier" | "readonly"
            )
        });
        let name = p
            .child_by_field_name("pattern")
            .filter(|n| n.kind() == "identifier");
        if let (true, Some(name), Some(t)) = (property, name, declared_type(p, src, generics)) {
            fields.entry(text(name, src).to_string()).or_insert(t);
        }
    }
}

/// For `ns.sub` (or `ns.a.sub`) where `ns` is an imported namespace: the last name, `sub`.
fn namespace_root<'a>(
    mut object: Node<'a>,
    src: &'a str,
    modules: &HashMap<String, String>,
) -> Option<&'a str> {
    let sub = field_text(object, "property", src)?;
    while object.kind() == "member_expression" {
        object = object.child_by_field_name("object")?;
    }
    // A name imported from a package (`spec::name`) may be a plain value: only `z.f()` reads it.
    let namespace = |m: &String| !m.contains("::");
    (object.kind() == "identifier" && modules.get(text(object, src)).is_some_and(namespace))
        .then_some(sub)
}

/// CommonJS `require('./x')`: only a plain string argument counts.
fn add_require(node: Node, src: &str, facts: &mut Facts) {
    let Some(arg) = node
        .child_by_field_name("arguments")
        .and_then(|a| a.named_child(0))
        .filter(|a| a.kind() == "string")
    else {
        return;
    };
    let path = text(arg, src).trim_matches(['\'', '"']);
    if !path.is_empty() {
        facts.imports.push(path.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Lang, extract as extract_file};
    use crate::model::{Form, Hop, Kind};

    const SRC: &str = "import { formatPrice } from './price';
import React from 'react';
import { x } from '@/lib/x';
export * from '../shared';
export function show(n: number): string {
  return formatPrice(n);
}
export const Cart = (items: Item[]) => {
  return items.map(show);
};
export const TAX = 0.2;
class Store {
  add(x: number) {
    this.items.push(x);
  }
}
export interface Item { id: number }
export type Id = string;
export enum Mode { A }
function local() { show(1); }
";

    #[test]
    fn symbols() {
        let facts = extract_file(Lang::TypeScript, SRC).unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.line))
            .collect();
        let want = vec![
            ("show", Kind::Fn, 5),
            ("Cart", Kind::Fn, 8),
            ("TAX", Kind::Const, 11),
            ("Store", Kind::Class, 12),
            ("add", Kind::Fn, 13),
            ("Item", Kind::Interface, 17),
            ("Id", Kind::Type, 18),
            ("Mode", Kind::Enum, 19),
            ("local", Kind::Fn, 20),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn signatures_keep_export() {
        let facts = extract_file(Lang::TypeScript, SRC).unwrap();
        assert_eq!(
            facts.symbols[0].signature,
            "export function show(n: number): string"
        );
        assert_eq!(
            facts.symbols[1].signature,
            "export const Cart = (items: Item[]) =>"
        );
    }

    #[test]
    fn imports_keep_every_specifier() {
        let facts = extract_file(Lang::TypeScript, SRC).unwrap();
        assert_eq!(
            facts.imports,
            vec!["../shared", "./price", "@/lib/x", "react"]
        );
    }

    #[test]
    fn calls_know_their_enclosing_fn() {
        let facts = extract_file(Lang::TypeScript, SRC).unwrap();
        let got: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.line, c.caller.as_deref()))
            .collect();
        let want = vec![
            ("formatPrice", 6, Some("export function show")),
            ("map", 9, Some("export const Cart")),
            ("push", 14, Some("add")),
            ("show", 20, Some("function local")),
        ];
        assert_eq!(got, want);
    }

    fn members(src: &str) -> Vec<(String, Option<String>, u32)> {
        let facts = extract_file(Lang::TypeScript, src).unwrap();
        facts
            .symbols
            .iter()
            .map(|s| (s.name.clone(), s.container.clone(), s.line))
            .collect()
    }

    #[test]
    fn a_namespace_import_of_a_bare_index_is_still_a_module() {
        let got = call_forms("import * as z from '../../index.js';\nz.string();\n");
        assert_eq!(
            got,
            vec![("string".to_string(), Form::Module("../../index.js".into()))]
        );
    }

    #[test]
    fn a_namespace_imported_by_name_from_a_package_is_a_module_only_as_a_receiver() {
        let src = "import { z, Foo, make } from 'zod/v4';\nz.string();\nFoo.of();\nmake();\n";
        assert_eq!(
            call_forms(src),
            vec![
                ("string".to_string(), Form::Module("zod/v4::z".into())),
                typed("of", "Foo"),
                ("make".into(), Form::Bare),
            ]
        );
    }

    #[test]
    fn a_package_name_import_keeps_its_old_form_outside_a_plain_receiver() {
        let src = "import { config, api, zod as zz } from 'pkg';
config.db.get();
new api.Client();
zz.string();
function f() {
  const config = make();
  config.get();
}
";
        let got: Vec<_> = call_forms(src)
            .into_iter()
            .filter(|(c, _)| c != "make")
            .collect();
        assert_eq!(
            got,
            vec![
                method("get"),
                ("Client".into(), Form::Bare),
                ("string".into(), Form::Module("pkg::zod".into())),
                ("get".into(), hop("make", Form::Bare, false)),
            ]
        );
    }

    #[test]
    fn a_local_export_of_an_import_re_exports_it() {
        let src = "import * as z from './external.js';\nexport { z, z as default, own };\n";
        let facts = extract_file(Lang::TypeScript, src).unwrap();
        let got: Vec<_> = facts
            .uses
            .iter()
            .map(|u| (u.alias.as_str(), u.path.as_str(), u.public))
            .collect();
        assert_eq!(
            got,
            vec![
                ("z", "./external.js", false),
                ("z", "./external.js", true),
                ("default", "./external.js", true),
            ]
        );
    }

    #[test]
    fn imports_and_re_exports_are_kept_as_uses() {
        let src = "import * as z from 'zod/v4';
import d, { a, b as c } from './x.js';
import type { T } from './t';
export * from './schemas.js';
export * as core from '../core/index.js';
export { e, f as g } from './y';
export { local };
";
        let facts = extract_file(Lang::TypeScript, src).unwrap();
        let got: Vec<_> = facts
            .uses
            .iter()
            .map(|u| (u.alias.as_str(), u.path.as_str(), u.public))
            .collect();
        assert_eq!(
            got,
            vec![
                ("z", "zod/v4", false),
                ("d", "./x.js::default", false),
                ("a", "./x.js::a", false),
                ("c", "./x.js::b", false),
                ("T", "./t::T", false),
                ("*", "./schemas.js", true),
                ("core", "../core/index.js", true),
                ("e", "./y::e", true),
                ("g", "./y::f", true),
            ]
        );
    }

    #[test]
    fn classes_and_interfaces_keep_what_they_extend_and_implement() {
        let src = "class B extends ns.A<T> implements I, J<K> {}
interface X extends Y, ns.Z<T> {}
class C extends mixin(D) {}
class E {}
";
        let facts = extract_file(Lang::TypeScript, src).unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.bases.join(",")))
            .collect();
        assert_eq!(
            got,
            vec![
                ("B", "A,I,J".into()),
                ("X", "Y,Z".into()),
                ("C", String::new()),
                ("E", String::new())
            ]
        );
    }

    fn sym(name: &str, container: Option<&str>, line: u32) -> (String, Option<String>, u32) {
        (name.into(), container.map(String::from), line)
    }

    #[test]
    fn methods_know_their_class_and_functions_have_none() {
        let got = members("class A {\n  run() {}\n}\nfunction free() {}\n");
        assert_eq!(
            got,
            vec![
                sym("A", None, 1),
                sym("run", Some("A"), 2),
                sym("free", None, 4)
            ]
        );
    }

    #[test]
    fn interface_method_signatures_are_symbols_of_the_interface() {
        let got = members("interface T {\n  parse(x: string): number;\n  size: number;\n}\n");
        assert_eq!(got, vec![sym("T", None, 1), sym("parse", Some("T"), 2)]);
    }

    #[test]
    fn abstract_methods_are_symbols_of_their_class() {
        let got = members("abstract class B {\n  abstract go(): void;\n}\n");
        assert_eq!(got, vec![sym("B", None, 1), sym("go", Some("B"), 2)]);
    }

    #[test]
    fn return_types_name_a_plain_class_this_or_a_promised_class() {
        let src = "class A {
  me(): this { return this; }
  wrap(): ZodOptional<this> {}
  async load(): Promise<Repo> {}
  n(): string {}
  g<T>(): T {}
  u(): A | B {}
  none() {}
}
interface I { opt(): this; }
function make(): ns.Builder {}
export function over(a: string): One;
export function over(a: number): Two;
export function over(a: any) {}
export function same(a: string): One;
export function same(a: number): One;
export function same(a: any): One { return a; }
export function pick(a: string): One;
export function pick(a: number): Two;
export function pick(a: any): One { return a; }
export const mk = (): Thing => new Thing();
";
        let facts = extract_file(Lang::TypeScript, src).unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .filter(|s| s.kind == Kind::Fn)
            .map(|s| {
                let r = s.returns.as_ref();
                (s.name.as_str(), r.map(|r| (r.ty.as_str(), r.future)))
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("me", Some(("A", false))),
                ("wrap", Some(("ZodOptional", false))),
                ("load", Some(("Repo", true))),
                ("n", None),
                ("g", None),
                ("u", None),
                ("none", None),
                ("opt", Some(("I", false))),
                ("make", Some(("Builder", false))),
                ("over", None),
                ("same", Some(("One", false))),
                ("pick", Some(("One", false))),
                ("mk", Some(("Thing", false))),
            ]
        );
    }

    #[test]
    fn overloads_are_one_symbol_at_the_first_signature() {
        let src = "export function f(a: string): A;\nexport function f(a: number): B;\nexport function f(a: any) { return a; }\n";
        assert_eq!(members(src), vec![sym("f", None, 1)]);
        let in_class =
            "class C {\n  m(a: string): void;\n  m(a: number): void;\n  m(a: any) {}\n}\n";
        assert_eq!(
            members(in_class),
            vec![sym("C", None, 1), sym("m", Some("C"), 2)]
        );
    }

    #[test]
    fn doc_comments_between_overloads_do_not_split_them() {
        let top = "/** a */\nexport function f(a: string): A;\n/** b */\nexport function f(a: number): B;\nexport function f(a: any) {}\n";
        assert_eq!(members(top), vec![sym("f", None, 2)]);
        let class = "class C {\n  /** a */\n  m(a: string): void;\n  // b\n  m(a: any) {}\n}\n";
        assert_eq!(
            members(class),
            vec![sym("C", None, 1), sym("m", Some("C"), 3)]
        );
    }

    #[test]
    fn declare_overloads_are_one_symbol_too() {
        let src = "declare function f(a: string): A;\ndeclare function f(a: number): B;\nexport declare function g(a: string): A;\nexport declare function g(a: number): B;\n";
        assert_eq!(members(src), vec![sym("f", None, 1), sym("g", None, 3)]);
    }

    #[test]
    fn a_receiver_with_a_known_type_is_a_call_on_that_type() {
        let src = "class Svc {
  private repo: Repo;
  logger = new Logger();
  constructor(private db: Db) {}
  run(x: Item, y?: Other) {
    const a = new Thing();
    const b: Typed = make();
    this.repo.find(); this.logger.log(); this.db.q();
    x.go(); y.stop(); a.hit(); b.beat(); unknown.f();
  }
}
";
        let t = |n: &str| Form::Type(n.into());
        let got: Vec<_> = forms(src).into_iter().map(|(c, f, _)| (c, f)).collect();
        assert_eq!(
            got,
            vec![
                ("Logger".to_string(), Form::Bare),
                ("Thing".into(), Form::Bare),
                ("make".into(), Form::Bare),
                ("find".into(), t("Repo")),
                ("log".into(), t("Logger")),
                ("q".into(), t("Db")),
                ("go".into(), t("Item")),
                ("stop".into(), t("Other")),
                ("hit".into(), t("Thing")),
                ("beat".into(), t("Typed")),
                ("f".into(), Form::Method),
            ]
        );
    }

    #[test]
    fn types_are_scoped_to_their_function_and_class() {
        let src = "class A { r: One; m() { this.r.a(); } }
class B { r: Two; m() { this.r.b(); } }
function f(x: P) { x.p(); }
function g(x: Q) { x.q(); }
function h() { x.z(); }
";
        let got: Vec<_> = forms(src).into_iter().map(|(c, f, _)| (c, f)).collect();
        let t = |n: &str| Form::Type(n.into());
        assert_eq!(
            got,
            vec![
                ("a".to_string(), t("One")),
                ("b".into(), t("Two")),
                ("p".into(), t("P")),
                ("q".into(), t("Q")),
                ("z".into(), Form::Method),
            ]
        );
    }

    #[test]
    fn generic_and_qualified_types_use_their_base_name_and_unions_are_unknown() {
        let src = "function f(a: Box<T>, b: ns.Thing, c: A | B, d: string[]) { a.x(); b.y(); c.z(); d.w(); }\n";
        let got: Vec<_> = forms(src).into_iter().map(|(c, f, _)| (c, f)).collect();
        let t = |n: &str| Form::Type(n.into());
        assert_eq!(
            got,
            vec![
                ("x".to_string(), t("Box")),
                ("y".into(), t("Thing")),
                ("z".into(), Form::Method),
                ("w".into(), Form::Method),
            ]
        );
    }

    fn call_forms(src: &str) -> Vec<(String, Form)> {
        forms(src).into_iter().map(|(c, f, _)| (c, f)).collect()
    }

    fn typed(callee: &str, t: &str) -> (String, Form) {
        (callee.into(), Form::Type(t.into()))
    }

    fn method(callee: &str) -> (String, Form) {
        (callee.into(), Form::Method)
    }

    fn hop(callee: &str, form: Form, awaited: bool) -> Form {
        Form::Returned(Box::new(Hop {
            callee: callee.into(),
            form,
            awaited,
            ..Default::default()
        }))
    }

    #[test]
    fn a_method_on_a_call_result_is_a_chain_and_new_names_its_class() {
        let src = "import * as z from './z';
async function f() {
  z.string().optional();
  (await load()).run();
  a()!.b();
  new Foo().go();
}
";
        let z = || Form::Module("z".into());
        assert_eq!(
            call_forms(src),
            vec![
                ("optional".to_string(), hop("string", z(), false)),
                ("string".into(), z()),
                ("run".into(), hop("load", Form::Bare, true)),
                ("load".into(), Form::Bare),
                ("b".into(), hop("a", Form::Bare, false)),
                ("a".into(), Form::Bare),
                ("go".into(), Form::Type("Foo".into())),
                ("Foo".into(), Form::Bare),
            ]
        );
    }

    #[test]
    fn a_value_made_by_a_call_keeps_the_chain_even_at_top_level() {
        let src = "import * as z from './z';
const Test = z.object();
Test.parse();
class Real {}
Real.make();
function f() {
  const s = Test.extend();
  s.run();
  const t = s.pick();
  t.go();
}
";
        let z = || Form::Module("z".into());
        let object = || hop("object", z(), false);
        let extend = || hop("extend", object(), false);
        let pick = || hop("pick", extend(), false);
        let got: Vec<_> = call_forms(src)
            .into_iter()
            .filter(|(c, _)| ["parse", "make", "run", "go"].contains(&c.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("parse".to_string(), object()),
                typed("make", "Real"),
                ("run".into(), extend()),
                ("go".into(), pick()),
            ]
        );
    }

    #[test]
    fn a_call_made_local_yields_to_its_annotation_a_second_value_and_await() {
        let src = "async function f() {
  const x: Foo = make();
  x.a();
  { const d = one(); }
  { const d = two(); }
  d.b();
  const r = await load();
  r.run();
}
";
        let got: Vec<_> = call_forms(src)
            .into_iter()
            .filter(|(c, _)| ["a", "b", "run"].contains(&c.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                typed("a", "Foo"),
                method("b"),
                ("run".into(), hop("load", Form::Bare, true)),
            ]
        );
    }

    #[test]
    fn a_value_whose_names_are_shadowed_where_it_is_used_is_unknown() {
        let src = "import * as z from './z';
const T = z.object();
function h(z: Other) {
  T.parse();
}
function f(a: Foo) {
  const s = a.b();
  function g(a: Bar) {
    s.run();
  }
  s.go();
}
";
        let got: Vec<_> = call_forms(src)
            .into_iter()
            .filter(|(c, _)| ["parse", "run", "go"].contains(&c.as_str()))
            .collect();
        let b = hop("b", Form::Type("Foo".into()), false);
        assert_eq!(got, vec![method("parse"), method("run"), ("go".into(), b)]);
    }

    #[test]
    fn a_required_module_keeps_the_class_rule() {
        let src = "const Foo = require('./foo');\nfunction f() {\n  Foo.bar();\n}\n";
        let got: Vec<_> = call_forms(src)
            .into_iter()
            .filter(|(c, _)| c == "bar")
            .collect();
        assert_eq!(got, vec![typed("bar", "Foo")]);
        // ... but it still hides an outer name it shadows.
        let src = "function f(db: Db) {\n  function g() {\n    const db = require('./db');\n    db.q();\n  }\n}\n";
        let got: Vec<_> = call_forms(src)
            .into_iter()
            .filter(|(c, _)| c == "q")
            .collect();
        assert_eq!(got, vec![method("q")]);
    }

    #[test]
    fn a_local_made_from_itself_ends() {
        let got = call_forms("function f() {\n  const a = a.f();\n  a.g();\n}\n");
        assert!(got.iter().any(|(c, _)| c == "g"), "{got:?}");
    }

    #[test]
    fn an_untyped_or_destructured_shadow_hides_the_outer_type() {
        let src =
            "function f(x: Foo) { items.map(x => x.a()); items.map(({ x }) => x.b()); x.c(); }\n";
        assert_eq!(
            call_forms(src),
            vec![
                method("map"),
                method("a"),
                method("map"),
                method("b"),
                typed("c", "Foo")
            ]
        );
    }

    #[test]
    fn a_name_declared_twice_with_different_types_is_unknown() {
        let src = "function f() { { const x: A = g(); x.a(); } { const x: B = g(); x.b(); } }\nfunction h(y: P) { const y2 = 1; y.c(); const y = k(); }\n";
        let got = call_forms(src);
        assert!(got.contains(&method("a")) && got.contains(&method("b")));
        assert!(
            got.contains(&method("c")),
            "an untyped redeclaration makes it unknown"
        );
    }

    #[test]
    fn a_type_parameter_is_not_a_class() {
        let src = "function f<T>(x: T, y: U) { x.a(); y.b(); }\nclass C<R> { r: R; m() { this.r.c(); } }\n";
        assert_eq!(
            call_forms(src),
            vec![method("a"), typed("b", "U"), method("c")]
        );
    }

    #[test]
    fn this_fields_follow_this_through_arrows_but_not_nested_functions() {
        let src = "class C {\n  r: R;\n  m() {\n    items.map(() => this.r.a());\n    const f = function () { this.r.b(); };\n    const o = { n() { this.r.c(); } };\n    this.r.d();\n  }\n}\nconst K = class { q: Q; m() { this.q.e(); } };\n";
        let got = call_forms(src);
        let find = |n: &str| got.iter().find(|c| c.0 == n).unwrap().1.clone();
        assert_eq!(find("a"), Form::Type("R".into()));
        assert_eq!(find("b"), Form::Method);
        assert_eq!(find("c"), Form::Method);
        assert_eq!(find("d"), Form::Type("R".into()));
        assert_eq!(find("e"), Form::Type("Q".into()));
    }

    #[test]
    fn readonly_and_override_parameter_properties_and_non_null_receivers() {
        let src = "class C {\n  constructor(readonly db: Db, override log: Log, plain: P) {}\n  m(x: X) { this.db.a(); this.log.b(); this.plain.c(); x!.d(); this.db!.e(); }\n}\n";
        assert_eq!(
            call_forms(src),
            vec![
                typed("a", "Db"),
                typed("b", "Log"),
                method("c"),
                typed("d", "X"),
                typed("e", "Db")
            ]
        );
    }

    #[test]
    fn inner_functions_inherit_outer_types_and_do_not_leak_theirs() {
        let src =
            "function f(x: Foo) { const g = () => x.a(); function h(z: Zed) { z.b(); } z.c(); }\n";
        assert_eq!(
            call_forms(src),
            vec![typed("a", "Foo"), typed("b", "Zed"), method("c")]
        );
    }

    #[test]
    fn a_typed_local_beats_a_module_name_and_a_call_before_its_declaration_is_typed() {
        let src = "import * as util from './util';\nfunction f(util: Thing) { util.a(); X.b(); late.c(); const late: Late = mk(); }\n";
        assert_eq!(
            call_forms(src),
            vec![
                typed("a", "Thing"),
                typed("b", "X"),
                typed("c", "Late"),
                ("mk".into(), Form::Bare)
            ]
        );
    }

    #[test]
    fn a_catch_parameter_hides_an_outer_type() {
        let src = "function f(e: Err) { try { g(); } catch (e) { e.a(); } e.b(); }\n";
        let got = call_forms(src);
        assert!(got.contains(&method("a")) && got.contains(&method("b")));
    }

    #[test]
    fn a_loop_variable_hides_an_outer_type_but_a_bare_assignment_does_not() {
        let src = "function f(x: Foo, y: Bar, z: Baz) { for (const x of xs) x.a(); for (const { y } of ys) y.b(); for (z of zs) z.c(); }\n";
        assert_eq!(
            call_forms(src),
            vec![method("a"), method("b"), typed("c", "Baz")]
        );
    }

    #[test]
    fn parameters_inside_type_annotations_are_not_locals() {
        let src = "function f() { const cb: (x: Foo) => void = g; x.run(); }\n";
        assert!(call_forms(src).contains(&method("run")));
    }

    #[test]
    fn an_untyped_local_shadows_an_imported_namespace() {
        let src =
            "import * as z from './z';\nfunction f(z) { z.go(); }\nfunction g() { z.stay(); }\n";
        assert_eq!(
            call_forms(src),
            vec![method("go"), ("stay".into(), Form::Module("z".into()))]
        );
    }

    #[test]
    fn a_parameter_that_shadows_an_imported_function_is_a_bare_call() {
        let src = "import { run } from './run';\nfunction f(run: Fn) { run(); }\nfunction g() { run(); }\n";
        assert_eq!(
            call_forms(src),
            vec![
                ("run".into(), Form::Bare),
                ("run".into(), Form::Module("run".into()))
            ]
        );
    }

    #[test]
    fn renamed_and_defaulted_destructuring_binds_the_right_names() {
        let src = "function f(c: Foo, d: Dee, e: Eee) {
  const { a: c } = o; c.one();
  const { x = d } = o; d.two(); x.three();
  const [p = e] = o; e.four();
}
";
        assert_eq!(
            call_forms(src),
            vec![
                method("one"),
                typed("two", "Dee"),
                method("three"),
                typed("four", "Eee")
            ]
        );
    }

    #[test]
    fn new_expressions_are_calls_to_the_class() {
        let src = "import * as ns from './ns';\nimport { Mod } from './mod';\nclass A {}\nfunction f() { new A(); new ns.B(1); new Mod(); new Map(); }\n";
        let got = forms(src);
        let names: Vec<_> = got.iter().map(|c| (c.0.as_str(), c.1.clone())).collect();
        assert_eq!(
            names,
            vec![
                ("A", Form::Bare),
                ("B", Form::Module("ns".into())),
                ("Mod", Form::Module("mod".into())),
                ("Map", Form::Bare),
            ]
        );
    }

    #[test]
    fn jsx_components_are_calls_but_html_tags_are_not() {
        let src = "import { Button } from './button';
function App() {
  return <div><Button label=\"x\" /><Card>hi</Card><Foo.Bar /></div>;
}
function Card() { return <p/>; }
";
        let facts = extract_file(Lang::Tsx, src).unwrap();
        let got: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.form.clone(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "Button",
                    Form::Module("button".into()),
                    Some("function App")
                ),
                ("Card", Form::Bare, Some("function App")),
                ("Bar", Form::Bare, Some("function App")),
            ]
        );
    }

    #[test]
    fn new_through_a_value_or_this_is_a_plain_class_name() {
        let src = "import * as Models from './models';
class A {
  m(r: Repo) { new r.Item(); new this.Foo(); new Models.User(); new Models.sub.Deep(); }
}
";
        let got: Vec<_> = forms(src).into_iter().map(|c| (c.0, c.1)).collect();
        assert_eq!(
            got,
            vec![
                ("Item".to_string(), Form::Bare),
                ("Foo".into(), Form::Bare),
                ("User".into(), Form::Module("models".into())),
                ("Deep".into(), Form::Module("sub".into())),
            ]
        );
    }

    #[test]
    fn jsx_member_tags_shadowed_tags_and_plain_javascript() {
        let src = "import * as ui from './ui';
function A({ Comp }) { return <div><ui.Card /><Comp /><Foo.Bar /></div>; }
";
        for lang in [Lang::Tsx, Lang::JavaScript] {
            let facts = extract_file(lang, src).unwrap();
            let got: Vec<_> = facts
                .calls
                .iter()
                .map(|c| (c.callee.as_str(), c.form.clone()))
                .collect();
            assert_eq!(
                got,
                vec![
                    ("Card", Form::Module("ui".into())),
                    ("Comp", Form::Bare),
                    ("Bar", Form::Bare),
                ],
                "{lang:?}"
            );
        }
    }

    #[test]
    fn the_same_name_in_two_places_stays_two_symbols() {
        let got =
            members("function f() {}\nclass C {\n  f() {}\n}\nfunction g() {}\nfunction f() {}\n");
        assert_eq!(got.iter().filter(|s| s.0 == "f").count(), 3);
    }

    #[test]
    fn calls_inside_an_overloaded_function_keep_the_implementation_as_caller() {
        let facts = extract_file(
            Lang::TypeScript,
            "function f(a: string): A;\nfunction f(a: any) { return g(a); }\n",
        )
        .unwrap();
        assert_eq!(facts.calls[0].caller.as_deref(), Some("function f"));
    }

    fn forms(src: &str) -> Vec<(String, Form, Option<String>)> {
        let facts = extract_file(Lang::TypeScript, src).unwrap();
        facts
            .calls
            .iter()
            .map(|c| (c.callee.clone(), c.form.clone(), c.owner.clone()))
            .collect()
    }

    fn form(name: &str, f: Form, owner: Option<&str>) -> (String, Form, Option<String>) {
        (name.into(), f, owner.map(String::from))
    }

    #[test]
    fn calls_record_how_they_are_written() {
        let src = "import * as z from './core/index.js';
import { helper, Klass } from './util';
class A {
  run() { this.go(); helper(); z.make(); B.build(); x.y(); local(); super.go(); Klass.make(); }
}
top();
";
        assert_eq!(
            forms(src),
            vec![
                form("go", Form::SelfRef, Some("A")),
                form("helper", Form::Module("util".into()), Some("A")),
                form("make", Form::Module("core".into()), Some("A")),
                form("build", Form::Type("B".into()), Some("A")),
                form("y", Form::Method, Some("A")),
                form("local", Form::Bare, Some("A")),
                form("go", Form::Method, Some("A")),
                form("make", Form::Type("Klass".into()), Some("A")),
                form("top", Form::Bare, None),
            ]
        );
    }

    #[test]
    fn a_nested_function_declaration_has_no_owner_but_an_arrow_keeps_it() {
        let src = "class A {\n  run() {\n    const f = () => this.a();\n    function g() { this.b(); }\n  }\n}\n";
        assert_eq!(
            forms(src),
            vec![
                form("a", Form::SelfRef, Some("A")),
                form("b", Form::SelfRef, None)
            ]
        );
    }

    #[test]
    fn a_namespace_import_is_a_module_even_from_a_package_or_alias() {
        let src = "import * as z from 'zod/v3';\nimport * as _ from '@scope/lodash';\nz.custom(); _.map();\n";
        assert_eq!(
            forms(src),
            vec![
                form("custom", Form::Module("v3".into()), None),
                form("map", Form::Module("lodash".into()), None)
            ]
        );
    }

    #[test]
    fn a_member_of_an_imported_namespace_is_a_module_of_that_name() {
        let src = "import * as z from './z';\nz.util.f(); this.items.push(1); other.util.g();\n";
        assert_eq!(
            forms(src),
            vec![
                form("f", Form::Module("util".into()), None),
                form("push", Form::Method, None),
                form("g", Form::Method, None)
            ]
        );
    }

    #[test]
    fn packages_and_aliased_imports_are_not_modules_of_ours() {
        let src = "import { map } from 'lodash';\nimport { a as b } from './x';\nmap(); b();\n";
        assert_eq!(
            forms(src),
            vec![form("map", Form::Bare, None), form("b", Form::Bare, None)]
        );
    }

    #[test]
    fn signatures_in_type_literals_are_not_symbols() {
        let src = "type T = { f(): void };\nfunction g(x: { cb(): void }) {}\ninterface I {\n  m(): void;\n}\n";
        let got: Vec<_> = members(src).into_iter().map(|s| s.0).collect();
        assert_eq!(got, vec!["T", "g", "I", "m"]);
    }

    #[test]
    fn getters_and_setters_and_nested_functions_are_not_overloads() {
        let src = "class C {\n  get x() { return 1; }\n  set x(v) {}\n}\nfunction f() {\n  function f() {}\n}\n";
        let got = members(src);
        assert_eq!(got.iter().filter(|s| s.0 == "x").count(), 2);
        assert_eq!(got.iter().filter(|s| s.0 == "f").count(), 2);
    }

    #[test]
    fn object_literal_and_class_expression_methods_have_their_own_container() {
        let src = "class A {\n  run() {\n    const o = { m() { this.a(); } };\n  }\n}\nconst B = class {\n  n() { this.b(); }\n};\n";
        let facts = extract_file(Lang::TypeScript, src).unwrap();
        let owner = |name: &str| {
            facts
                .calls
                .iter()
                .find(|c| c.callee == name)
                .and_then(|c| c.owner.clone())
        };
        assert_eq!(owner("a"), None, "`this` is the object literal");
        assert_eq!(owner("b").as_deref(), Some("B"));
        let n = facts.symbols.iter().find(|s| s.name == "n").unwrap();
        assert_eq!(n.container.as_deref(), Some("B"));
    }

    #[test]
    fn builtin_namespaces_look_like_types() {
        // `Math.max()` is read as `Type(Math)`: no project type has that name, so nothing is ruled out.
        assert_eq!(
            forms("Math.max(1, 2);\n"),
            vec![form("max", Form::Type("Math".into()), None)]
        );
    }

    #[test]
    fn require_with_a_string_is_an_import() {
        let facts = extract_file(
            Lang::JavaScript,
            "const a = require('./a');\nconst b = require(name);\n",
        )
        .unwrap();
        assert_eq!(facts.imports, vec!["./a"]);
    }

    #[test]
    fn tsx_parses_jsx() {
        let facts = extract_file(
            Lang::Tsx,
            "export function App() { return <div>{fmt(1)}</div>; }\n",
        )
        .unwrap();
        assert_eq!(facts.symbols[0].name, "App");
        assert_eq!(facts.calls[0].callee, "fmt");
    }
}
