//! Go extraction: functions, methods, types and calls. A Go import names a package (a folder),
//! not a file, so no imports; files still link through the calls they make.

use tree_sitter::Node;

use super::generic::{Config, name_field, non_empty};
use super::{Facts, field_text};
use crate::model::Kind;

const CONFIG: Config = Config {
    items: &[
        ("function_declaration", Kind::Fn),
        ("method_declaration", Kind::Fn),
        ("type_spec", Kind::Type),
        ("type_alias", Kind::Type),
    ],
    calls: &["call_expression", "type_conversion_expression"],
    scopes: &[],
    name: name_field,
    callee,
    imports: |_, _, _| {},
};

pub fn extract(root: Node, src: &str) -> Facts {
    super::generic::extract(root, src, &CONFIG)
}

/// `f()` -> f, `pkg.f()` and `s.f()` -> f; anything fancier (`(x)()`, `fns[0]()`) is skipped.
/// `F[int](x)` is parsed as a type conversion, so it is read as a call to `F`.
fn callee(node: Node, src: &str) -> Option<String> {
    if node.kind() == "type_conversion_expression" {
        let ty = node.child_by_field_name("type")?;
        let name = field_text(ty, "type", src).filter(|_| ty.kind() == "generic_type")?;
        return non_empty_name(name.rsplit('.').next().unwrap_or(name));
    }
    let f = node.child_by_field_name("function")?;
    match f.kind() {
        "identifier" => non_empty(f, src),
        "selector_expression" => field_text(f, "field", src).map(String::from),
        _ => None,
    }
}

fn non_empty_name(name: &str) -> Option<String> {
    Some(name.trim())
        .filter(|n| !n.is_empty())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::super::{Lang, extract as extract_file};
    use crate::model::Kind;

    const SRC: &str = "package shop

import \"fmt\"

type Store struct{ items []int }

type Item interface{ ID() int }

func (s *Store) Add(x int) {
	s.items = append(s.items, x)
	fmt.Println(Fmt(x))
}

func Fmt(x int) string {
	return \"\"
}
";

    #[test]
    fn symbols() {
        let facts = extract_file(Lang::Go, SRC).unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.line))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Store", Kind::Type, 5),
                ("Item", Kind::Type, 7),
                ("Add", Kind::Fn, 9),
                ("Fmt", Kind::Fn, 14),
            ]
        );
        assert_eq!(facts.symbols[2].signature, "func (s *Store) Add(x int)");
    }

    #[test]
    fn generic_calls_aliases_and_function_literals() {
        let facts = extract_file(
            Lang::Go,
            "package p\ntype A = B\nfunc run() {\n\tF[int](1)\n\tG[int, string](2)\n\tfns[0]()\n\tgo func() { pkg.H() }()\n}\n",
        )
        .unwrap();
        let symbols: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();
        assert_eq!(symbols, vec![("A", Kind::Type), ("run", Kind::Fn)]);
        let calls: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            calls,
            vec![("F", Some("run")), ("G", Some("run")), ("H", Some("run"))]
        );
    }

    #[test]
    fn calls_know_their_enclosing_function() {
        let facts = extract_file(Lang::Go, SRC).unwrap();
        let got: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("append", Some("Add")),
                ("Println", Some("Add")),
                ("Fmt", Some("Add")),
            ]
        );
    }
}
