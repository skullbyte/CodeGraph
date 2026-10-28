//! C# extraction: types, methods, calls. `using` names a namespace, not a file, so no imports.

use tree_sitter::Node;

use super::generic::{Config, name_field, non_empty};
use super::{Facts, text};
use crate::model::Kind;

const CONFIG: Config = Config {
    items: &[
        ("class_declaration", Kind::Class),
        ("record_declaration", Kind::Class),
        ("record_struct_declaration", Kind::Struct),
        ("struct_declaration", Kind::Struct),
        ("interface_declaration", Kind::Interface),
        ("enum_declaration", Kind::Enum),
        ("method_declaration", Kind::Fn),
        ("constructor_declaration", Kind::Fn),
        ("destructor_declaration", Kind::Fn),
        ("operator_declaration", Kind::Fn),
        ("conversion_operator_declaration", Kind::Fn),
        ("local_function_statement", Kind::Fn),
    ],
    calls: &["invocation_expression", "object_creation_expression"],
    // A property or indexer is not listed, but the calls in its accessors belong to it.
    scopes: &["property_declaration", "indexer_declaration"],
    name,
    callee,
    imports: |_, _, _| {},
};

pub fn extract(root: Node, src: &str) -> Facts {
    super::generic::extract(root, src, &CONFIG)
}

/// `~X` for a destructor, `operator==` / `operator int` for operators, `this` for an indexer.
fn name(node: Node, src: &str) -> Vec<String> {
    let field = |f: &str| {
        node.child_by_field_name(f)
            .map(|n| text(n, src).to_string())
    };
    match node.kind() {
        "destructor_declaration" => name_field(node, src)
            .into_iter()
            .map(|n| format!("~{n}"))
            .collect(),
        "operator_declaration" => field("operator")
            .map(|op| format!("operator{op}"))
            .into_iter()
            .collect(),
        "conversion_operator_declaration" => field("type")
            .map(|ty| format!("operator {ty}"))
            .into_iter()
            .collect(),
        "indexer_declaration" => vec!["this".to_string()],
        _ => name_field(node, src),
    }
}

/// `F()`, `a.F()`, `a?.F()` and `a.F<T>()` -> F; `new Foo<T>()` and `new a.Foo()` -> Foo.
fn callee(node: Node, src: &str) -> Option<String> {
    if node.kind() == "object_creation_expression" {
        let ty = text(node.child_by_field_name("type")?, src);
        let ty = ty.split('<').next().unwrap_or(ty);
        return Some(
            ty.rsplit(['.', ':'])
                .next()
                .unwrap_or(ty)
                .trim()
                .to_string(),
        )
        .filter(|t| !t.is_empty());
    }
    let mut f = node.child_by_field_name("function")?;
    // `a?.F()`: the method is the last part, a `member_binding_expression`.
    if f.kind() == "conditional_access_expression" {
        let last = u32::try_from(f.named_child_count().checked_sub(1)?).ok()?;
        f = f.named_child(last)?;
    }
    let name = match f.kind() {
        "member_access_expression" | "member_binding_expression" => {
            f.child_by_field_name("name")?
        }
        _ => f,
    };
    let name = if name.kind() == "generic_name" {
        name.named_child(0)?
    } else {
        name
    };
    (name.kind() == "identifier")
        .then(|| non_empty(name, src))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::super::{Lang, extract as extract_file};
    use crate::model::Kind;

    const SRC: &str = "using System;
namespace Shop {
  public class Store {
    public Store() { Init(); }
    public void Add(Item x) {
      items.Add(x);
      var c = new Shop.Cart<Item>();
      Fmt<int>(1);
      int Local() { return 1; }
    }
  }
  public interface IItem {}
  public struct Pair {}
  public enum Mode { A }
  public record Rec(int A);
}
";

    #[test]
    fn symbols() {
        let facts = extract_file(Lang::CSharp, SRC).unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.line))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Store", Kind::Class, 3),
                ("Store", Kind::Fn, 4),
                ("Add", Kind::Fn, 5),
                ("Local", Kind::Fn, 9),
                ("IItem", Kind::Interface, 12),
                ("Pair", Kind::Struct, 13),
                ("Mode", Kind::Enum, 14),
                ("Rec", Kind::Class, 15),
            ]
        );
        assert_eq!(facts.symbols[2].signature, "public void Add(Item x)");
        assert!(facts.imports.is_empty());
    }

    #[test]
    fn destructors_operators_and_property_accessors_are_callers() {
        let facts = extract_file(
            Lang::CSharp,
            "class A {
  ~A() { Free(); }
  public static A operator +(A a, A b) { return Make(); }
  public static implicit operator int(A a) { return Num(); }
  public int P { get { return Get(); } set { Set(value); } }
  public int this[int i] { get { return At(i); } }
  public int Q => Calc();
}
",
        )
        .unwrap();
        let symbols: Vec<_> = facts.symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(symbols, vec!["A", "~A", "operator+", "operator int"]);
        let calls: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            calls,
            vec![
                ("Free", Some("~A")),
                ("Make", Some("operator+")),
                ("Num", Some("operator int")),
                ("Get", Some("P")),
                ("Set", Some("P")),
                ("At", Some("this")),
                ("Calc", Some("Q")),
            ]
        );
    }

    #[test]
    fn nested_property_and_lambda_in_accessor() {
        let facts = extract_file(
            Lang::CSharp,
            "class Outer { class Inner { public int P { get { return Get(); } } }
  public int Q { get { return list.Select(x => Conv(x)).Count(); } } }
",
        )
        .unwrap();
        let calls: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            calls,
            vec![
                ("Get", Some("P")),
                ("Count", Some("Q")),
                ("Select", Some("Q")),
                ("Conv", Some("Q")),
            ]
        );
    }

    #[test]
    fn global_qualified_new_is_a_call_to_the_type() {
        let facts = extract_file(
            Lang::CSharp,
            "class A { void M() { new global::Shop.Cart(); new global::Foo(); } }\n",
        )
        .unwrap();
        let got: Vec<_> = facts.calls.iter().map(|c| c.callee.as_str()).collect();
        assert_eq!(got, vec!["Cart", "Foo"]);
    }

    #[test]
    fn conditional_this_and_target_typed_calls() {
        let facts = extract_file(
            Lang::CSharp,
            "class A { void M() { a?.F(); a.b?.G<int>(); this.H(); new Foo(); Foo x = new(); } }\n",
        )
        .unwrap();
        let got: Vec<_> = facts.calls.iter().map(|c| c.callee.as_str()).collect();
        assert_eq!(got, vec!["F", "G", "H", "Foo"]);
    }

    #[test]
    fn calls_know_their_enclosing_method() {
        let facts = extract_file(Lang::CSharp, SRC).unwrap();
        let got: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Init", Some("Store")),
                ("Add", Some("Add")),
                ("Cart", Some("Add")),
                ("Fmt", Some("Add")),
            ]
        );
    }
}
