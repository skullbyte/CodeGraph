//! The optional AI layer for documents: a store of what Claude read in them (concepts and links to
//! code), kept in `.claude/semantic.json`, cached by file hash. Agent output is untrusted: `ingest`
//! validates every field and never executes anything from it.
//! Schema and labels follow Graphify's extraction spec (`skills/claude/references/extraction-spec.md`);
//! the secret-name filter is a port of `_is_sensitive` in Graphify's `detect.py`.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::index;
use crate::model::Index;
use crate::query::{self, CallerHit, DefHit, Explain};
use crate::walk;

/// Inline (the main agent reads the docs) up to this many pending lines; above it, subagents.
pub const INLINE_MAX_LINES: usize = 1500;
/// Lines per subagent chunk.
pub const CHUNK_LINES: usize = 6000;
/// Docs bigger than this are never offered (same cap as the markdown scan).
pub const MAX_DOC_BYTES: u64 = 1 << 20;
pub const MAX_INGEST_BYTES: usize = 1 << 20;
pub const MAX_CONCEPTS: usize = 200;
pub const MAX_LINKS: usize = 500;
pub const MAX_NAME_CHARS: usize = 110;
/// Hashes are shown (and accepted) as this many leading hex characters.
pub const SHORT_HASH: usize = 16;

pub const RELATIONS: [&str; 4] = [
    "describes",
    "rationale_for",
    "references",
    "conceptually_related_to",
];
pub const CONFIDENCES: [&str; 3] = ["EXTRACTED", "INFERRED", "AMBIGUOUS"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Concept {
    pub name: String,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub line: u32,
    pub to: String,
    pub rel: String,
    pub conf: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocEntry {
    pub hash: String,
    pub concepts: Vec<Concept>,
    pub links: Vec<Link>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    pub files: BTreeMap<String, DocEntry>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct PendingDoc {
    pub path: String,
    pub lines: usize,
    pub hash: String,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Pending {
    pub docs: Vec<PendingDoc>,
    pub total_lines: usize,
    /// Empty when inline; otherwise the paths each subagent reads.
    pub chunks: Vec<Vec<String>>,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub concepts: usize,
    pub links: usize,
    pub dropped: usize,
}

pub fn store_path(root: &Path) -> PathBuf {
    root.join(".claude").join("semantic.json")
}

/// The saved store; empty when none exists yet, an error when the file is damaged.
pub fn load_store(root: &Path) -> Result<Store> {
    let path = store_path(root);
    if !path.is_file() {
        return Ok(Store::default());
    }
    let data = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&data).with_context(|| format!("parsing {}", path.display()))
}

pub fn save_store(root: &Path, store: &Store) -> Result<()> {
    fs::create_dir_all(root.join(".claude"))?;
    index::write_atomic(&store_path(root), &serde_json::to_vec(store)?)
}

/// Names that look like secrets: never offered to an agent.
pub fn is_sensitive(rel: &str) -> bool {
    let lower = rel.to_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let template = [".example", ".sample", ".template", ".dist"]
        .iter()
        .any(|t| name.ends_with(t));
    let env = name.starts_with(".env") && !template;
    let words = [
        "secret",
        "credential",
        "password",
        "passwd",
        "token",
        "id_rsa",
        "id_ed25519",
        ".npmrc",
        ".netrc",
    ];
    env || words.iter().any(|w| lower.contains(w))
        || name.ends_with(".pem")
        || [".key", ".pfx", ".p12"].iter().any(|e| name.ends_with(e))
}

/// Docs whose current content is not in the store yet, with the plan for reading them.
pub fn pending(root: &Path, store: &Store, max_lines: Option<usize>) -> Result<Pending> {
    let mut docs = Vec::new();
    let mut total_lines = 0;
    for rel in walk::list_docs(root)? {
        if is_sensitive(&rel) {
            continue;
        }
        let path = root.join(&rel);
        if fs::metadata(&path).map_or(true, |m| m.len() > MAX_DOC_BYTES) {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else { continue };
        let full = blake3::hash(&bytes).to_hex().to_string();
        if store.files.get(&rel).is_some_and(|e| e.hash == full) {
            continue;
        }
        let lines = if rel.to_lowercase().ends_with(".pdf") {
            0
        } else {
            count_lines(&bytes)
        };
        if max_lines.is_some_and(|cap| !docs.is_empty() && total_lines + lines > cap) {
            break;
        }
        total_lines += lines;
        docs.push(PendingDoc {
            path: rel,
            lines,
            hash: full[..SHORT_HASH].to_string(),
        });
    }
    let chunks = if total_lines <= INLINE_MAX_LINES {
        Vec::new()
    } else {
        plan_chunks(&docs)
    };
    Ok(Pending {
        docs,
        total_lines,
        chunks,
    })
}

fn count_lines(bytes: &[u8]) -> usize {
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count();
    newlines + usize::from(bytes.last().is_some_and(|&b| b != b'\n'))
}

/// Pack docs in order into chunks of at most `CHUNK_LINES` (a bigger doc gets a chunk of its own).
fn plan_chunks(docs: &[PendingDoc]) -> Vec<Vec<String>> {
    let mut chunks: Vec<Vec<String>> = Vec::new();
    let mut used = 0;
    for doc in docs {
        if chunks.is_empty() || used + doc.lines > CHUNK_LINES {
            chunks.push(Vec::new());
            used = 0;
        }
        used += doc.lines;
        if let Some(chunk) = chunks.last_mut() {
            chunk.push(doc.path.clone());
        }
    }
    chunks
}

/// What an agent writes for one doc.
#[derive(Deserialize)]
struct Submission {
    file: String,
    hash: String,
    #[serde(default)]
    concepts: Vec<Concept>,
    #[serde(default)]
    links: Vec<Link>,
}

/// Validate one agent submission and store it. Wrong file, stale hash, bad JSON or too much data
/// is an error; a single bad link or concept is dropped and counted.
pub fn ingest(root: &Path, index: &Index, store: &mut Store, raw: &str) -> Result<Summary> {
    if raw.len() > MAX_INGEST_BYTES {
        bail!("input is over {MAX_INGEST_BYTES} bytes");
    }
    let sub: Submission = serde_json::from_str(raw).context("input is not the expected JSON")?;
    if sub.concepts.len() > MAX_CONCEPTS || sub.links.len() > MAX_LINKS {
        bail!("over {MAX_CONCEPTS} concepts or {MAX_LINKS} links for one file");
    }
    let (docs, full, lines) = current_doc(root, &sub.file)?;
    let given = sub.hash.to_lowercase();
    if given.len() < SHORT_HASH || !full.starts_with(&given) {
        bail!(
            "{} changed or the hash is wrong; run `docs pending` again",
            sub.file
        );
    }
    let mut summary = Summary::default();
    let concepts = clean_concepts(sub.concepts, lines, &mut summary);
    let known = known_targets(index, store, &sub.file, &concepts);
    let links = clean_links(sub.links, lines, &known, &mut summary);
    summary.concepts = concepts.len();
    summary.links = links.len();
    // Entries of docs that were deleted, renamed or became secret-named must not keep answering.
    let live: HashSet<&str> = docs.iter().map(String::as_str).collect();
    store
        .files
        .retain(|path, _| live.contains(path.as_str()) && !is_sensitive(path));
    store.files.insert(
        sub.file,
        DocEntry {
            hash: full,
            concepts,
            links,
        },
    );
    Ok(summary)
}

/// Keep concepts with a usable name and a line inside the doc; count the rest as dropped.
fn clean_concepts(raw: Vec<Concept>, lines: usize, summary: &mut Summary) -> Vec<Concept> {
    let mut kept: Vec<Concept> = Vec::new();
    for c in raw {
        let name = clean(&c.name);
        let duplicate = kept.iter().any(|k| k.name == name);
        if name.is_empty() || duplicate || !line_ok(c.line, lines) {
            summary.dropped += 1;
        } else {
            kept.push(Concept { name, line: c.line });
        }
    }
    kept
}

/// Keep links to a known target with a valid relation, confidence and line.
fn clean_links(
    raw: Vec<Link>,
    lines: usize,
    known: &HashSet<&str>,
    summary: &mut Summary,
) -> Vec<Link> {
    let mut kept = Vec::new();
    for l in raw {
        let to = clean(&l.to);
        let valid = known.contains(to.as_str())
            && line_ok(l.line, lines)
            && RELATIONS.contains(&l.rel.as_str())
            && CONFIDENCES.contains(&l.conf.as_str());
        if valid {
            kept.push(Link { to, ..l });
        } else {
            summary.dropped += 1;
        }
    }
    kept
}

fn line_ok(line: u32, lines: usize) -> bool {
    line >= 1 && line as usize <= lines
}

/// For a doc that `docs pending` could offer: all doc paths, its full hash and its line count.
fn current_doc(root: &Path, rel: &str) -> Result<(Vec<String>, String, usize)> {
    let docs = walk::list_docs(root)?;
    if is_sensitive(rel) || !docs.iter().any(|d| d == rel) {
        bail!("{rel} is not a document of this project");
    }
    let path = root.join(rel);
    if fs::metadata(&path)?.len() > MAX_DOC_BYTES {
        bail!("{rel} is over {MAX_DOC_BYTES} bytes");
    }
    let bytes = fs::read(&path).with_context(|| format!("reading {rel}"))?;
    let lines = if rel.to_lowercase().ends_with(".pdf") {
        usize::MAX
    } else {
        count_lines(&bytes)
    };
    Ok((docs, blake3::hash(&bytes).to_hex().to_string(), lines))
}

/// Without control characters (a newline would fake an extra result line), trimmed, cut to the cap.
fn clean(name: &str) -> String {
    let hidden = |c: char| {
        c.is_control()
            || matches!(c, '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
    };
    let plain: String = name.chars().filter(|&c| !hidden(c)).collect();
    plain.trim().chars().take(MAX_NAME_CHARS).collect()
}

/// Names a link may point to: code symbols, concepts of other docs, and this doc's own concepts.
fn known_targets<'a>(
    index: &'a Index,
    store: &'a Store,
    file: &str,
    own: &'a [Concept],
) -> HashSet<&'a str> {
    let code = index.files.values().flat_map(|f| {
        f.symbols
            .iter()
            .filter(|s| s.kind.is_code_def())
            .map(|s| s.name.as_str())
    });
    let others = store
        .files
        .iter()
        .filter(|(path, _)| path.as_str() != file)
        .flat_map(|(_, e)| e.concepts.iter().map(|c| c.name.as_str()));
    code.chain(others)
        .chain(own.iter().map(|c| c.name.as_str()))
        .collect()
}

/// `def` plus the concepts the AI pass found with this name (`[concept]` lines, after code).
pub fn def_with(index: &Index, store: &Store, name: &str) -> Vec<DefHit> {
    let mut hits = query::def(index, name);
    hits.extend(concept_hits(store, name));
    hits
}

/// `callers` plus doc links pointing at the name (AMBIGUOUS links stay hidden).
pub fn callers_with(index: &Index, store: &Store, name: &str) -> Vec<CallerHit> {
    let mut hits = query::callers(index, name);
    hits.extend(link_hits(store, name));
    hits
}

/// `explain` with concepts among the definitions and doc links among the incoming links.
pub fn explain_with(index: &Index, store: &Store, name: &str) -> Explain {
    let mut explained = query::explain(index, name);
    explained.defs.extend(concept_hits(store, name));
    explained
        .incoming
        .extend(link_hits(store, name).into_iter().map(|h| query::Link {
            name: format!("[doc] {}", h.caller.unwrap_or_default()),
            location: Some(format!("{}:{}", h.path, h.line)),
            sites: 1,
        }));
    explained
}

fn concept_hits(store: &Store, name: &str) -> Vec<DefHit> {
    store
        .files
        .iter()
        .flat_map(|(path, e)| {
            e.concepts
                .iter()
                .filter(|c| c.name == name)
                .map(move |c| DefHit {
                    path: path.clone(),
                    line: c.line,
                    signature: format!("[concept] {}", c.name),
                })
        })
        .collect()
}

fn link_hits(store: &Store, name: &str) -> Vec<CallerHit> {
    store
        .files
        .iter()
        .flat_map(|(path, e)| {
            e.links
                .iter()
                .filter(|l| l.to == name && l.conf != "AMBIGUOUS")
                .map(move |l| CallerHit {
                    path: path.clone(),
                    line: l.line,
                    caller: Some(l.rel.clone()),
                    target: None,
                    doc: true,
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/lib.rs", "pub fn explain() {}\n");
        write(dir.path(), "docs/a.md", "# A\nrate limiting\n");
        dir
    }

    fn hash_of(root: &Path, rel: &str) -> String {
        let body = fs::read(root.join(rel)).unwrap();
        blake3::hash(&body).to_hex()[..SHORT_HASH].to_string()
    }

    fn built(root: &Path) -> Index {
        index::build(root, None).unwrap().0
    }

    fn json(file: &str, hash: &str, tail: &str) -> String {
        format!(r#"{{"file":"{file}","hash":"{hash}",{tail}}}"#)
    }

    #[test]
    fn secret_looking_names_are_sensitive() {
        for bad in [
            ".env",
            "config/.env.local",
            "my-secret.md",
            "Credentials.txt",
            "db_password.txt",
            "a/server.pem",
            "k.key",
        ] {
            assert!(is_sensitive(bad), "{bad}");
        }
        for ok in [".env.example", "env.template", "docs/a.md", "keyboard.md"] {
            assert!(!is_sensitive(ok), "{ok}");
        }
    }

    #[test]
    fn pending_lists_only_new_docs() {
        let dir = project();
        write(dir.path(), "notes/.env", "TOKEN=1\n");
        write(dir.path(), "docs/my-secret.md", "hidden\n");
        let p = pending(dir.path(), &Store::default(), None).unwrap();
        let paths: Vec<_> = p.docs.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(paths, ["docs/a.md"]);
        assert_eq!(p.docs[0].lines, 2);
        assert_eq!(p.docs[0].hash, hash_of(dir.path(), "docs/a.md"));
        assert!(p.chunks.is_empty(), "small jobs are inline");
    }

    #[test]
    fn a_stored_hash_is_not_pending_but_a_changed_file_is() {
        let dir = project();
        let mut store = Store::default();
        let full = blake3::hash(&fs::read(dir.path().join("docs/a.md")).unwrap())
            .to_hex()
            .to_string();
        store.files.insert(
            "docs/a.md".into(),
            DocEntry {
                hash: full,
                concepts: vec![],
                links: vec![],
            },
        );
        assert!(pending(dir.path(), &store, None).unwrap().docs.is_empty());
        write(dir.path(), "docs/a.md", "# A\nchanged\n");
        assert_eq!(pending(dir.path(), &store, None).unwrap().docs.len(), 1);
    }

    #[test]
    fn big_jobs_are_chunked_and_max_lines_caps_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let body = "line\n".repeat(4000);
        for n in 0..3 {
            write(dir.path(), &format!("d/{n}.md"), &body);
        }
        let p = pending(dir.path(), &Store::default(), None).unwrap();
        assert_eq!(p.total_lines, 12000);
        assert_eq!(
            p.chunks.len(),
            3,
            "4000-line docs do not share a 6000 chunk"
        );
        let capped = pending(dir.path(), &Store::default(), Some(5000)).unwrap();
        assert_eq!(capped.docs.len(), 1, "stops before going over the cap");
    }

    #[test]
    fn ingest_stores_valid_concepts_and_links() {
        let dir = project();
        let idx = built(dir.path());
        let mut store = Store::default();
        let h = hash_of(dir.path(), "docs/a.md");
        let raw = json(
            "docs/a.md",
            &h,
            r#""concepts":[{"name":"  rate limiting ","line":2}],"links":[{"line":2,"to":"explain","rel":"describes","conf":"INFERRED"},{"line":2,"to":"rate limiting","rel":"references","conf":"EXTRACTED"}]"#,
        );
        let s = ingest(dir.path(), &idx, &mut store, &raw).unwrap();
        assert_eq!(
            s,
            Summary {
                concepts: 1,
                links: 2,
                dropped: 0
            }
        );
        let e = &store.files["docs/a.md"];
        assert_eq!(e.concepts[0].name, "rate limiting");
        assert!(
            e.hash.starts_with(&h) && e.hash.len() == 64,
            "full hash stored"
        );
        assert!(pending(dir.path(), &store, None).unwrap().docs.is_empty());
    }

    #[test]
    fn ingest_drops_bad_links_and_counts_them() {
        let dir = project();
        let idx = built(dir.path());
        let mut store = Store::default();
        let h = hash_of(dir.path(), "docs/a.md");
        let raw = json(
            "docs/a.md",
            &h,
            r#""concepts":[],"links":[{"line":1,"to":"nope","rel":"describes","conf":"INFERRED"},{"line":1,"to":"explain","rel":"bogus","conf":"INFERRED"},{"line":1,"to":"explain","rel":"describes","conf":"MAYBE"},{"line":1,"to":"explain","rel":"describes","conf":"AMBIGUOUS"}]"#,
        );
        let s = ingest(dir.path(), &idx, &mut store, &raw).unwrap();
        assert_eq!(
            s,
            Summary {
                concepts: 0,
                links: 1,
                dropped: 3
            }
        );
    }

    #[test]
    fn ingest_rejects_stale_unknown_and_oversized_input() {
        let dir = project();
        let idx = built(dir.path());
        let mut store = Store::default();
        let ok = hash_of(dir.path(), "docs/a.md");
        let body = r#""concepts":[],"links":[]"#;
        let mut try_it = |raw: String| ingest(dir.path(), &idx, &mut store, &raw).is_err();
        assert!(
            try_it(json("docs/a.md", "0000000000000000", body)),
            "stale hash"
        );
        assert!(try_it(json("docs/a.md", "abc", body)), "short hash");
        assert!(try_it(json("docs/missing.md", &ok, body)), "not a doc here");
        assert!(try_it(json("../x.md", &ok, body)), "path escape");
        assert!(try_it(json("src/lib.rs", &ok, body)), "not a doc file");
        assert!(try_it("not json".into()));
        assert!(try_it(" ".repeat(MAX_INGEST_BYTES + 1)), "too big");
        let many: Vec<String> = (0..=MAX_CONCEPTS)
            .map(|n| format!(r#"{{"name":"c{n}","line":1}}"#))
            .collect();
        let raw = json(
            "docs/a.md",
            &ok,
            &format!(r#""concepts":[{}],"links":[]"#, many.join(",")),
        );
        assert!(try_it(raw), "too many concepts");
    }

    #[test]
    fn ingest_refuses_secret_named_docs() {
        let dir = project();
        write(dir.path(), "docs/my-secret.md", "x\n");
        let idx = built(dir.path());
        let mut store = Store::default();
        let h = hash_of(dir.path(), "docs/my-secret.md");
        let raw = json("docs/my-secret.md", &h, r#""concepts":[],"links":[]"#);
        assert!(ingest(dir.path(), &idx, &mut store, &raw).is_err());
    }

    #[test]
    fn names_are_cut_to_the_cap() {
        let dir = project();
        let idx = built(dir.path());
        let mut store = Store::default();
        let h = hash_of(dir.path(), "docs/a.md");
        let long = "x".repeat(500);
        let raw = json(
            "docs/a.md",
            &h,
            &format!(r#""concepts":[{{"name":"{long}","line":1}}],"links":[]"#),
        );
        ingest(dir.path(), &idx, &mut store, &raw).unwrap();
        assert_eq!(
            store.files["docs/a.md"].concepts[0].name.chars().count(),
            MAX_NAME_CHARS
        );
    }

    #[test]
    fn store_round_trips_and_a_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_store(dir.path()).unwrap().files.is_empty());
        let mut store = Store::default();
        store.files.insert(
            "a.md".into(),
            DocEntry {
                hash: "h".into(),
                concepts: vec![],
                links: vec![],
            },
        );
        save_store(dir.path(), &store).unwrap();
        assert_eq!(load_store(dir.path()).unwrap().files, store.files);
    }

    fn store_with(file: &str, concept: &str, to: &str, conf: &str) -> Store {
        let mut store = Store::default();
        store.files.insert(
            file.into(),
            DocEntry {
                hash: "h".into(),
                concepts: vec![Concept {
                    name: concept.into(),
                    line: 3,
                }],
                links: vec![Link {
                    line: 4,
                    to: to.into(),
                    rel: "describes".into(),
                    conf: conf.into(),
                }],
            },
        );
        store
    }

    #[test]
    fn def_finds_concepts_after_code() {
        let dir = project();
        let idx = built(dir.path());
        let store = store_with("docs/a.md", "rate limiting", "explain", "INFERRED");
        let hits = def_with(&idx, &store, "rate limiting");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text(), "docs/a.md:3 [concept] rate limiting");
        assert_eq!(
            def_with(&idx, &store, "explain").len(),
            1,
            "code hit unchanged"
        );
    }

    #[test]
    fn callers_and_explain_show_links_but_hide_ambiguous() {
        let dir = project();
        let idx = built(dir.path());
        let store = store_with("docs/a.md", "rate limiting", "explain", "INFERRED");
        let hits = callers_with(&idx, &store, "explain");
        assert_eq!(hits[0].text(), "docs/a.md:4  in describes  [doc]");
        assert!(
            explain_with(&idx, &store, "explain")
                .lines()
                .iter()
                .any(|l| l.contains("docs/a.md:4"))
        );
        let hidden = store_with("docs/a.md", "rate limiting", "explain", "AMBIGUOUS");
        assert!(callers_with(&idx, &hidden, "explain").is_empty());
    }

    #[test]
    fn control_characters_and_bad_lines_are_dropped() {
        let dir = project();
        let idx = built(dir.path());
        let mut store = Store::default();
        let h = hash_of(dir.path(), "docs/a.md");
        let raw = json(
            "docs/a.md",
            &h,
            r#""concepts":[{"name":"x\nsrc/auth.rs:1 fn login","line":1},{"name":"far","line":99},{"name":"zero","line":0}],"links":[]"#,
        );
        let s = ingest(dir.path(), &idx, &mut store, &raw).unwrap();
        assert_eq!(s.concepts, 1);
        assert_eq!(s.dropped, 2);
        assert_eq!(
            store.files["docs/a.md"].concepts[0].name,
            "xsrc/auth.rs:1 fn login"
        );
    }

    #[test]
    fn ingest_forgets_docs_that_no_longer_exist() {
        let dir = project();
        let idx = built(dir.path());
        let mut store = store_with("docs/gone.md", "old", "explain", "INFERRED");
        let h = hash_of(dir.path(), "docs/a.md");
        let raw = json("docs/a.md", &h, r#""concepts":[],"links":[]"#);
        ingest(dir.path(), &idx, &mut store, &raw).unwrap();
        assert!(!store.files.contains_key("docs/gone.md"));
    }

    #[test]
    fn ingest_forgets_entries_that_became_secret_named() {
        let dir = project();
        write(dir.path(), "docs/my-secret.md", "x\n");
        let idx = built(dir.path());
        let mut store = store_with("docs/my-secret.md", "old", "explain", "INFERRED");
        let h = hash_of(dir.path(), "docs/a.md");
        let raw = json(
            "docs/a.md",
            &h,
            r#""concepts":[{"name":"a\u202eb\u200bc","line":1}],"links":[]"#,
        );
        ingest(dir.path(), &idx, &mut store, &raw).unwrap();
        assert!(!store.files.contains_key("docs/my-secret.md"));
        assert_eq!(store.files["docs/a.md"].concepts[0].name, "abc");
    }

    #[test]
    fn a_repeated_concept_name_is_kept_once() {
        let dir = project();
        let idx = built(dir.path());
        let mut store = Store::default();
        let h = hash_of(dir.path(), "docs/a.md");
        let raw = json(
            "docs/a.md",
            &h,
            r#""concepts":[{"name":"x","line":1},{"name":" x ","line":2}],"links":[]"#,
        );
        let s = ingest(dir.path(), &idx, &mut store, &raw).unwrap();
        assert_eq!((s.concepts, s.dropped), (1, 1));
    }

    #[test]
    fn explain_lists_a_concept_as_a_definition() {
        let dir = project();
        let idx = built(dir.path());
        let store = store_with("docs/a.md", "rate limiting", "explain", "INFERRED");
        let lines = explain_with(&idx, &store, "rate limiting").lines();
        assert_eq!(lines, ["def docs/a.md:3 [concept] rate limiting"]);
    }
}
