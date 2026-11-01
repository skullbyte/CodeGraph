//! Java extraction: classes, interfaces, enums, records, methods, imports and calls.

use tree_sitter::Node;

use super::generic::{Config, name_field, no_form, non_empty};
use super::{Facts, text};
use crate::model::Kind;

const CONFIG: Config = Config {
    items: &[
        ("class_declaration", Kind::Class),
        ("record_declaration", Kind::Class),
        ("interface_declaration", Kind::Interface),
        ("annotation_type_declaration", Kind::Interface),
        ("enum_declaration", Kind::Enum),
        ("method_declaration", Kind::Fn),
        ("constructor_declaration", Kind::Fn),
    ],
    calls: &[
        "method_invocation",
        "object_creation_expression",
        "explicit_constructor_invocation",
        "method_reference",
    ],
    scopes: &[],
    containers: &[],
    form: no_form,
    name: name_field,
    callee,
    imports,
};

pub fn extract(root: Node, src: &str) -> Facts {
    super::generic::extract(root, src, &CONFIG)
}

/// `f()` and `a.f()` -> f; `new Foo<T>()` and `new a.Foo()` -> Foo; `this(..)` -> the class's own
/// constructor, `super(..)` -> the superclass's; `A::f` -> f, `A::new` -> A.
fn callee(node: Node, src: &str) -> Option<String> {
    match node.kind() {
        "method_invocation" => node
            .child_by_field_name("name")
            .and_then(|n| non_empty(n, src)),
        "explicit_constructor_invocation" => {
            let class = enclosing_class(node)?;
            let target = match text(node.child_by_field_name("constructor")?, src) {
                "this" => class.child_by_field_name("name")?,
                _ => class.child_by_field_name("superclass")?.named_child(0)?,
            };
            type_name(text(target, src))
        }
        "method_reference" => {
            let mut cursor = node.walk();
            let last = node.named_children(&mut cursor).last()?;
            if last.kind() == "identifier" {
                non_empty(last, src)
            } else {
                // `Foo::new` builds a Foo; `int[]::new` builds an array, which is no project type.
                let owner = node.named_child(0).filter(|o| o.kind() != "array_type")?;
                type_name(text(owner, src))
            }
        }
        _ => type_name(text(node.child_by_field_name("type")?, src)),
    }
}

/// `a.b.Foo<T>` -> Foo.
fn type_name(ty: &str) -> Option<String> {
    let ty = ty.split('<').next().unwrap_or(ty);
    Some(ty.rsplit('.').next().unwrap_or(ty).trim().to_string()).filter(|t| !t.is_empty())
}

fn enclosing_class(node: Node) -> Option<Node> {
    let mut up = node.parent();
    while let Some(n) = up {
        if matches!(
            n.kind(),
            "class_declaration" | "enum_declaration" | "record_declaration"
        ) {
            return Some(n);
        }
        up = n.parent();
    }
    None
}

/// `import a.b.C;` -> `a.b.C`; a nested class or static member also gives its owner classes
/// (`a.b.C.D` -> `a.b.C`, since only capitalised segments are classes). `import a.b.*;` names no file.
fn imports(node: Node, src: &str, out: &mut Vec<String>) {
    if node.kind() != "import_declaration" {
        return;
    }
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    if children.iter().any(|c| c.kind() == "asterisk") {
        return;
    }
    let Some(path) = children
        .iter()
        .find(|c| matches!(c.kind(), "scoped_identifier" | "identifier"))
    else {
        return;
    };
    let path = text(*path, src);
    out.push(path.to_string());
    let mut end = 0;
    let segments: Vec<_> = path.split('.').collect();
    for (i, segment) in segments.iter().enumerate().take(segments.len() - 1) {
        end += segment.len() + usize::from(i > 0);
        if segment.starts_with(char::is_uppercase) {
            out.push(path[..end].to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Lang, extract as extract_file};
    use crate::model::Kind;

    const SRC: &str = "package app;
import com.shop.Cart;
import static com.shop.Util.fmt;
import java.util.*;
import com.shop.Cart.Item;
public class Store {
    Store() { init(); }
    public void add(Item x) {
        items.add(x);
        Cart c = new com.shop.Cart<Item>();
        fmt(x);
    }
    interface Item {}
    enum Mode { A }
    record Pair(int a) {}
}
";

    #[test]
    fn annotation_lines_are_not_the_signature() {
        let facts = extract_file(
            Lang::Java,
            "@Deprecated\n@SuppressWarnings(\"x\")\npublic class A {\n    @Override\n    public void run() {}\n}\n",
        )
        .unwrap();
        let sigs: Vec<_> = facts.symbols.iter().map(|s| s.signature.as_str()).collect();
        assert_eq!(sigs, vec!["public class A", "public void run()"]);
    }

    #[test]
    fn symbols() {
        let facts = extract_file(Lang::Java, SRC).unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.line))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Store", Kind::Class, 6),
                ("Store", Kind::Fn, 7),
                ("add", Kind::Fn, 8),
                ("Item", Kind::Interface, 13),
                ("Mode", Kind::Enum, 14),
                ("Pair", Kind::Class, 15),
            ]
        );
        assert_eq!(facts.symbols[2].signature, "public void add(Item x)");
    }

    #[test]
    fn imports_keep_classes_and_static_owners() {
        let facts = extract_file(Lang::Java, SRC).unwrap();
        assert_eq!(
            facts.imports,
            vec![
                "com.shop.Cart",
                "com.shop.Cart.Item",
                "com.shop.Util",
                "com.shop.Util.fmt",
            ]
        );
    }

    #[test]
    fn this_super_and_method_references_are_calls() {
        let facts = extract_file(
            Lang::Java,
            "class A extends B<T> {
    A() { this(1); }
    A(int x) { super(x); }
    void m() { list.forEach(Util::show); Supplier<Foo> s = Foo::new; }
}
",
        )
        .unwrap();
        let got: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("A", Some("A")),
                ("B", Some("A")),
                ("forEach", Some("m")),
                ("show", Some("m")),
                ("Foo", Some("m")),
            ]
        );
    }

    #[test]
    fn constructor_calls_in_odd_places() {
        let facts = extract_file(
            Lang::Java,
            "class Plain { Plain() { super(); } }
class Outer extends Base {
    class Inner extends Other { Inner() { super(); } }
    Outer() { outer.super(1); }
}
enum E { A; E() { this(1); } E(int x) {} }
record R(int a) { R { this(a, 0); } R(int a, int b) { this(a); } }
class Arr { void m() { Function<Integer, int[]> f = int[]::new; } }
",
        )
        .unwrap();
        let got: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Other", Some("Inner")),
                ("Base", Some("Outer")),
                ("E", Some("E")),
                ("R", Some("R")),
            ]
        );
    }

    #[test]
    fn calls_know_their_enclosing_method() {
        let facts = extract_file(Lang::Java, SRC).unwrap();
        let got: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("init", Some("Store")),
                ("add", Some("add")),
                ("Cart", Some("add")),
                ("fmt", Some("add")),
            ]
        );
    }
}
