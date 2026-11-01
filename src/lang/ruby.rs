//! Ruby extraction: classes, modules, methods and calls. `require` is an ordinary call, as in
//! Graphify, so files link through the calls they make.
//! Ported idea: Graphify's Ruby `LanguageConfig` in `extract.py` (Apache-2.0 / MIT, see NOTICE).

use tree_sitter::Node;

use super::Facts;
use super::generic::{Config, no_form, non_empty};
use crate::model::Kind;

const CONFIG: Config = Config {
    items: &[
        ("class", Kind::Class),
        ("module", Kind::Mod),
        ("method", Kind::Fn),
        ("singleton_method", Kind::Fn),
    ],
    calls: &["call"],
    scopes: &[],
    containers: &[],
    form: no_form,
    name,
    callee,
    imports: |_, _, _| {},
};

pub fn extract(root: Node, src: &str) -> Facts {
    let mut facts = super::generic::extract(root, src, &CONFIG);
    for symbol in &mut facts.symbols {
        symbol.signature = without_comment(&symbol.signature);
    }
    facts
}

/// `def f(x) # :nodoc` -> `def f(x)`: a `#` after whitespace and outside quotes starts a comment.
fn without_comment(signature: &str) -> String {
    let mut quote = None;
    let mut previous = ' ';
    for (at, c) in signature.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '#' && previous.is_whitespace() => {
                return signature[..at].trim_end().to_string();
            }
            None => {}
        }
        previous = c;
    }
    signature.to_string()
}

/// The `name` field; `class A::B` is named `B`, the way a call to it would write it.
fn name(node: Node, src: &str) -> Vec<String> {
    let Some(full) = node
        .child_by_field_name("name")
        .and_then(|n| non_empty(n, src))
    else {
        return Vec::new();
    };
    let last = full.rsplit("::").next().unwrap_or(&full);
    vec![last.trim().to_string()]
}

/// `f(x)`, `a.f` and `A::f(x)` -> f. A bare word (`new`, `helper`) is no `call` node and is not seen.
fn callee(node: Node, src: &str) -> Option<String> {
    non_empty(node.child_by_field_name("method")?, src)
}

#[cfg(test)]
mod tests {
    use super::super::{Lang, extract as extract_file};
    use crate::model::Kind;

    const SRC: &str = "require 'json'

module Shop
  class Store < Base
    def add(x)
      puts format_price(x)
      helper(x)
      Fmt.round(x)
      self.save
    end

    def self.build
      new
    end
  end
end

class Shop::Cart; end

def run; Store.new.add(1); end
";

    #[test]
    fn symbols() {
        let facts = extract_file(Lang::Ruby, SRC).unwrap();
        let got: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.line))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Shop", Kind::Mod, 3),
                ("Store", Kind::Class, 4),
                ("add", Kind::Fn, 5),
                ("build", Kind::Fn, 12),
                ("Cart", Kind::Class, 18),
                ("run", Kind::Fn, 20),
            ]
        );
        assert_eq!(facts.symbols[2].signature, "def add(x)");
    }

    #[test]
    fn calls_know_their_enclosing_method() {
        let facts = extract_file(Lang::Ruby, SRC).unwrap();
        let got: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("require", None),
                ("puts", Some("add")),
                ("format_price", Some("add")),
                ("helper", Some("add")),
                ("round", Some("add")),
                ("save", Some("add")),
                ("add", Some("run")),
                ("new", Some("run")),
            ]
        );
    }

    #[test]
    fn operators_setters_singleton_blocks_and_multiline_names() {
        let facts = extract_file(
            Lang::Ruby,
            "class A::\n  B\n  def ==(o); same(o); end\n  def x=(v); store(v); end\n  class << self\n    def y; [1].each { |i| log(i) }; end\n  end\nend\nclass Empty\nend\n",
        )
        .unwrap();
        let symbols: Vec<_> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();
        assert_eq!(
            symbols,
            vec![
                ("B", Kind::Class),
                ("==", Kind::Fn),
                ("x=", Kind::Fn),
                ("y", Kind::Fn),
                ("Empty", Kind::Class),
            ]
        );
        let calls: Vec<_> = facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect();
        assert_eq!(
            calls,
            vec![
                ("same", Some("==")),
                ("store", Some("x=")),
                ("each", Some("y")),
                ("log", Some("y")),
            ]
        );
    }

    #[test]
    fn a_trailing_comment_is_not_part_of_the_signature() {
        let facts = extract_file(
            Lang::Ruby,
            "def find_by(*args) # :nodoc\n  x\nend\ndef f(x = \"#a\", y = '#b') # note\n  x\nend\n",
        )
        .unwrap();
        let sigs: Vec<_> = facts.symbols.iter().map(|s| s.signature.as_str()).collect();
        assert_eq!(
            sigs,
            vec!["def find_by(*args)", "def f(x = \"#a\", y = '#b')"]
        );
    }

    #[test]
    fn ruby_has_no_file_imports() {
        assert!(extract_file(Lang::Ruby, SRC).unwrap().imports.is_empty());
    }

    #[test]
    fn extensions_are_picked_up() {
        assert_eq!(Lang::from_path("lib/store.rb"), Some(Lang::Ruby));
        assert_eq!(Lang::from_path("tasks/db.rake"), Some(Lang::Ruby));
    }
}
