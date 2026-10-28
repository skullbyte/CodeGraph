//! Java extraction: classes, interfaces, enums, records, methods, imports and calls.

use tree_sitter::Node;

use super::generic::{Config, name_field, non_empty};
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
    calls: &["method_invocation", "object_creation_expression"],
    scopes: &[],
    name: name_field,
    callee,
    imports,
};

pub fn extract(root: Node, src: &str) -> Facts {
    super::generic::extract(root, src, &CONFIG)
}

/// `f()` and `a.f()` -> f; `new Foo<T>()` and `new a.Foo()` -> Foo.
fn callee(node: Node, src: &str) -> Option<String> {
    if node.kind() == "method_invocation" {
        return node
            .child_by_field_name("name")
            .and_then(|n| non_empty(n, src));
    }
    let ty = text(node.child_by_field_name("type")?, src);
    let ty = ty.split('<').next().unwrap_or(ty);
    Some(ty.rsplit('.').next().unwrap_or(ty).trim().to_string()).filter(|t| !t.is_empty())
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
