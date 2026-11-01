//! Markdown: headings, links to other docs and `code` mentions, with no LLM and no grammar.
//! Idea from Graphify's `extractors/markdown.py` (Apache-2.0 + MIT, see NOTICE): one page of
//! headings, `references` links, and inline-code mentions that resolve to a single code symbol.

use std::collections::HashSet;

use super::Facts;
use crate::model::{Call, Form, Kind, Symbol};

/// Docs bigger than this are skipped (a generated dump would only add noise).
const MAX_BYTES: usize = 1_000_000;
/// Link targets with one of these endings are docs; links to code, images and sites are not.
const DOC_EXTS: [&str; 6] = ["md", "mdx", "markdown", "qmd", "rst", "txt"];
/// Front matter longer than this is not front matter.
const MAX_FRONTMATTER: usize = 200;

/// Where the scan is: the heading we are under and what was already recorded.
struct Scan {
    facts: Facts,
    heading: Option<String>,
    seen: HashSet<(Option<String>, String)>,
}

/// Headings, doc links and code mentions of one markdown file.
pub fn extract(src: &str) -> Facts {
    if src.len() > MAX_BYTES {
        return Facts::default();
    }
    let lines: Vec<&str> = src.lines().collect();
    let mut scan = Scan {
        facts: Facts::default(),
        heading: None,
        seen: HashSet::new(),
    };
    let mut fence: Option<(char, usize)> = None;
    for (i, raw) in lines.iter().enumerate().skip(frontmatter_end(&lines)) {
        let line = u32::try_from(i + 1).unwrap_or(u32::MAX);
        let text = raw.trim_start();
        let indent = raw.len() - text.len();
        if let Some((ch, n)) = fence {
            if indent <= 3 && closes_fence(text, ch, n) {
                fence = None;
            }
            continue;
        }
        if indent <= 3 {
            if let Some(opened) = opens_fence(text) {
                fence = Some(opened);
                continue;
            }
            if let Some((level, title)) = atx_heading(text) {
                scan.heading = Some(title.to_string());
                scan.facts.symbols.push(Symbol {
                    name: title.to_string(),
                    kind: Kind::Heading,
                    line,
                    signature: format!("{} {title}", "#".repeat(level))
                        .chars()
                        .take(110)
                        .collect(),
                    container: None,
                });
                scan.inline(title, line);
                continue;
            }
            if let Some(target) = reference_definition(text) {
                scan.link(target);
                continue;
            }
        }
        scan.inline(raw, line);
    }
    scan.facts.imports.sort();
    scan.facts.imports.dedup();
    scan.facts
}

impl Scan {
    /// Code mentions, `[t](x.md)` links and `[[wikilinks]]` of one line of prose.
    fn inline(&mut self, text: &str, line: u32) {
        let mut prose = String::new();
        let mut rest = text;
        while let Some(open) = rest.find('`') {
            prose.push_str(&rest[..open]);
            let run = rest[open..].chars().take_while(|&c| c == '`').count();
            let after = &rest[open + run..];
            match find_run(after, run) {
                Some(close) => {
                    self.mention(&after[..close], line);
                    prose.push(' ');
                    rest = &after[close + run..];
                }
                None => {
                    prose.push_str(&rest[open..open + run]);
                    rest = after;
                }
            }
        }
        prose.push_str(rest);
        for target in inline_links(&prose) {
            self.link(target);
        }
        for inner in wikilinks(&prose) {
            let page = inner.split(['|', '#']).next().unwrap_or("").trim();
            if !page.is_empty() {
                let page = if is_doc(page) {
                    page.to_string()
                } else {
                    format!("{page}.md")
                };
                self.facts.imports.push(page);
            }
        }
    }

    fn link(&mut self, target: &str) {
        if let Some(doc) = doc_target(target) {
            self.facts.imports.push(doc);
        }
    }

    /// `foo`, `foo()`, `Foo::bar`, `obj.method()`: the last name, once per heading.
    fn mention(&mut self, code: &str, line: u32) {
        let code = code.trim();
        let (code, called) = code
            .strip_suffix("()")
            .map_or((code, false), |inner| (inner, true));
        let segments: Vec<&str> = if code.contains("::") {
            code.split("::").collect()
        } else if called {
            code.split('.').collect()
        } else {
            vec![code]
        };
        let Some(name) = segments.last().copied() else {
            return;
        };
        if name.len() < 2 || !segments.iter().all(|s| is_identifier(s)) {
            return;
        }
        if self.seen.insert((self.heading.clone(), name.to_string())) {
            self.facts.calls.push(Call {
                callee: name.to_string(),
                line,
                caller: self.heading.clone(),
                form: Form::Unknown,
                owner: None,
            });
        }
    }
}

fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Index of the first line after a leading `---` ... `---` block; 0 when there is none.
fn frontmatter_end(lines: &[&str]) -> usize {
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return 0;
    }
    lines
        .iter()
        .enumerate()
        .skip(1)
        .take(MAX_FRONTMATTER)
        .find(|(_, l)| matches!(l.trim_end(), "---" | "..."))
        .map_or(0, |(i, _)| i + 1)
}

fn opens_fence(text: &str) -> Option<(char, usize)> {
    let ch = text.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let run = text.chars().take_while(|&c| c == ch).count();
    // A backtick fence's info string cannot hold a backtick; that is inline code instead.
    (run >= 3 && !(ch == '`' && text[run..].contains('`'))).then_some((ch, run))
}

fn closes_fence(text: &str, ch: char, run: usize) -> bool {
    let text = text.trim_end();
    text.len() >= run && text.chars().all(|c| c == ch)
}

/// `(level, title)` of an ATX heading (`## Title ##`); none for 7+ `#` or an empty title.
fn atx_heading(text: &str) -> Option<(usize, &str)> {
    let level = text.chars().take_while(|&c| c == '#').count();
    let after = &text[level..];
    // CommonMark: the `#`s must be followed by a space or tab (`#hashtag`, `#include` are not headings).
    if !(1..=6).contains(&level) || !after.starts_with([' ', '\t']) {
        return None;
    }
    let rest = after.trim();
    let stripped = rest.trim_end_matches('#');
    let title = if stripped.len() != rest.len() && (stripped.is_empty() || stripped.ends_with(' '))
    {
        stripped.trim_end()
    } else {
        rest
    };
    (!title.is_empty()).then_some((level, title))
}

/// `[label]: target` at the start of a line.
fn reference_definition(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('[')?;
    let (label, after) = rest.split_once("]:")?;
    if label.is_empty() || label.starts_with('^') || label.contains(']') {
        return None;
    }
    after.split_whitespace().next()
}

/// Byte offset of the next run of exactly `run` backticks.
fn find_run(text: &str, run: usize) -> Option<usize> {
    let mut at = 0;
    while let Some(found) = text[at..].find('`') {
        let start = at + found;
        let len = text[start..].chars().take_while(|&c| c == '`').count();
        if len == run {
            return Some(start);
        }
        at = start + len;
    }
    None
}

/// Targets of `[text](target)`, images (`![..](..)`) excluded.
fn inline_links(prose: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = prose[at..].find("](") {
        let close = at + found;
        let start = close + 2;
        at = start;
        let is_image = prose[..close]
            .rfind('[')
            .is_some_and(|open| prose[..open].ends_with('!'));
        let target = &prose[start..];
        let end = if target.starts_with('<') {
            target.find('>').map(|i| i + 1)
        } else {
            target.find(')')
        };
        let Some(end) = end else { continue };
        let raw = &target[..end];
        // `(x.md "title")`: the title is not part of the target.
        let raw = if raw.starts_with('<') {
            raw
        } else {
            raw.split_whitespace().next().unwrap_or("")
        };
        if !is_image {
            out.push(raw);
        }
    }
    out
}

/// Inner text of every `[[...]]`.
fn wikilinks(prose: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = prose;
    while let Some(open) = rest.find("[[") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("]]") else { break };
        out.push(&after[..close]);
        rest = &after[close + 2..];
    }
    out
}

fn is_doc(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| DOC_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// A link target worth an import: a relative path to a doc, without `#anchor`, `?query` or `<>`.
fn doc_target(target: &str) -> Option<String> {
    let target = target.trim().trim_start_matches('<').trim_end_matches('>');
    let path = target.split(['#', '?']).next().unwrap_or("");
    let outside = path.is_empty()
        || path.starts_with('/')
        || path.contains("://")
        || path.starts_with("mailto:");
    (!outside && is_doc(path)).then(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Kind;

    fn names(facts: &Facts) -> Vec<(&str, u32)> {
        facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.line))
            .collect()
    }

    fn mentions(facts: &Facts) -> Vec<(&str, Option<&str>)> {
        facts
            .calls
            .iter()
            .map(|c| (c.callee.as_str(), c.caller.as_deref()))
            .collect()
    }

    #[test]
    fn headings_become_heading_symbols_with_their_line() {
        let f = extract("# Title\n\ntext\n\n## Usage (CLI) ##\n\n####### not a heading\n");
        assert_eq!(names(&f), vec![("Title", 1), ("Usage (CLI)", 5)]);
        assert!(f.symbols.iter().all(|s| s.kind == Kind::Heading));
        assert_eq!(f.symbols[1].signature, "## Usage (CLI)");
    }

    #[test]
    fn fenced_code_and_frontmatter_are_skipped() {
        let src = "---\ntitle: x\n# not a heading\n---\n# Real\n```rust\n# nope\n`hidden()`\n```\n~~~\n# nope2\n~~~\nafter `shown()`\n";
        let f = extract(src);
        assert_eq!(names(&f), vec![("Real", 5)]);
        assert_eq!(mentions(&f), vec![("shown", Some("Real"))]);
    }

    #[test]
    fn links_to_docs_become_imports_and_the_rest_is_ignored() {
        let src = "[a](a.md) [b](../b.md#sec \"t\") [c](<c d.md>) ![img](p.png)\n\
                   [web](https://x.io/y.md) [mail](mailto:a@b.c) [top](#top) [src](main.rs)\n\
                   [ref]: ref.md\n[[wiki|alias#h]] [[note]] [[note.md]]\n";
        let mut f = extract(src);
        f.imports.sort();
        assert_eq!(
            f.imports,
            vec!["../b.md", "a.md", "c d.md", "note.md", "ref.md", "wiki.md"]
        );
    }

    #[test]
    fn inline_code_mentions_keep_only_identifier_shapes() {
        let src =
            "# H\n`foo` `bar()` `Foo::baz` `obj.method()` `main.rs` `a b` `x` `1abc` `--flag`\n";
        let f = extract(src);
        assert_eq!(
            mentions(&f),
            vec![
                ("foo", Some("H")),
                ("bar", Some("H")),
                ("baz", Some("H")),
                ("method", Some("H"))
            ]
        );
    }

    #[test]
    fn a_mention_is_recorded_once_per_heading_and_top_level_has_no_caller() {
        let f = extract("`early`\n# A\n`dup` `dup`\n`dup()`\n# B\n`dup`\n");
        assert_eq!(
            mentions(&f),
            vec![("early", None), ("dup", Some("A")), ("dup", Some("B"))]
        );
        assert_eq!(f.calls[1].line, 3);
    }

    #[test]
    fn a_hash_without_a_space_is_not_a_heading() {
        let f = extract("#hashtag\n#include <x>\n#123\n#\n# Real\n");
        assert_eq!(names(&f), vec![("Real", 5)]);
    }

    #[test]
    fn multibyte_text_crlf_and_an_unclosed_fence_are_safe() {
        let src = "# Café 日本語 🚀\r\n`naïve()` [é](ü.md) [[日本]]\r\n```\r\n`lost()`\r\n";
        let f = extract(src);
        assert_eq!(names(&f), vec![("Café 日本語 🚀", 1)]);
        assert!(
            mentions(&f).is_empty(),
            "non-ASCII names are not identifiers"
        );
        assert_eq!(f.imports, vec!["ü.md", "日本.md"]);
    }

    #[test]
    fn oversized_docs_are_skipped() {
        let big = format!("# T\n{}", "x".repeat(MAX_BYTES));
        assert_eq!(extract(&big), Facts::default());
    }
}
