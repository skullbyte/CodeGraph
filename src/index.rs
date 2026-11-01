//! Hashes files, parses changed ones in parallel, saves and loads graph.json.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use rayon::prelude::*;

use crate::lang::{self, Lang};
use crate::model::{FileEntry, Index};
use crate::{query, walk};

/// Bump when the graph.json layout or extraction rules change; old caches are then ignored.
pub const VERSION: u32 = 9;

/// How many files were parsed afresh versus taken from the cache.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub parsed: usize,
    pub reused: usize,
    /// Files that could not be read (e.g. not UTF-8) or parsed; left out of the index.
    pub skipped: usize,
    /// Saved SCIP files applied again after this run parsed something.
    pub scip_files: usize,
}

/// The readable graph, rewritten on every index/update.
pub fn graph_path(root: &Path) -> PathBuf {
    root.join(".claude").join("graph.json")
}

/// Compact binary copy of the same graph; much faster to load than the JSON, so it is the cache.
fn cache_path(root: &Path) -> PathBuf {
    root.join(".claude").join("graph.bin")
}

/// Is there a graph cache for this project? (Cheaper than loading it.)
pub fn cache_exists(root: &Path) -> bool {
    cache_path(root).is_file()
}

/// Load the cached graph; `None` if missing, unreadable, from another `VERSION`, or built by a
/// different binary (so a changed extractor never reuses stale results).
pub fn load(root: &Path) -> Option<Index> {
    let data = fs::read(cache_path(root)).ok()?;
    let mut index: Index = postcard::from_bytes(&data).ok()?;
    if index.version != VERSION || index.extractor != fingerprint() {
        return None;
    }
    for delta in read_deltas(root, index.epoch) {
        index.aliases = delta.aliases;
        match delta.entry {
            Some(entry) => index.files.insert(delta.path, entry),
            None => index.files.remove(&delta.path),
        };
    }
    Some(index)
}

/// The first fields of graph.bin, read without decoding the graph (same order as `Index`).
#[derive(serde::Deserialize)]
struct Header {
    version: u32,
    extractor: String,
    epoch: u64,
    aliases: BTreeMap<String, String>,
}

/// What an update needs to know about the graph on disk without loading it: its epoch and the
/// import aliases it ended with (the last delta's, else its own). `None` when the graph is missing
/// or not usable by this binary (other version or extractor).
pub fn fast_base(root: &Path) -> Option<(u64, BTreeMap<String, String>)> {
    use std::io::Read;
    let mut buf = Vec::new();
    fs::File::open(cache_path(root))
        .ok()?
        .take(1 << 16) // the aliases sit right after the epoch; a bigger map is not worth it
        .read_to_end(&mut buf)
        .ok()?;
    let (header, _) = postcard::take_from_bytes::<Header>(&buf).ok()?;
    if header.version != VERSION || header.extractor != fingerprint() {
        return None;
    }
    let aliases = read_deltas(root, header.epoch)
        .pop()
        .map_or(header.aliases, |d| d.aliases);
    Some((header.epoch, aliases))
}

/// One change recorded after the last full save: a file's new entry, or `None` once it is gone.
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Delta {
    pub path: String,
    pub entry: Option<FileEntry>,
    pub aliases: BTreeMap<String, String>,
}

/// Past this size the log is folded into the graph by the next full path (it is read on every load).
pub const MAX_DELTA_BYTES: u64 = 1 << 20;

fn delta_path(root: &Path) -> PathBuf {
    root.join(".claude").join("delta.bin")
}

/// Size of the delta log, 0 if there is none (used to notice changes and to decide on compaction).
pub fn delta_len(root: &Path) -> u64 {
    fs::metadata(delta_path(root)).map_or(0, |m| m.len())
}

/// Append changes to the log of the graph with `epoch` (the caller holds the project lock). A log
/// of another epoch is replaced; a torn tail (an append that died half way) is cut off first, or
/// everything after it would be unreadable. Layout: 8 bytes epoch, then `u32 length + postcard`.
pub fn append_deltas(root: &Path, epoch: u64, deltas: &[Delta]) -> Result<()> {
    use std::io::Write;
    let path = delta_path(root);
    let existing = fs::read(&path).unwrap_or_default();
    let (_, valid) = parse_log(&existing, epoch);
    let mut bytes = Vec::new();
    if valid == 0 {
        bytes.extend(epoch.to_le_bytes());
    }
    for delta in deltas {
        let record = postcard::to_stdvec(delta)?;
        bytes.extend(u32::try_from(record.len())?.to_le_bytes());
        bytes.extend(record);
    }
    let mut file = fs::OpenOptions::new();
    file.create(true);
    if valid == existing.len() && valid > 0 {
        file.append(true);
    } else {
        // Replacing a log of another epoch, or cutting a torn tail: keep the good records.
        file.write(true).truncate(true);
        bytes.splice(0..0, existing[..valid].iter().copied());
    }
    file.open(&path)
        .and_then(|mut f| f.write_all(&bytes))
        .with_context(|| format!("writing {}", path.display()))
}

/// The records of a log of `epoch` and how many bytes of `data` they cover (the header
/// included); 0 when the log is empty or belongs to another epoch.
fn parse_log(data: &[u8], epoch: u64) -> (Vec<Delta>, usize) {
    if data.get(..8) != Some(&epoch.to_le_bytes()[..]) {
        return (Vec::new(), 0);
    }
    let mut deltas = Vec::new();
    let mut rest = &data[8..];
    while let Some((len, tail)) = rest.split_first_chunk::<4>() {
        let len = u32::from_le_bytes(*len) as usize;
        let Some(record) = tail.get(..len) else { break };
        let Ok(delta) = postcard::from_bytes(record) else {
            break;
        };
        deltas.push(delta);
        rest = &tail[len..];
    }
    (deltas, data.len() - rest.len())
}

/// The recorded changes of the graph with `epoch`; a torn last record is ignored.
fn read_deltas(root: &Path, epoch: u64) -> Vec<Delta> {
    let data = fs::read(delta_path(root)).unwrap_or_default();
    parse_log(&data, epoch).0
}

/// Size and modification time of the running executable: changes whenever it is rebuilt.
fn fingerprint() -> String {
    let meta = std::env::current_exe().and_then(fs::metadata);
    let Ok(meta) = meta else {
        return "unknown".to_string();
    };
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    format!("{}-{modified}", meta.len())
}

/// Take the project-wide lock (waits if another codegraph run holds it); released when the
/// returned file is dropped, or automatically if the process dies.
pub fn lock(root: &Path) -> Result<fs::File> {
    if !root.is_dir() {
        bail!("project dir {} not found", root.display());
    }
    fs::create_dir_all(root.join(".claude"))?;
    let file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(root.join(".claude").join("graph.lock"))?;
    file.lock()?;
    Ok(file)
}

/// All import aliases of a project: tsconfig's (TypeScript) and composer's (PHP).
pub fn import_aliases(root: &Path) -> BTreeMap<String, String> {
    let mut aliases = tsconfig_aliases(root);
    aliases.extend(composer_aliases(root));
    aliases
}

/// PSR-4 namespace roots from composer.json (`autoload` and `autoload-dev`): `"App\\": "src/"`
/// becomes `"App\\"` -> `"src/"`; several folders are joined with `|`. A PHP prefix holds a
/// backslash, so it never collides with a TypeScript alias in the same map.
fn composer_aliases(root: &Path) -> BTreeMap<String, String> {
    let mut folders: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let Ok(raw) = fs::read_to_string(root.join("composer.json")) else {
        return BTreeMap::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return BTreeMap::new();
    };
    for section in ["autoload", "autoload-dev"] {
        let Some(map) = json[section]["psr-4"].as_object() else {
            continue;
        };
        for (prefix, target) in map {
            if !prefix.ends_with('\\') {
                continue; // the root namespace ("") or a malformed key names no folder
            }
            let targets = match target {
                serde_json::Value::String(one) => vec![one.as_str()],
                serde_json::Value::Array(many) => many.iter().filter_map(|t| t.as_str()).collect(),
                _ => continue,
            };
            for dir in targets {
                let dir = query::normalize(dir);
                let folder = if dir.is_empty() {
                    dir
                } else {
                    format!("{dir}/")
                };
                folders.entry(prefix.clone()).or_default().push(folder);
            }
        }
    }
    // The project root is the folder "", so a list cannot be told apart from "no folder" by emptiness.
    folders
        .into_iter()
        .map(|(prefix, list)| (prefix, list.join("|")))
        .collect()
}

/// Import aliases from `compilerOptions.paths` in tsconfig.json, e.g. `"@/*": ["./src/*"]` becomes
/// `"@/"` -> `"src/"`. With no tsconfig at all, `@/` -> `src/` (the common default).
/// tsconfig allows comments and trailing commas, so clean those first.
fn tsconfig_aliases(root: &Path) -> BTreeMap<String, String> {
    let default = || BTreeMap::from([("@/".to_string(), "src/".to_string())]);
    let Ok(raw) = fs::read_to_string(root.join("tsconfig.json")) else {
        return default();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&strip_jsonc(&raw)) else {
        return default();
    };
    let options = &json["compilerOptions"];
    // A tsconfig without `paths` defines no aliases, so `@/x` must not be guessed.
    let Some(paths) = options["paths"].as_object() else {
        return BTreeMap::new();
    };
    let base = options["baseUrl"].as_str().unwrap_or(".");
    let aliases: BTreeMap<String, String> = paths
        .iter()
        .filter_map(|(key, targets)| {
            // Only wildcard entries (`"~/*": ["./src/*"]`) are folder aliases.
            let prefix = key.strip_suffix('*').filter(|p| !p.is_empty())?;
            let target = targets.get(0)?.as_str()?.strip_suffix('*')?;
            let dir = query::normalize(&format!("{base}/{target}"));
            let folder = if dir.is_empty() {
                dir
            } else {
                format!("{dir}/")
            };
            Some((prefix.to_string(), folder))
        })
        .collect();
    aliases
}

/// Clean tsconfig's JSON-with-comments: drop a leading BOM, `//` and `/* */` comments, and
/// trailing commas, leaving string contents alone.
fn strip_jsonc(raw: &str) -> String {
    strip_trailing_commas(&strip_comments(raw.trim_start_matches('\u{feff}')))
}

fn strip_comments(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    let (mut i, mut in_str) = (0, false);
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if in_str {
            out.push(c);
            if c == '\\' {
                out.extend(next);
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        } else if c == '/' && next == Some('*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        } else {
            in_str = c == '"';
            out.push(c);
        }
        i += 1;
    }
    out
}

/// Comments are already gone, so a comma is trailing when only whitespace precedes `}` or `]`.
fn strip_trailing_commas(json: &str) -> String {
    let chars: Vec<char> = json.chars().collect();
    let mut out = String::with_capacity(json.len());
    let (mut in_str, mut escaped) = (false, false);
    for (i, &c) in chars.iter().enumerate() {
        if in_str {
            in_str = escaped || c != '"';
            escaped = !escaped && c == '\\';
        } else if c == '"' {
            in_str = true;
        } else if c == ',' {
            let rest = chars[i + 1..].iter().find(|ch| !ch.is_whitespace());
            if matches!(rest, Some('}' | ']')) {
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// Write both the JSON graph and the binary cache under `<root>/.claude/`.
pub fn save(root: &Path, index: &mut Index) -> Result<()> {
    fs::create_dir_all(root.join(".claude"))?;
    // A new epoch retires any delta log written for the previous graph.
    index.epoch = new_epoch();
    let index = &*index;
    // The readable JSON (the slow half: ~12 ms on tokio) and the binary cache are independent.
    let (json, bin) = std::thread::scope(|scope| {
        let json = scope.spawn(|| -> Result<()> {
            write_atomic(&graph_path(root), &serde_json::to_vec(index)?)
        });
        let bin = postcard::to_stdvec(index)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| write_atomic(&cache_path(root), &bytes));
        (
            json.join()
                .unwrap_or_else(|_| bail!("writing graph.json panicked")),
            bin,
        )
    });
    let _ = fs::remove_file(delta_path(root)); // after the graph: a reader never loses changes
    json.and(bin)
}

fn new_epoch() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos());
    // Mixed with the process id so two saves in one clock tick still differ.
    ((nanos as u64) ^ (u64::from(std::process::id()) << 40)) | 1
}

/// Write to a temp file, then rename: readers (and parallel hook runs) never see a half-written file.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, data).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| {
        let _ = fs::remove_file(&tmp);
        format!("replacing {}", path.display())
    })
}

/// Index every source file under `root`, re-parsing only files whose hash changed since `previous`.
pub fn build(root: &Path, previous: Option<&Index>) -> Result<(Index, Stats)> {
    let paths = walk::list_files(root)?;
    // `par_iter` runs the closure on all CPU cores; `None` marks a file we had to skip.
    let results: Vec<Option<(FileEntry, bool)>> = paths
        .par_iter()
        .map(|rel| {
            let (src, hash) = read(root, rel).ok()?;
            let cached = previous
                .and_then(|p| p.files.get(rel))
                .filter(|e| e.hash == hash);
            match cached {
                Some(entry) => Some((entry.clone(), true)),
                None => Some((entry_from(rel, &src, hash).ok()?, false)),
            }
        })
        .collect();
    let mut index = Index {
        version: VERSION,
        extractor: fingerprint(),
        epoch: 0,
        aliases: import_aliases(root),
        files: Default::default(),
    };
    let mut stats = Stats::default();
    for result in results {
        let Some((entry, reused)) = result else {
            stats.skipped += 1;
            continue;
        };
        if reused {
            stats.reused += 1;
        } else {
            stats.parsed += 1;
        }
        index.files.insert(entry.path.clone(), entry);
    }
    Ok((index, stats))
}

/// Re-index a single file in place; a file that is gone (or unsupported) is dropped.
pub fn update_file(root: &Path, index: &mut Index, rel: &str) -> Result<()> {
    // tsconfig.json and composer.json aren't indexed files, so refresh their aliases on every update.
    index.aliases = import_aliases(root);
    if !walk::is_indexable(root, rel) || !root.join(rel).is_file() {
        index.files.remove(rel);
        return Ok(());
    }
    let (src, hash) = read(root, rel)?;
    index
        .files
        .insert(rel.to_string(), entry_from(rel, &src, hash)?);
    Ok(())
}

fn read(root: &Path, rel: &str) -> Result<(String, String)> {
    let src = fs::read_to_string(root.join(rel)).with_context(|| format!("reading {rel}"))?;
    let hash = blake3::hash(src.as_bytes()).to_hex().to_string();
    Ok((src, hash))
}

/// The entry for one file as it is on disk now.
pub fn entry_for(root: &Path, rel: &str) -> Result<FileEntry> {
    let (src, hash) = read(root, rel)?;
    entry_from(rel, &src, hash)
}

fn entry_from(rel: &str, src: &str, hash: String) -> Result<FileEntry> {
    let lang = Lang::from_path(rel).with_context(|| format!("unsupported file {rel}"))?;
    let facts = lang::extract(lang, src)?;
    Ok(FileEntry {
        path: rel.to_string(),
        hash,
        lines: u32::try_from(src.lines().count()).unwrap_or(u32::MAX),
        symbols: facts.symbols,
        imports: facts.imports,
        calls: facts.calls,
        uses: facts.uses,
        resolved: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn second_build_reuses_unchanged_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a.rs", "fn a() {}\n");
        write(root, "b.rs", "fn b() { a(); }\n");

        let (first, stats) = build(root, None).unwrap();
        assert_eq!(
            stats,
            Stats {
                parsed: 2,
                reused: 0,
                skipped: 0,
                scip_files: 0
            }
        );

        let (second, stats) = build(root, Some(&first)).unwrap();
        assert_eq!(
            stats,
            Stats {
                parsed: 0,
                reused: 2,
                skipped: 0,
                scip_files: 0
            }
        );
        assert_eq!(first, second);

        write(root, "b.rs", "fn b() { c(); }\n");
        let (third, stats) = build(root, Some(&second)).unwrap();
        assert_eq!(
            stats,
            Stats {
                parsed: 1,
                reused: 1,
                skipped: 0,
                scip_files: 0
            }
        );
        assert_eq!(third.files["b.rs"].calls[0].callee, "c");
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.rs", "fn a() {}\n");
        let (mut index, _) = build(dir.path(), None).unwrap();
        save(dir.path(), &mut index).unwrap();
        assert!(graph_path(dir.path()).is_file());
        assert_eq!(load(dir.path()), Some(index));
    }

    fn saved_project(files: &[(&str, &str)]) -> (tempfile::TempDir, Index) {
        let dir = tempfile::tempdir().unwrap();
        for (path, body) in files {
            write(dir.path(), path, body);
        }
        let (mut index, _) = build(dir.path(), None).unwrap();
        save(dir.path(), &mut index).unwrap();
        (dir, index)
    }

    fn entry_of(rel: &str, src: &str) -> FileEntry {
        entry_from(rel, src, "h".into()).unwrap()
    }

    fn change(path: &str, entry: Option<FileEntry>) -> Delta {
        Delta {
            path: path.into(),
            entry,
            aliases: BTreeMap::new(),
        }
    }

    #[test]
    fn deltas_are_applied_on_load_in_order() {
        let (dir, index) = saved_project(&[("a.rs", "fn a() {}\n"), ("b.rs", "fn b() {}\n")]);
        let root = dir.path();
        let deltas = [
            change("a.rs", Some(entry_of("a.rs", "fn renamed() {}\n"))),
            change("b.rs", None),
            change("c.rs", Some(entry_of("c.rs", "fn c() {}\n"))),
        ];
        append_deltas(root, index.epoch, &deltas[..2]).unwrap();
        append_deltas(root, index.epoch, &deltas[2..]).unwrap();
        let loaded = load(root).unwrap();
        assert_eq!(loaded.files["a.rs"].symbols[0].name, "renamed");
        assert!(!loaded.files.contains_key("b.rs"));
        assert!(loaded.files.contains_key("c.rs"));
        assert_eq!(fast_base(root).map(|b| b.0), Some(index.epoch));
    }

    #[test]
    fn a_log_of_another_epoch_or_a_torn_record_is_ignored() {
        let (dir, index) = saved_project(&[("a.rs", "fn a() {}\n")]);
        let root = dir.path();
        let renamed = change("a.rs", Some(entry_of("a.rs", "fn renamed() {}\n")));
        append_deltas(root, index.epoch + 2, std::slice::from_ref(&renamed)).unwrap();
        assert_eq!(load(root).unwrap().files["a.rs"].symbols[0].name, "a");
        // Right epoch again: replaces the stale log. Then cut the last record in half.
        append_deltas(root, index.epoch, std::slice::from_ref(&renamed)).unwrap();
        assert_eq!(load(root).unwrap().files["a.rs"].symbols[0].name, "renamed");
        let path = delta_path(root);
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, &bytes[..bytes.len() - 3]).unwrap();
        assert_eq!(load(root).unwrap().files["a.rs"].symbols[0].name, "a");
    }

    #[test]
    fn an_append_after_a_torn_record_is_still_read() {
        let (dir, index) = saved_project(&[("a.rs", "fn a() {}\n"), ("b.rs", "fn b() {}\n")]);
        let root = dir.path();
        let first = change("a.rs", Some(entry_of("a.rs", "fn one() {}\n")));
        append_deltas(root, index.epoch, &[first]).unwrap();
        let path = delta_path(root);
        let mut torn = fs::read(&path).unwrap();
        torn.extend([9, 0, 0, 0, 1, 2]); // a record that claims 9 bytes and has 2
        fs::write(&path, torn).unwrap();
        let second = change("b.rs", Some(entry_of("b.rs", "fn two() {}\n")));
        append_deltas(root, index.epoch, &[second]).unwrap();
        let loaded = load(root).unwrap();
        assert_eq!(loaded.files["a.rs"].symbols[0].name, "one");
        assert_eq!(loaded.files["b.rs"].symbols[0].name, "two");
    }

    #[test]
    fn the_header_prefix_gives_epoch_and_aliases_without_the_graph() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.rs", "fn a() {}\n");
        let (mut index, _) = build(dir.path(), None).unwrap();
        index.aliases.insert("@/".into(), "src/".into());
        save(dir.path(), &mut index).unwrap();
        let (epoch, aliases) = fast_base(dir.path()).unwrap();
        assert_eq!((epoch, &aliases), (index.epoch, &index.aliases));
        // The last delta's aliases win over the graph's own.
        let newer = BTreeMap::from([("~/".to_string(), "lib/".to_string())]);
        let moved = Delta {
            path: "a.rs".into(),
            entry: None,
            aliases: newer.clone(),
        };
        append_deltas(dir.path(), epoch, &[moved]).unwrap();
        assert_eq!(fast_base(dir.path()).unwrap().1, newer);
    }

    #[test]
    fn a_full_save_retires_the_log() {
        let (dir, index) = saved_project(&[("a.rs", "fn a() {}\n")]);
        let root = dir.path();
        append_deltas(root, index.epoch, &[change("a.rs", None)]).unwrap();
        assert!(delta_len(root) > 0 && load(root).unwrap().files.is_empty());
        let mut again = load(root).unwrap();
        save(root, &mut again).unwrap();
        assert_eq!(delta_len(root), 0);
        assert_ne!(again.epoch, index.epoch);
        assert!(
            load(root).unwrap().files.is_empty(),
            "the deleted file stays deleted"
        );
    }

    #[test]
    fn a_graph_from_another_version_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.rs", "fn a() {}\n");
        let (mut index, _) = build(dir.path(), None).unwrap();
        index.version = VERSION - 1; // e.g. a v5 graph.bin, before `Form::Returned`
        save(dir.path(), &mut index).unwrap();
        assert_eq!(load(dir.path()), None);
    }

    #[test]
    fn update_file_replaces_and_removes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a.rs", "fn a() {}\n");
        let (mut index, _) = build(root, None).unwrap();

        write(root, "a.rs", "fn renamed() {}\n");
        update_file(root, &mut index, "a.rs").unwrap();
        assert_eq!(index.files["a.rs"].symbols[0].name, "renamed");

        fs::remove_file(root.join("a.rs")).unwrap();
        update_file(root, &mut index, "a.rs").unwrap();
        assert!(index.files.is_empty());
    }

    #[test]
    fn update_ignores_excluded_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "target/gen.rs", "fn g() {}\n");
        write(root, "skip/x.rs", "fn x() {}\n");
        fs::write(root.join(".gitignore"), "skip/\n").unwrap();
        let mut index = Index::default();
        for rel in ["target/gen.rs", "skip/x.rs"] {
            update_file(root, &mut index, rel).unwrap();
        }
        assert!(index.files.is_empty());
    }

    #[test]
    fn unreadable_files_are_counted_as_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "ok.rs", "fn ok() {}\n");
        fs::write(root.join("bad.rs"), [0xff, 0xfe, 0x00]).unwrap();
        let (index, stats) = build(root, None).unwrap();
        assert_eq!(index.files.len(), 1);
        assert_eq!(stats.skipped, 1);
    }

    #[test]
    fn graph_from_another_binary_is_not_reused() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.rs", "fn a() {}\n");
        let (mut index, _) = build(dir.path(), None).unwrap();
        index.extractor = "some-older-build".into();
        save(dir.path(), &mut index).unwrap();
        assert_eq!(load(dir.path()), None);
    }

    #[test]
    fn alias_is_read_from_tsconfig_with_comments() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let default = BTreeMap::from([("@/".to_string(), "src/".to_string())]);
        assert_eq!(build(root, None).unwrap().0.aliases, default);
        fs::write(
            root.join("tsconfig.json"),
            r#"{
  // Next.js style
  "compilerOptions": {
    "baseUrl": "web", /* relative to this file */
    "paths": {
      "@/*": ["./app/*"],
      "~/*": ["./lib/*"],
      "@components/*": ["./app/components/*"],
      "@exact": ["x.ts"],
    },
  },
}"#,
        )
        .unwrap();
        let want = BTreeMap::from([
            ("@/".to_string(), "web/app/".to_string()),
            ("~/".to_string(), "web/lib/".to_string()),
            (
                "@components/".to_string(),
                "web/app/components/".to_string(),
            ),
        ]);
        assert_eq!(build(root, None).unwrap().0.aliases, want);
    }

    #[test]
    fn lock_serializes_concurrent_writers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let counter = root.join("counter");
        fs::write(&counter, "0").unwrap();
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    let _guard = lock(root).unwrap();
                    let n: u32 = fs::read_to_string(&counter).unwrap().parse().unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    fs::write(&counter, (n + 1).to_string()).unwrap();
                });
            }
        });
        assert_eq!(fs::read_to_string(&counter).unwrap(), "8");
    }

    #[test]
    fn strip_jsonc_handles_tricky_shapes() {
        let cases = [
            ("{\"a\": 1, // note\n}", r#"{"a":1}"#),
            ("{\"a\": [1, /* x */ ]}", r#"{"a":[1]}"#),
            (r#"{"u": "http://x/*y*/"}"#, r#"{"u":"http://x/*y*/"}"#),
            (r#"{"q": "a\"//b", }"#, r#"{"q":"a\"//b"}"#),
            (r#"{"c": "x,}", }"#, r#"{"c":"x,}"}"#),
            ("\u{feff}{\"a\": 1}", r#"{"a":1}"#),
            ("{\"a\": 1} /* never closed", r#"{"a":1}"#),
        ];
        for (input, want) in cases {
            let got: serde_json::Value = serde_json::from_str(&strip_jsonc(input)).unwrap();
            let want: serde_json::Value = serde_json::from_str(want).unwrap();
            assert_eq!(got, want, "input: {input}");
        }
    }

    #[test]
    fn composer_psr4_roots_become_aliases_with_several_folders_joined() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("composer.json"),
            r#"{"autoload":{"psr-4":{"App\\":"src/","Lib\\":["a/","./b"],"Root\\":[".","src/"],"":"x/","Bad":"y/"}},
                "autoload-dev":{"psr-4":{"App\\":"tests/"}}}"#,
        )
        .unwrap();
        let aliases = composer_aliases(root);
        assert_eq!(
            aliases.get("App\\").map(String::as_str),
            Some("src/|tests/")
        );
        assert_eq!(aliases.get("Lib\\").map(String::as_str), Some("a/|b/"));
        // The project root is a folder too: it must survive next to another folder.
        assert_eq!(aliases.get("Root\\").map(String::as_str), Some("|src/"));
        assert_eq!(
            aliases.len(),
            3,
            "root namespace and keys without a backslash are skipped"
        );
        assert!(composer_aliases(&root.join("missing")).is_empty());
    }

    #[test]
    fn tsconfig_without_paths_has_no_aliases_and_root_folder_alias_works() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions": {"strict": true}}"#,
        )
        .unwrap();
        assert!(tsconfig_aliases(root).is_empty());
        fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions": {"paths": {"~/*": ["./*"]}}}"#,
        )
        .unwrap();
        assert_eq!(
            tsconfig_aliases(root),
            BTreeMap::from([("~/".to_string(), String::new())])
        );
    }

    #[test]
    fn update_refreshes_aliases_and_lock_rejects_missing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (mut index, _) = build(root, None).unwrap();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions": {"paths": {"~/*": ["./lib/*"]}}}"#,
        )
        .unwrap();
        update_file(root, &mut index, "gone.rs").unwrap();
        assert!(index.aliases.contains_key("~/"));

        let missing = root.join("typo");
        assert!(lock(&missing).is_err());
        assert!(!missing.exists());
    }
}
