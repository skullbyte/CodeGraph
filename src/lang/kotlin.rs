//! Kotlin extraction: classes, objects, functions, imports and calls.
//! Ported idea: Graphify's Kotlin `LanguageConfig` in `extract.py` (Apache-2.0 / MIT, see NOTICE).

use tree_sitter::Node;

use super::generic::{Config, no_form, non_empty};
use super::{Facts, text};
use crate::model::Kind;

const CONFIG: Config = Config {
    items: &[
        ("class_declaration", Kind::Class),
        ("object_declaration", Kind::Class),
        ("type_alias", Kind::Type),
        ("function_declaration", Kind::Fn),
        ("enum_entry", Kind::Const),
    ],
    calls: &["call_expression"],
    scopes: &[],
    containers: &[],
    form: no_form,
    name,
    callee,
    imports,
};

pub fn extract(root: Node, src: &str) -> Facts {
    super::generic::extract(root, src, &CONFIG)
}

/// The `name` field; a `typealias` has none, its name is the first identifier.
fn name(node: Node, src: &str) -> Vec<String> {
    let named = node.child_by_field_name("name").or_else(|| {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .find(|c| c.kind() == "identifier")
    });
    named.and_then(|n| non_empty(n, src)).into_iter().collect()
}

/// `f()` -> f; `a.b.f()` -> f; `Foo(1)` -> Foo. Anything else (`(g)()`, `f()()`) is skipped.
fn callee(node: Node, src: &str) -> Option<String> {
    let target = node.named_child(0)?;
    match target.kind() {
        "identifier" => non_empty(target, src),
        "navigation_expression" => {
            let mut cursor = target.walk();
            let last = target.named_children(&mut cursor).last()?;
            (last.kind() == "identifier")
                .then(|| non_empty(last, src))
                .flatten()
        }
        _ => None,
    }
}

/// `import a.b.C` -> `a.b.C` (also for `import a.b.C as D`); `import a.b.*` names no file.
fn imports(node: Node, src: &str, out: &mut Vec<String>) {
    if node.kind() != "import" || text(node, src).trim_end().ends_with('*') {
        return;
    }
    let mut cursor = node.walk();
    let path = node
        .named_children(&mut cursor)
        .find(|c| matches!(c.kind(), "qualified_identifier" | "identifier"));
    if let Some(path) = path.and_then(|p| non_empty(p, src)) {
        out.push(path);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Lang, extract as extract_file};
    use crate::model::Kind;

    const SRC: &str = "package shop

import shop.util.Fmt
import shop.util.*
import shop.model.Item as Thing

typealias Id = Int

interface Named { fun id(): Int }

class Store(val n: Int) {
    fun add(x: Int) {
        println(Fmt.format(x))
        helper(x)
    }
    companion object {
        fun make(): Store = Store(1)
    }
}

object Registry {
    fun run() = Store(2).add(1)
}

fun helper(x: Int) {}
";

    #[test]
    fn symbols() {
        let facts = extract_file(Lang::Kotlin, SRC).unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.line))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Id", Kind::Type, 7),
                ("Named", Kind::Class, 9),
                ("id", Kind::Fn, 9),
                ("Store", Kind::Class, 11),
                ("add", Kind::Fn, 12),
                ("make", Kind::Fn, 17),
                ("Registry", Kind::Class, 21),
                ("run", Kind::Fn, 22),
                ("helper", Kind::Fn, 25),
            ]
        );
        assert_eq!(facts.symbols[4].signature, "fun add(x: Int)");
    }

    #[test]
    fn calls_know_their_enclosing_function() {
        let facts = extract_file(Lang::Kotlin, SRC).unwrap();
        let got: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("println", Some("add")),
                ("format", Some("add")),
                ("helper", Some("add")),
                ("Store", Some("make")),
                ("add", Some("run")),
                ("Store", Some("run")),
            ]
        );
    }

    #[test]
    fn enum_entries_are_constants() {
        let facts = extract_file(
            Lang::Kotlin,
            "enum class Policy {\n    KEEP_OPEN,\n    CLOSE { override fun x() {} },\n}\n",
        )
        .unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Policy", Kind::Class),
                ("KEEP_OPEN", Kind::Const),
                ("CLOSE", Kind::Const),
                ("x", Kind::Fn),
            ]
        );
    }

    #[test]
    fn calls_in_an_enum_entry_body_belong_to_its_function_not_to_the_entry() {
        let facts = extract_file(
            Lang::Kotlin,
            "enum class P {\n    A(make()),\n    B { override fun x() { inner() } },\n}\n",
        )
        .unwrap();
        let calls: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(calls, vec![("make", None), ("inner", Some("x"))]);
    }

    #[test]
    fn imports_skip_wildcards_and_keep_the_real_name_of_an_alias() {
        let facts = extract_file(Lang::Kotlin, SRC).unwrap();
        assert_eq!(facts.imports, vec!["shop.model.Item", "shop.util.Fmt"]);
    }

    #[test]
    fn annotation_lines_are_not_the_signature() {
        let facts = extract_file(
            Lang::Kotlin,
            "/** doc */\n@OkHttpInternalApi\n@Suppress(\"x\")\nclass RealCall(\n  val n: Int,\n) : Call {\n}\n@Foo class Same(val n: Int) {\n}\n@Deprecated(\n  \"old\"\n)\nfun f() = 1\n",
        )
        .unwrap();
        let sigs: Vec<_> = facts.symbols.iter().map(|s| s.signature.as_str()).collect();
        assert_eq!(
            sigs,
            vec![
                "class RealCall(",
                "@Foo class Same(val n: Int)",
                "fun f() = 1"
            ]
        );
    }

    #[test]
    fn extensions_are_picked_up() {
        assert_eq!(Lang::from_path("a/B.kt"), Some(Lang::Kotlin));
        assert_eq!(Lang::from_path("build.gradle.kts"), Some(Lang::Kotlin));
    }
}
