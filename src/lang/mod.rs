//! Language support: one file per language, chosen by file extension.

pub mod c;
pub mod csharp;
pub mod generic;
pub mod golang;
pub mod java;
pub mod kotlin;
pub mod markdown;
pub mod php;
pub mod python;
pub mod ruby;
pub mod rust;
pub mod swift;
pub mod typescript;

use anyhow::{Result, anyhow};
use tree_sitter::{Language, Node, Parser};

use crate::model::{Call, Symbol};

/// What one file contributes to the graph, before hashing and line counting.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub symbols: Vec<Symbol>,
    pub imports: Vec<String>,
    pub calls: Vec<Call>,
}

/// Supported languages; adding one means a new variant, a grammar and a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    /// Parsed with the TSX grammar: JavaScript (and JSX) is a subset of it.
    JavaScript,
    Python,
    Java,
    CSharp,
    Go,
    Kotlin,
    Php,
    Ruby,
    Swift,
    C,
    Cpp,
    /// `.h`: tried as C first (K&R functions, C++ keywords as names), as C++ when that fails.
    Header,
    /// Docs: no grammar, a hand-written line scanner (`markdown.rs`).
    Markdown,
}

impl Lang {
    pub fn from_path(path: &str) -> Option<Lang> {
        if path.ends_with(".d.ts") {
            return None;
        }
        match path.rsplit('.').next()? {
            "rs" => Some(Lang::Rust),
            "ts" => Some(Lang::TypeScript),
            "tsx" => Some(Lang::Tsx),
            "py" => Some(Lang::Python),
            "java" => Some(Lang::Java),
            "cs" => Some(Lang::CSharp),
            "go" => Some(Lang::Go),
            "kt" | "kts" => Some(Lang::Kotlin),
            "php" => Some(Lang::Php),
            "rb" | "rake" => Some(Lang::Ruby),
            "swift" => Some(Lang::Swift),
            "c" => Some(Lang::C),
            "h" => Some(Lang::Header),
            "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Some(Lang::Cpp),
            "js" | "jsx" | "mjs" | "cjs" => Some(Lang::JavaScript),
            "md" | "mdx" | "markdown" | "qmd" => Some(Lang::Markdown),
            _ => None,
        }
    }

    /// `None` for languages without a tree-sitter grammar (markdown).
    fn grammar(self) -> Option<Language> {
        Some(match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::Java => tree_sitter_java::LANGUAGE.into(),
            Lang::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
            Lang::Php => tree_sitter_php::LANGUAGE_PHP.into(),
            Lang::Ruby => tree_sitter_ruby::LANGUAGE.into(),
            Lang::Swift => tree_sitter_swift::LANGUAGE.into(),
            Lang::C | Lang::Header => tree_sitter_c::LANGUAGE.into(),
            Lang::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Lang::Tsx | Lang::JavaScript => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::Markdown => return None,
        })
    }
}

/// Parse `src` and pull out its symbols, imports and calls.
pub fn extract(lang: Lang, src: &str) -> Result<Facts> {
    let Some(grammar) = lang.grammar() else {
        return Ok(markdown::extract(src));
    };
    let mut parser = Parser::new();
    parser.set_language(&grammar)?;
    let mut tree = parser
        .parse(src, None)
        .ok_or_else(|| anyhow!("parser gave no tree"))?;
    if lang == Lang::Header && tree.root_node().has_error() {
        parser.set_language(&tree_sitter_cpp::LANGUAGE.into())?;
        if let Some(cpp) = parser
            .parse(src, None)
            .filter(|t| !t.root_node().has_error())
        {
            tree = cpp;
        }
    }
    let mut facts = match lang {
        Lang::Rust => rust::extract(tree.root_node(), src),
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => {
            typescript::extract(tree.root_node(), src)
        }
        Lang::Python => python::extract(tree.root_node(), src),
        Lang::Java => java::extract(tree.root_node(), src),
        Lang::CSharp => csharp::extract(tree.root_node(), src),
        Lang::Go => golang::extract(tree.root_node(), src),
        Lang::Kotlin => kotlin::extract(tree.root_node(), src),
        Lang::Php => php::extract(tree.root_node(), src),
        Lang::Ruby => ruby::extract(tree.root_node(), src),
        Lang::Swift => swift::extract(tree.root_node(), src),
        Lang::C | Lang::Cpp | Lang::Header => c::extract(tree.root_node(), src),
        Lang::Markdown => markdown::extract(src),
    };
    facts.imports.sort();
    facts.imports.dedup();
    Ok(facts)
}

/// Source text of a node ("" if the byte range is somehow invalid).
pub(crate) fn text<'a>(node: Node, src: &'a str) -> &'a str {
    src.get(node.byte_range()).unwrap_or("")
}

/// Text of a named child field, e.g. `name` of a function.
pub(crate) fn field_text<'a>(node: Node, field: &str, src: &'a str) -> Option<&'a str> {
    node.child_by_field_name(field).map(|n| text(n, src))
}

/// One-line declaration text from `start` up to `body` (or the first line when there is no body).
/// Lines that only hold an annotation (`@Name`, `@Name(..)`, also over several lines) come first
/// in some languages (Kotlin, Java) and say nothing about the declaration, so they are skipped.
pub(crate) fn signature(start: Node, body: Option<Node>, src: &str) -> String {
    let end = body.map_or(start.end_byte(), |b| b.start_byte());
    let raw = src.get(start.start_byte()..end).unwrap_or("");
    let raw = without_annotation_lines(raw);
    let raw = if body.is_some() {
        raw
    } else {
        raw.lines().next().unwrap_or("")
    };
    let joined = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = joined.trim_end_matches(['{', ' ']);
    trimmed.chars().take(110).collect()
}

/// `text` without its leading annotation-only lines; unchanged when nothing else would be left.
fn without_annotation_lines(text: &str) -> &str {
    let mut rest = text;
    while let Some(line_end) = annotation_end(rest) {
        let after = &rest[line_end..];
        if after.trim().is_empty() {
            return rest;
        }
        rest = after;
    }
    rest
}

/// Byte length of the leading annotation (with its line break) when `text` starts with a line
/// that is only an annotation, e.g. `@Name\n` or `@Name(\n  "x"\n)\n`; None otherwise.
fn annotation_end(text: &str) -> Option<usize> {
    let trimmed = text.trim_start();
    let lead = text.len() - trimmed.len();
    let body = trimmed.strip_prefix('@')?;
    let name_len = body
        .find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '.' | ':')))
        .unwrap_or(body.len());
    if name_len == 0 {
        return None;
    }
    let mut at = lead + 1 + name_len;
    let after = &text[at..];
    if let Some(args) = after.strip_prefix('(') {
        // Find the matching `)` (parentheses inside "strings" do not count); text after it on the
        // same line means a real declaration follows.
        let mut depth = 1;
        let mut close = None;
        let mut quoted = false;
        let mut escaped = false;
        for (i, c) in args.char_indices() {
            if quoted {
                quoted = escaped || c != '"';
                escaped = c == '\\' && !escaped;
                continue;
            }
            match c {
                '"' => quoted = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        at += 1 + close? + 1;
    }
    let tail = &text[at..];
    let line_len = tail.find('\n').map_or(tail.len(), |i| i + 1);
    let rest = tail[..line_len].trim();
    (rest.is_empty() || rest.starts_with("//")).then_some(at + line_len)
}

/// Name shown after "in" for callers: the signature up to its first `(` or `<`
/// (so generics like `fn f<T: Fn(u8)>` don't leak into the name). The parentheses of a
/// restricted visibility such as `pub(crate)` are part of the name, not its end.
pub(crate) fn caller_name(signature: &str) -> String {
    let skip = signature
        .strip_prefix("pub(")
        .and_then(|rest| rest.find(')'))
        .map_or(0, |close| "pub(".len() + close + 1);
    let end = signature[skip..]
        .find(['(', '<'])
        .map_or(signature.len(), |i| skip + i);
    signature[..end]
        .trim_end_matches([' ', '='])
        .trim()
        .to_string()
}

/// 1-based line number, as editors and `rg -n` show it.
pub(crate) fn line_of(node: Node) -> u32 {
    u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::without_annotation_lines as skip;

    #[test]
    fn leading_annotation_lines_are_skipped_in_every_syntax() {
        assert_eq!(skip("@available(macOS 10.15, *)\nfunc f()"), "func f()");
        assert_eq!(
            skip("@Component({a: (1)})\nexport class X"),
            "export class X"
        );
        assert_eq!(skip("@dataclass\nclass X:"), "class X:");
        assert_eq!(skip("@Ñandú\nclass Ü"), "class Ü");
        assert_eq!(skip("@Foo(\")\")\nclass X"), "class X");
        assert_eq!(skip("@Foo(\"a\\\"(\")\nclass X"), "class X");
        assert_eq!(skip("@Foo // note\nclass X"), "class X");
        assert_eq!(skip("@A\n@B(1)\n@C\nfun f()"), "fun f()");
    }

    #[test]
    fn real_declarations_and_odd_input_are_left_alone() {
        assert_eq!(skip("@Foo class X"), "@Foo class X");
        assert_eq!(skip("@interface Foo {"), "@interface Foo {");
        assert_eq!(skip("@Foo"), "@Foo", "never leave nothing");
        assert_eq!(skip("@Foo(\nclass X"), "@Foo(\nclass X");
        assert_eq!(skip("@\nclass X"), "@\nclass X");
        assert_eq!(skip("fn f()"), "fn f()");
        assert_eq!(skip(""), "");
    }
}
