//! `--teach` for `def` and `explain`: a plain-language line under each definition, built from a
//! fixed table of language words and the definition's own doc comment. No AI tokens are spent
//! producing it. Ours, not in Graphify.

use std::path::Path;

use crate::query::{DefHit, Explain, Mode};
use crate::semantic;
use crate::tour;

/// At most this many glossary words per definition.
const MAX_TERMS: usize = 4;
/// Only this many hits of one answer are taught (each reads part of a file).
const MAX_TAUGHT: usize = 10;

/// One row per (language group, word). Groups: rust, ts (also JS), py, go, java, kt, php, rb, swift, cs, c (also C++).
const TABLE: [(&str, &str, &str); 83] = [
    ("rust", "pub", "visible outside its module"),
    (
        "rust",
        "pub(..)",
        "visible only inside part of the crate, not to everyone",
    ),
    ("rust", "fn", "a function"),
    ("rust", "async", "can pause while it waits"),
    (
        "rust",
        "impl",
        "methods for a type, or (in a parameter) any type with that trait",
    ),
    ("rust", "trait", "a set of methods a type promises to have"),
    ("rust", "struct", "a record with named fields"),
    ("rust", "enum", "a value that is one of several variants"),
    ("rust", "mod", "a module, a namespace for items"),
    (
        "rust",
        "const",
        "a fixed value (const fn: a function the compiler can run early)",
    ),
    ("rust", "unsafe", "code the compiler cannot check"),
    ("rust", "dyn", "any type with a trait, chosen while running"),
    ("rust", "self", "the value the method is called on"),
    ("rust", "Self", "the type being defined"),
    ("rust", "&mut", "a mutable borrow: may change the value"),
    ("rust", "&", "a borrow: reads a value without owning it"),
    ("rust", "Result", "a value or an error"),
    ("rust", "Option", "a value or nothing"),
    ("rust", "Vec", "a growable list"),
    ("rust", "Box", "a value stored on the heap"),
    ("rust", "String", "text that can grow"),
    ("ts", "export", "visible to other files"),
    ("ts", "function", "a function"),
    ("ts", "class", "a blueprint for objects"),
    ("ts", "interface", "the shape an object must have"),
    ("ts", "type", "a name for a type"),
    ("ts", "async", "can pause while it waits"),
    ("ts", "const", "a name that is not reassigned"),
    ("ts", "Promise", "a value that arrives later"),
    ("ts", "enum", "a fixed set of named values"),
    ("py", "def", "a function"),
    ("py", "class", "a blueprint for objects"),
    ("py", "self", "the object the method is called on"),
    ("py", "async", "can pause while it waits"),
    ("py", "None", "no value"),
    ("go", "func", "a function"),
    ("go", "type", "a name for a type"),
    ("go", "struct", "a record with named fields"),
    ("go", "interface", "a set of methods a type can have"),
    ("go", "error", "a value describing a failure"),
    ("go", "package", "a group of files that belong together"),
    ("java", "public", "visible everywhere"),
    ("java", "private", "visible only inside its class"),
    ("java", "static", "belongs to the class, not to an object"),
    ("java", "class", "a blueprint for objects"),
    ("java", "interface", "methods a class promises to have"),
    ("java", "void", "returns nothing"),
    ("java", "final", "cannot be changed or overridden"),
    ("cs", "public", "visible everywhere"),
    ("cs", "private", "visible only inside its class"),
    ("cs", "static", "belongs to the class, not to an object"),
    ("cs", "class", "a blueprint for objects"),
    ("cs", "interface", "methods a class promises to have"),
    ("cs", "void", "returns nothing"),
    ("cs", "async", "can pause while it waits"),
    (
        "c",
        "static",
        "private to this file (C); shared by all objects (C++ class member)",
    ),
    ("c", "const", "a value that cannot be changed"),
    ("c", "void", "returns nothing"),
    ("c", "struct", "a record with named fields"),
    ("c", "typedef", "a new name for a type"),
    ("c", "class", "a blueprint for objects"),
    ("kt", "fun", "a function"),
    ("kt", "val", "a name whose value never changes"),
    ("kt", "var", "a name whose value can change"),
    ("kt", "object", "a class with exactly one instance"),
    ("kt", "override", "replaces a method of the parent"),
    ("php", "function", "a function"),
    ("php", "class", "a blueprint for objects"),
    ("php", "public", "visible everywhere"),
    ("php", "static", "belongs to the class, not to an object"),
    ("php", "namespace", "a name that groups related code"),
    ("rb", "def", "a method"),
    ("rb", "class", "a blueprint for objects"),
    (
        "rb",
        "module",
        "a named group of methods, shared by classes",
    ),
    ("rb", "self", "the object the method is called on"),
    ("rb", "nil", "no value"),
    ("swift", "func", "a function"),
    ("swift", "let", "a name whose value never changes"),
    ("swift", "var", "a name whose value can change"),
    (
        "swift",
        "struct",
        "a record with named fields, copied when passed around",
    ),
    ("swift", "protocol", "methods a type promises to have"),
    ("c", "template", "code that works for many types"),
    ("c", "*", "a pointer: the address of a value"),
];

fn group_of(path: &str) -> Option<&'static str> {
    match path.rsplit_once('.')?.1 {
        "rs" => Some("rust"),
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => Some("ts"),
        "py" => Some("py"),
        "go" => Some("go"),
        "java" => Some("java"),
        "kt" | "kts" => Some("kt"),
        "php" => Some("php"),
        "rb" | "rake" => Some("rb"),
        "swift" => Some("swift"),
        "cs" => Some("cs"),
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" => Some("c"),
        _ => None,
    }
}

/// Is `chars[at..]` the word `word`, not just the start of a longer word?
fn word_at(chars: &[char], at: usize, word: &str) -> bool {
    let end = at + word.len();
    chars
        .get(at..end)
        .is_some_and(|w| w.iter().copied().eq(word.chars()))
        && !chars
            .get(end)
            .is_some_and(|c| c.is_alphanumeric() || *c == '_')
}

/// Words of a signature in order, with `&mut` (also `&'a mut`), `&`, `*` and `pub(..)` kept as
/// words of their own.
fn tokens(signature: &str) -> Vec<String> {
    let chars: Vec<char> = signature.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '&' {
            let mut j = i + 1;
            while chars.get(j) == Some(&' ') {
                j += 1;
            }
            if chars.get(j) == Some(&'\'') {
                j += 1;
                while chars
                    .get(j)
                    .is_some_and(|c| c.is_alphanumeric() || *c == '_')
                {
                    j += 1;
                }
                while chars.get(j) == Some(&' ') {
                    j += 1;
                }
            }
            if word_at(&chars, j, "mut") {
                out.push("&mut".to_string());
                i = j + 3;
            } else {
                out.push("&".to_string());
                i += 1;
            }
        } else if c == '*' {
            out.push("*".to_string());
            i += 1;
        } else if c.is_alphanumeric() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let restricted = word == "pub" && chars.get(i) == Some(&'(');
            out.push(if restricted {
                "pub(..)".to_string()
            } else {
                word
            });
        } else {
            i += 1;
        }
    }
    out
}

/// Words that are also common parameter names: only taken from the part before the first `(`.
const HEAD_ONLY: [&str; 8] = [
    "type",
    "class",
    "interface",
    "enum",
    "struct",
    "trait",
    "mod",
    "package",
];

/// `word = meaning` pairs for the words of `signature`, in the order they appear.
pub fn terms(signature: &str, path: &str) -> Vec<(&'static str, &'static str)> {
    let Some(group) = group_of(path) else {
        return Vec::new();
    };
    let mut found: Vec<(&'static str, &'static str)> = Vec::new();
    let head_end = signature.find('(').unwrap_or(signature.len());
    for token in tokens(signature) {
        let in_head = signature[..head_end]
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|w| w == token);
        if HEAD_ONLY.contains(&token.as_str()) && !in_head {
            continue;
        }
        let row = TABLE.iter().find(|(g, w, _)| *g == group && *w == token);
        if let Some(&(_, word, meaning)) =
            row.filter(|(_, w, _)| !found.iter().any(|(f, _)| f == w))
        {
            found.push((word, meaning));
        }
    }
    found.truncate(MAX_TERMS);
    found
}

/// Lines to print under a definition: `  terms: ...` and `  doc: ...` (each only when there is something).
pub fn teach(root: &Path, hit: &DefHit) -> Vec<String> {
    let mut out = Vec::new();
    let found = terms(&hit.signature, &hit.path);
    if !found.is_empty() {
        let list: Vec<String> = found.iter().map(|(w, m)| format!("{w} = {m}")).collect();
        out.push(format!("  terms: {}", list.join("; ")));
    }
    if let Some(doc) = doc_line(root, hit) {
        out.push(format!("  doc: {doc}"));
    }
    out
}

/// `def` answer lines; with `teach`, each hit is followed by its `teach` lines.
pub fn def_lines(root: &Path, hits: &[DefHit], teach_on: bool) -> Vec<String> {
    let mut out = Vec::new();
    for (n, hit) in hits.iter().enumerate() {
        out.push(hit.text());
        if teach_on && n < MAX_TAUGHT {
            out.extend(teach(root, hit));
        }
    }
    out
}

/// `explain` answer lines in `mode`; with `teach`, `teach` lines under each definition and a
/// `read next` hint.
pub fn explain_lines(root: &Path, explained: &Explain, teach_on: bool, mode: Mode) -> Vec<String> {
    if !teach_on {
        return explained.lines_in(mode, |_| Vec::new());
    }
    let taught = std::cell::Cell::new(0);
    let mut out = explained.lines_in(mode, |d| {
        taught.set(taught.get() + 1);
        if taught.get() > MAX_TAUGHT {
            Vec::new()
        } else {
            teach(root, d)
        }
    });
    if matches!(mode, Mode::Brief | Mode::Full) {
        out.extend(read_next(explained));
    }
    out
}

/// The most-called callee that has a definition in the project.
pub fn read_next(explained: &Explain) -> Option<String> {
    let link = explained.outgoing.iter().find(|l| l.location.is_some())?;
    Some(format!(
        "read next: {} ({})",
        link.name,
        link.location.as_deref()?
    ))
}

/// How many lines under a Python `def` are searched for the end of its signature and its docstring.
const DOCSTRING_LOOKAHEAD: usize = 12;
/// How many lines of a multi-line attribute or decorator are skipped when walking up.
const ATTRIBUTE_LOOKBACK: usize = 30;

/// First line of the doc comment right above the definition (or the docstring right below, in
/// Python). Only code files have one; plain `//` counts only in Go, where it is the convention.
fn doc_line(root: &Path, hit: &DefHit) -> Option<String> {
    let group = group_of(&hit.path)?;
    let text = tour::read_head(root, &hit.path)?;
    let lines: Vec<&str> = text.lines().collect();
    let at = (hit.line as usize).checked_sub(1)?;
    let found = if group == "py" {
        docstring_below(&lines, at)?
    } else if group == "rb" {
        hash_comment_above(&lines, at)?
    } else {
        comment_above(&lines, at, group == "go")?
    };
    let plain = semantic::clean(&found);
    (!plain.is_empty()).then(|| plain.chars().take(100).collect())
}

/// The docstring after a `def`, skipping the rest of a multi-line signature.
fn docstring_below(lines: &[&str], at: usize) -> Option<String> {
    let window: Vec<&str> = lines
        .iter()
        .skip(at)
        .take(DOCSTRING_LOOKAHEAD)
        .copied()
        .collect();
    let end = window
        .iter()
        .position(|l| l.split(" #").next().unwrap_or(l).trim_end().ends_with(':'))?;
    let body: Vec<&str> = window[end + 1..]
        .iter()
        .copied()
        .skip_while(|l| l.trim().is_empty())
        .collect();
    let first = body.first()?.trim();
    if !(first.starts_with("\"\"\"") || first.starts_with("'''")) {
        return None; // a body comment or code is not documentation
    }
    tour::first_doc_text(&body, true)
}

/// Ruby magic comments: settings for the interpreter or a tool, not a description.
const RUBY_MAGIC: [&str; 6] = [
    "frozen_string_literal:",
    "typed:",
    "rubocop:",
    "encoding:",
    "coding:",
    ":nodoc:",
];

/// How many lines of a Ruby `=begin` block are searched for its start.
const BEGIN_LOOKBACK: usize = 200;

/// The block of `#` lines, or the `=begin`..`=end` block, directly above a Ruby definition.
fn hash_comment_above(lines: &[&str], at: usize) -> Option<String> {
    let before = lines.get(..at)?;
    if before.last().is_some_and(|l| l.trim_end() == "=end") {
        return begin_block(before);
    }
    let mut above: Vec<&str> = before
        .iter()
        .rev()
        .map(|l| l.trim())
        .take_while(|l| l.starts_with('#'))
        .filter(|l| {
            let text = l.trim_start_matches('#').trim().to_lowercase();
            !RUBY_MAGIC.iter().any(|m| text.starts_with(m))
        })
        .collect();
    above.reverse();
    tour::first_doc_text(&above, true)
}

/// The text of the `=begin` block that ends the given lines with `=end`, as if it were `#` lines.
fn begin_block(before: &[&str]) -> Option<String> {
    let end = before.len() - 1;
    let from = end.saturating_sub(BEGIN_LOOKBACK);
    // Another `=end` first means the one we started from has no `=begin` of its own.
    let start = (from..end)
        .rev()
        .take_while(|&i| !before[i].starts_with("=end"))
        .find(|&i| before[i].starts_with("=begin"))?;
    let commented: Vec<String> = before[start + 1..end]
        .iter()
        .map(|l| format!("# {}", l.trim()))
        .collect();
    let as_hashes: Vec<&str> = commented.iter().map(String::as_str).collect();
    tour::first_doc_text(&as_hashes, true)
}

/// The doc comment directly above `at`, stepping over attributes and decorators.
fn comment_above(lines: &[&str], at: usize, plain_slashes: bool) -> Option<String> {
    let before = lines.get(..at)?;
    let mut above: Vec<&str> = Vec::new();
    let mut i = before.len();
    while i > 0 {
        i -= 1;
        let line = before[i].trim();
        if line.starts_with("#[") || line.starts_with('@') {
            continue; // attributes and decorators sit between the comment and the definition
        }
        if looks_like_attribute_tail(line) {
            // Only a multi-line attribute if its first line is found close above; otherwise code.
            i = attribute_start(before, i)?;
            continue;
        }
        let doc = line.starts_with("///") || line.starts_with("/**") || line.starts_with('*');
        if doc || (plain_slashes && line.starts_with("//")) {
            above.push(line);
        } else {
            break;
        }
    }
    above.reverse();
    tour::first_doc_text(&above, false)
}

/// The last line of a multi-line `#[..]` or `@..(..)`.
fn looks_like_attribute_tail(line: &str) -> bool {
    let closes =
        line.ends_with(']') || line.ends_with(")]") || line.ends_with("})") || line.ends_with(')');
    closes && !line.starts_with("//") && !line.starts_with('*')
}

/// Index of the line that starts the multi-line attribute ending at `end`: the nearest line above
/// that begins with `#[` or `@`, which must open a bracket that the span up to `end` closes.
fn attribute_start(lines: &[&str], end: usize) -> Option<usize> {
    let from = end.saturating_sub(ATTRIBUTE_LOOKBACK);
    let start = (from..=end)
        .rev()
        .find(|&j| lines[j].trim().starts_with("#[") || lines[j].trim().starts_with('@'))?;
    let opens = brackets(lines[start]) > 0;
    let closed = lines[start..=end].iter().map(|l| brackets(l)).sum::<i32>() == 0;
    (opens && closed).then_some(start)
}

/// Opening minus closing `(`, `[` and `{` on a line.
fn brackets(line: &str) -> i32 {
    line.chars()
        .map(|c| match c {
            '(' | '[' | '{' => 1,
            ')' | ']' | '}' => -1,
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn words(sig: &str, path: &str) -> Vec<&'static str> {
        terms(sig, path).into_iter().map(|(w, _)| w).collect()
    }

    #[test]
    fn rust_words_come_in_order_and_are_capped() {
        let sig = "pub async fn run(&mut self, x: &str) -> Result<u32, Error>";
        assert_eq!(words(sig, "a.rs"), ["pub", "async", "fn", "&mut"]);
        assert_eq!(
            words("fn parse() -> Option<Vec<u8>>", "a.rs"),
            ["fn", "Option", "Vec"]
        );
        assert_eq!(words("fn read(x: &str)", "a.rs"), ["fn", "&"]);
    }

    #[test]
    fn each_language_uses_its_own_table() {
        assert_eq!(
            words("export async function load(): Promise<Data>", "a.ts"),
            ["export", "async", "function", "Promise"]
        );
        assert_eq!(words("def go(self)", "a.py"), ["def", "self"]);
        assert_eq!(words("func Run() error", "a.go"), ["func", "error"]);
        assert_eq!(
            words("public static void main()", "A.java"),
            ["public", "static", "void"]
        );
        assert_eq!(
            words("override fun run(): Unit", "A.kt"),
            ["override", "fun"]
        );
        assert_eq!(
            words("public static function run()", "A.php"),
            ["public", "static", "function"]
        );
        assert_eq!(words("def go(self)", "a.rb"), ["def", "self"]);
        assert_eq!(words("func run() async", "a.swift"), ["func"]);
        assert_eq!(
            words("static int f(const char *s)", "a.c"),
            ["static", "const", "*"]
        );
        assert!(
            words("def go(x) -> None", "a.rs").is_empty(),
            "a Python word in a Rust file is not taught"
        );
        assert!(words("# Title", "README.md").is_empty());
    }

    #[test]
    fn teach_prints_terms_and_the_doc_comment_above_or_below() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("a.rs"),
            "/// Adds one.\n#[inline]\npub fn add(x: u32) -> u32 { x + 1 }\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("b.py"),
            "def go(self):\n    \"\"\"Runs it.\"\"\"\n    pass\n",
        )
        .unwrap();
        let rust = DefHit {
            path: "a.rs".into(),
            line: 3,
            signature: "pub fn add(x: u32) -> u32".into(),
        };
        let lines = teach(dir.path(), &rust);
        assert_eq!(
            lines[0],
            "  terms: pub = visible outside its module; fn = a function"
        );
        assert_eq!(lines[1], "  doc: Adds one.");
        let py = DefHit {
            path: "b.py".into(),
            line: 1,
            signature: "def go(self)".into(),
        };
        assert_eq!(teach(dir.path(), &py)[1], "  doc: Runs it.");
    }

    #[test]
    fn ruby_doc_comment_is_the_hash_block_right_above() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("a.rb"),
            "# Far away.\n\n# frozen_string_literal: true\n# Adds one.\n# More text.\ndef add(x)\n  x + 1\nend\n\ndef bare\nend\n",
        )
        .unwrap();
        let hit = |line, signature: &str| DefHit {
            path: "a.rb".into(),
            line,
            signature: signature.into(),
        };
        assert_eq!(
            teach(dir.path(), &hit(6, "def add(x)"))[1],
            "  doc: Adds one."
        );
        // A blank line cuts the block: the comment further up is not this method's.
        assert_eq!(
            teach(dir.path(), &hit(10, "def bare")),
            ["  terms: def = a method"]
        );
    }

    #[test]
    fn ruby_begin_end_block_is_a_doc_comment_too() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("a.rb"),
            "=begin\nAdds one.\nMore text.\n=end\ndef add(x)\nend\n=begin\n=end\ndef empty\nend\n=end\ndef stray\nend\n",
        )
        .unwrap();
        let hit = |line, signature: &str| DefHit {
            path: "a.rb".into(),
            line,
            signature: signature.into(),
        };
        assert_eq!(
            teach(dir.path(), &hit(5, "def add(x)"))[1],
            "  doc: Adds one."
        );
        assert_eq!(
            teach(dir.path(), &hit(9, "def empty")),
            ["  terms: def = a method"]
        );
        // An `=end` with no `=begin` above it is not a block.
        assert_eq!(
            teach(dir.path(), &hit(12, "def stray")),
            ["  terms: def = a method"]
        );
    }

    #[test]
    fn nothing_to_teach_prints_nothing_and_a_missing_file_still_gets_terms() {
        let dir = tempfile::tempdir().unwrap();
        let hit = DefHit {
            path: "gone.rs".into(),
            line: 1,
            signature: "fn x()".into(),
        };
        assert_eq!(teach(dir.path(), &hit), ["  terms: fn = a function"]);
        let none = DefHit {
            path: "n.rs".into(),
            line: 1,
            signature: "x".into(),
        };
        assert!(teach(dir.path(), &none).is_empty());
    }

    #[test]
    fn read_next_is_the_most_called_callee_defined_in_the_project() {
        use crate::query::Link;
        let link = |name: &str, location: Option<&str>, sites| Link {
            name: name.into(),
            location: location.map(String::from),
            sites,
        };
        let e = Explain {
            defs: vec![],
            incoming: vec![],
            outgoing: vec![link("external", None, 3), link("helper", Some("a.rs:9"), 2)],
        };
        assert_eq!(read_next(&e).unwrap(), "read next: helper (a.rs:9)");
        let none = Explain {
            outgoing: vec![link("external", None, 1)],
            ..e
        };
        assert!(read_next(&none).is_none());
    }

    #[test]
    fn without_teach_the_lines_are_exactly_the_plain_ones() {
        let dir = tempfile::tempdir().unwrap();
        let hit = DefHit {
            path: "a.rs".into(),
            line: 1,
            signature: "pub fn x()".into(),
        };
        assert_eq!(
            def_lines(dir.path(), std::slice::from_ref(&hit), false),
            [hit.text()]
        );
        let e = Explain {
            defs: vec![hit],
            incoming: vec![],
            outgoing: vec![],
        };
        assert_eq!(explain_lines(dir.path(), &e, false, Mode::Full), e.lines());
        assert_eq!(
            explain_lines(dir.path(), &e, true, Mode::Brief).len(),
            2,
            "def line + terms line"
        );
    }

    #[test]
    fn read_next_belongs_to_the_overview_modes_only() {
        use crate::query::Link;
        let dir = tempfile::tempdir().unwrap();
        let e = Explain {
            defs: vec![],
            incoming: vec![],
            outgoing: vec![Link {
                name: "helper".into(),
                location: Some("a.rs:9".into()),
                sites: 2,
            }],
        };
        let has_hint = |mode| {
            explain_lines(dir.path(), &e, true, mode)
                .iter()
                .any(|l| l.starts_with("read next"))
        };
        assert!(has_hint(Mode::Brief) && has_hint(Mode::Full));
        assert!(!has_hint(Mode::Callers) && !has_hint(Mode::Callees) && !has_hint(Mode::Defs));
    }

    fn hit(path: &str, line: u32, signature: &str) -> DefHit {
        DefHit {
            path: path.into(),
            line,
            signature: signature.into(),
        }
    }

    fn doc_of(file: &str, body: &str, line: u32, sig: &str) -> Option<String> {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(file), body).unwrap();
        teach(dir.path(), &hit(file, line, sig))
            .into_iter()
            .find_map(|l| l.strip_prefix("  doc: ").map(String::from))
    }

    #[test]
    fn restricted_visibility_lifetimes_and_parameter_names_are_not_misread() {
        assert_eq!(words("pub(crate) fn x()", "a.rs"), ["pub(..)", "fn"]);
        assert_eq!(words("fn f(x: &'a mut T)", "a.rs"), ["fn", "&mut"]);
        assert_eq!(words("fn f(x: &mutable)", "a.rs"), ["fn", "&"]);
        assert_eq!(
            words("export function f(type: string)", "a.ts"),
            ["export", "function"]
        );
    }

    #[test]
    fn docs_are_only_read_for_code_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("R.md"), "x\n* bullet one\n[concept]\n").unwrap();
        assert!(teach(dir.path(), &hit("R.md", 3, "[concept] Zed")).is_empty());
    }

    #[test]
    fn only_the_first_hits_are_taught() {
        let dir = tempfile::tempdir().unwrap();
        let hits: Vec<DefHit> = (0..30).map(|n| hit("a.rs", n + 1, "fn x()")).collect();
        let lines = def_lines(dir.path(), &hits, true);
        assert_eq!(
            lines.iter().filter(|l| l.starts_with("  terms:")).count(),
            MAX_TAUGHT
        );
        assert_eq!(
            lines.iter().filter(|l| !l.starts_with("  ")).count(),
            30,
            "every hit is still listed"
        );
    }

    #[test]
    fn python_docstrings_survive_long_signatures_and_ignore_body_comments() {
        let long = "def f(\n    a,\n    b,\n):\n    \"\"\"Doc f.\"\"\"\n";
        assert_eq!(doc_of("a.py", long, 1, "def f(a, b)").unwrap(), "Doc f.");
        assert!(doc_of("a.py", "def f():\n    # TODO fix\n    pass\n", 1, "def f()").is_none());
        let decorated = "@cache\ndef f():\n    \"\"\"Cached.\"\"\"\n";
        assert_eq!(doc_of("a.py", decorated, 2, "def f()").unwrap(), "Cached.");
    }

    #[test]
    fn only_real_doc_comments_count_and_multiline_attributes_are_skipped() {
        assert!(doc_of("a.rs", "// TODO later\nfn f() {}\n", 2, "fn f()").is_none());
        assert!(doc_of("a.rs", "// ----\nfn f() {}\n", 2, "fn f()").is_none());
        let derive = "/// A point.\n#[derive(\n    Debug,\n)]\npub struct P;\n";
        assert_eq!(
            doc_of("a.rs", derive, 5, "pub struct P").unwrap(),
            "A point."
        );
        assert_eq!(
            doc_of(
                "a.go",
                "// Run starts it.\nfunc Run() {}\n",
                2,
                "func Run()"
            )
            .unwrap(),
            "Run starts it."
        );
        assert_eq!(
            doc_of(
                "a.ts",
                "/** Draws. */\nexport function f() {}\n",
                2,
                "export function f()"
            )
            .unwrap(),
            "Draws."
        );
    }

    #[test]
    fn a_definition_past_the_read_cap_still_gets_its_terms() {
        let mut body = "x\n".repeat(40_000);
        body.push_str("/// Late.\nfn late() {}\n");
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), &body).unwrap();
        let lines = teach(dir.path(), &hit("a.rs", 40_002, "fn late()"));
        assert_eq!(lines, ["  terms: fn = a function"]);
    }

    #[test]
    fn an_array_between_a_doc_comment_and_a_function_does_not_lend_its_doc() {
        let src = "/// Table of limits.\nconst LIMITS = [\n  1,\n  2,\n]\nexport function f() {}\n";
        assert!(doc_of("a.ts", src, 6, "export function f()").is_none());
        let real = "/// Real doc.\n#[derive(\n    Debug,\n)]\nstruct S;\n";
        assert_eq!(doc_of("a.rs", real, 5, "struct S").unwrap(), "Real doc.");
    }

    #[test]
    fn a_python_signature_line_may_end_with_a_comment() {
        let src = "def f(a):  # note\n    \"\"\"Doc f.\"\"\"\n    if a:\n        pass\n";
        assert_eq!(doc_of("a.py", src, 1, "def f(a)").unwrap(), "Doc f.");
    }

    #[test]
    fn code_ending_in_a_parenthesis_is_not_mistaken_for_an_annotation() {
        let src = "/** Old item. */\n@Foo\nregister(handler)\nexport function f() {}\n";
        assert!(doc_of("a.ts", src, 4, "export function f()").is_none());
        let real = "/** Draws. */\n@Component({\n  name: 'x',\n})\nexport class C {}\n";
        assert_eq!(doc_of("a.ts", real, 5, "export class C").unwrap(), "Draws.");
    }
}
