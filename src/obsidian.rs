//! Obsidian vault export: one note per file plus one per community, linked with `[[...]]`.
//! Graphify's version is `to_obsidian` in `export.py`; ours is files only and writes plain Markdown.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path};

use anyhow::{Context, Result};

use crate::community::{self, Community, FileGraph};
use crate::index;
use crate::model::{Index, Kind};

/// Every note as (path inside the vault, text): `<file path>.md` and `Community N.md`.
pub fn notes(index: &Index) -> Vec<(String, String)> {
    let graph = community::file_graph(index);
    let found = community::detect(&graph);
    let community_of: HashMap<&str, usize> = found
        .iter()
        .flat_map(|c| c.files.iter().map(|f| (f.as_str(), c.id)))
        .collect();
    let neighbours = neighbours(&graph);
    let mut out: Vec<(String, String)> = graph
        .files
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let community = community_of.get(path.as_str()).copied().unwrap_or(0);
            let links: Vec<&str> = neighbours[i]
                .iter()
                .map(|&j| graph.files[j].as_str())
                .collect();
            (
                format!("{path}.md"),
                file_note(index, path, community, &links),
            )
        })
        .collect();
    out.extend(
        found
            .iter()
            .map(|c| (format!("Community {}.md", c.id), community_note(c))),
    );
    out
}

/// Write the vault into `out`; returns how many notes were written.
pub fn write(index: &Index, out: &Path) -> Result<usize> {
    let mut written = 0;
    for (rel, text) in &notes(index) {
        // Paths come from our own walker, but a `..` must never reach outside the vault.
        if Path::new(rel)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            continue;
        }
        let path = out.join(rel);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        index::write_atomic(&path, text.as_bytes())?;
        written += 1;
    }
    Ok(written)
}

/// File index -> sorted indexes of linked files.
fn neighbours(graph: &FileGraph) -> Vec<Vec<usize>> {
    let mut next = vec![Vec::new(); graph.files.len()];
    for &(a, b) in graph.edges.keys() {
        next[a].push(b);
        next[b].push(a);
    }
    next.iter_mut().for_each(|list| list.sort_unstable());
    next
}

fn file_note(index: &Index, path: &str, community: usize, links: &[&str]) -> String {
    let mut text = format!(
        "---\ncommunity: {community}\n---\n# {path}\n\nIn [[Community {community}]].\n\n## Symbols\n"
    );
    let symbols = index
        .files
        .get(path)
        .map(|f| f.symbols.as_slice())
        .unwrap_or_default();
    for s in symbols.iter().filter(|s| s.kind != Kind::Impl) {
        text.push_str(&format!(
            "- {} {} (line {})\n",
            kind_word(s.kind),
            s.name,
            s.line
        ));
    }
    text.push_str("\n## Links\n");
    for link in links {
        text.push_str(&format!("- {}\n", wikilink(link)));
    }
    text
}

fn community_note(c: &Community) -> String {
    let mut text = format!(
        "# Community {}\n\nCohesion {:.2}\n\n## Files\n",
        c.id, c.cohesion
    );
    for file in &c.files {
        text.push_str(&format!("- {}\n", wikilink(file)));
    }
    text
}

/// `[[src/a.rs.md|src/a.rs]]`: the `.md` makes Obsidian find the note even though the name has a dot.
fn wikilink(path: &str) -> String {
    format!("[[{path}.md|{path}]]")
}

fn kind_word(kind: Kind) -> String {
    format!("{kind:?}").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::{self, Lang};
    use crate::model::FileEntry;

    fn index_of(files: &[(&str, &str)]) -> Index {
        let mut index = Index::default();
        for (path, src) in files {
            let facts = lang::extract(Lang::Rust, src).unwrap();
            index.files.insert(
                path.to_string(),
                FileEntry {
                    path: path.to_string(),
                    symbols: facts.symbols,
                    calls: facts.calls,
                    ..FileEntry::default()
                },
            );
        }
        index
    }

    fn two_files() -> Index {
        index_of(&[
            ("a.rs", "pub fn a() { b(); }\n"),
            ("sub/b.rs", "pub fn b() {}\n"),
        ])
    }

    #[test]
    fn file_note_lists_symbols_and_links() {
        let all: HashMap<_, _> = notes(&two_files()).into_iter().collect();
        let a = &all["a.rs.md"];
        assert!(a.starts_with("---\ncommunity: "));
        assert!(a.contains("- fn a (line 1)\n"));
        assert!(a.contains("- [[sub/b.rs.md|sub/b.rs]]\n"));
        assert!(all["sub/b.rs.md"].contains("- [[a.rs.md|a.rs]]\n"));
    }

    #[test]
    fn isolated_file_gets_a_community_and_empty_index_no_notes() {
        let all: HashMap<_, _> = notes(&index_of(&[("solo.rs", "pub fn solo() {}\n")]))
            .into_iter()
            .collect();
        assert!(all["solo.rs.md"].contains("In [[Community "));
        assert!(all.keys().any(|path| path.starts_with("Community ")));
        assert!(notes(&Index::default()).is_empty());
    }

    #[test]
    fn community_note_lists_its_files() {
        let all: HashMap<_, _> = notes(&two_files()).into_iter().collect();
        let community = all
            .iter()
            .find(|(path, _)| path.starts_with("Community "))
            .unwrap()
            .1;
        assert!(
            community.contains("[[a.rs.md|a.rs]]")
                && community.contains("[[sub/b.rs.md|sub/b.rs]]")
        );
    }

    #[test]
    fn write_creates_notes_and_skips_escaping_paths() {
        let mut index = two_files();
        let evil = FileEntry {
            path: "../evil.rs".into(),
            ..FileEntry::default()
        };
        index.files.insert("../evil.rs".into(), evil);
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        assert_eq!(write(&index, &vault).unwrap(), notes(&index).len() - 1);
        assert!(vault.join("a.rs.md").is_file() && vault.join("sub/b.rs.md").is_file());
        assert!(!dir.path().join("evil.rs.md").exists());
    }
}
