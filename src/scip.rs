//! Reader for SCIP indexes (`index.scip`, written by `rust-analyzer scip .`, `scip-typescript index`).
//! A SCIP file is protobuf; only the few fields codegraph needs are decoded, by hand, so there is
//! no extra dependency. Field numbers are from sourcegraph/scip `scip.proto`. Untrusted input:
//! every length is checked against the bytes left, and nothing here panics on garbage.

use std::collections::{BTreeMap, HashMap};

use anyhow::{Result, bail};

use crate::model::{Index, Resolution, ResolvedCall};

/// `Occurrence.symbol_roles` bit meaning "this occurrence is the definition".
const DEFINITION: u64 = 0x1;

/// One identifier in a document: where it is and which symbol it names.
#[derive(Debug, PartialEq, Eq)]
pub struct Occurrence<'a> {
    /// 0-based line and column where the identifier starts.
    pub line: u32,
    pub col: u32,
    pub symbol: &'a str,
    pub definition: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Document<'a> {
    pub path: &'a str,
    pub occurrences: Vec<Occurrence<'a>>,
}

/// How far above an identifier its call may start (a method chain spreads over several lines).
const CHAIN_LINES: u32 = 8;

/// What `apply` did, for the one-line report.
#[derive(Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Stats {
    /// Calls in the files the index covers.
    pub calls: usize,
    /// Calls now resolved (project definition or external).
    pub resolved: usize,
    pub external: usize,
    /// Index documents with no matching indexed file (the indexer ran from another root?).
    pub unknown_documents: usize,
    /// Indexed files with calls that no index document covers (code behind cfg flags or features
    /// the indexer skipped); their calls keep name matching.
    pub uncovered_files: usize,
    pub uncovered_calls: usize,
}

/// Attach to every call of `index` what the SCIP `docs` say it refers to. A call is matched to the
/// identifier occurrence with the same name on its line, or up to `CHAIN_LINES` below it; on equal
/// lines the earliest unmatched call wins. Re-running replaces the earlier resolutions of the files
/// the index covers; other files keep theirs.
pub fn apply(index: &mut Index, docs: &[Document]) -> Stats {
    let mut definitions: HashMap<&str, (&str, u32)> = HashMap::new();
    // A path listed twice is one file: gather its occurrences, in source order (SCIP promises none).
    let mut by_path: BTreeMap<&str, Vec<&Occurrence>> = BTreeMap::new();
    for doc in docs {
        for o in &doc.occurrences {
            if o.definition {
                definitions
                    .entry(o.symbol)
                    .or_insert((doc.path, o.line.saturating_add(1)));
            }
        }
        by_path
            .entry(doc.path)
            .or_default()
            .extend(&doc.occurrences);
    }
    let mut stats = Stats::default();
    let mut covered = std::collections::HashSet::new();
    for (path, mut occurrences) in by_path {
        occurrences.sort_by_key(|o| (o.line, o.col, o.symbol, o.definition));
        occurrences.dedup_by_key(|o| (o.line, o.col, o.symbol, o.definition));
        let Some(file) = index.files.get_mut(path) else {
            stats.unknown_documents += 1;
            continue;
        };
        if !file.is_doc() {
            stats.calls += file.calls.len();
        }
        let mut by_line: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        for (i, call) in file.calls.iter().enumerate() {
            by_line.entry(call.line).or_default().push(i);
        }
        let mut taken = vec![false; file.calls.len()];
        let mut resolved = Vec::new();
        for o in occurrences.into_iter().filter(|o| !o.definition) {
            let Some(name) = last_name(o.symbol) else {
                continue;
            };
            let line = o.line.saturating_add(1);
            let found = by_line
                .range(line.saturating_sub(CHAIN_LINES)..=line)
                .rev()
                .flat_map(|(_, calls)| calls)
                .find(|&&i| !taken[i] && file.calls[i].callee == name);
            let Some(&i) = found else { continue };
            let Ok(call) = u32::try_from(i) else { continue };
            taken[i] = true;
            let target = match definitions.get(o.symbol) {
                Some(&(path, line)) => Resolution::Def {
                    path: path.to_string(),
                    line,
                },
                None => {
                    stats.external += 1;
                    Resolution::External
                }
            };
            resolved.push(ResolvedCall { call, target });
        }
        resolved.sort_by_key(|r| r.call);
        stats.resolved += resolved.len();
        file.resolved = resolved;
        covered.insert(path);
    }
    for (path, file) in &mut index.files {
        if !covered.contains(path.as_str()) {
            // An earlier run may have covered it; this index says nothing, so forget those answers.
            file.resolved.clear();
            // SCIP never covers docs; their mentions are not calls to resolve.
            if !file.is_doc() && !file.calls.is_empty() {
                stats.uncovered_files += 1;
                stats.uncovered_calls += file.calls.len();
            }
        }
    }
    stats
}

/// The identifier a symbol names, e.g. `rust-analyzer cargo tokio 1.0 sync/Notify#notify_waiters().`
/// -> `notify_waiters`. `None` for local variables and symbols we cannot read.
/// Grammar: sourcegraph/scip `scip.proto`, "Symbol": `<scheme> <manager> <package> <version>
/// <descriptor>+`, a space inside a field is written as two.
fn last_name(symbol: &str) -> Option<String> {
    if symbol.starts_with("local ") {
        return None;
    }
    let mut rest = symbol;
    for _ in 0..4 {
        rest = &rest[single_space(rest)? + 1..];
    }
    let mut chars = rest.char_indices().peekable();
    let mut last = None;
    while let Some(&(_, c)) = chars.peek() {
        match c {
            // `[T]` type parameter, `(x)` parameter: not what the symbol is called. Their names
            // may be in backticks too (``impl#[`[T]`]len().``), so read them like any name.
            '[' | '(' => {
                let close = if c == '[' { ']' } else { ')' };
                chars.next();
                read_name(&mut chars)?;
                (chars.next()?.1 == close).then_some(())?;
            }
            _ => {
                let name = read_name(&mut chars)?;
                match chars.next()?.1 {
                    '/' | '#' | '.' | ':' | '!' => last = Some(name),
                    // Method: `name(disambiguator).`
                    '(' => {
                        chars.by_ref().find(|&(_, d)| d == ')')?;
                        (chars.next()?.1 == '.').then_some(())?;
                        last = Some(name);
                    }
                    _ => return None,
                }
            }
        }
    }
    last.filter(|n| !n.is_empty())
}

/// Byte index of the first space that is not half of an escaped (doubled) one.
fn single_space(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b' ' {
            if bytes.get(i + 1) == Some(&b' ') {
                i += 2;
                continue;
            }
            return Some(i);
        }
        i += 1;
    }
    None
}

/// A descriptor name: plain, or in backticks with `` ` `` doubled.
fn read_name(chars: &mut std::iter::Peekable<std::str::CharIndices>) -> Option<String> {
    let mut name = String::new();
    if chars.peek()?.1 == '`' {
        chars.next();
        loop {
            match chars.next()?.1 {
                '`' if chars.peek().map(|c| c.1) == Some('`') => {
                    chars.next();
                    name.push('`');
                }
                '`' => return Some(name),
                c => name.push(c),
            }
        }
    }
    while let Some(&(_, c)) = chars.peek() {
        if c.is_alphanumeric() || matches!(c, '_' | '+' | '-' | '$') {
            name.push(c);
            chars.next();
        } else {
            break;
        }
    }
    Some(name)
}

/// The documents of an index. Fails on truncated or malformed protobuf.
pub fn parse(data: &[u8]) -> Result<Vec<Document<'_>>> {
    let mut docs = Vec::new();
    let mut reader = Reader::new(data);
    while let Some((number, value)) = reader.next_field()? {
        if number == 2 {
            docs.push(document(value.bytes()?)?);
        }
    }
    Ok(docs)
}

fn document(data: &[u8]) -> Result<Document<'_>> {
    let mut doc = Document {
        path: "",
        occurrences: Vec::new(),
    };
    let mut reader = Reader::new(data);
    while let Some((number, value)) = reader.next_field()? {
        match number {
            1 => doc.path = value.text(),
            2 => {
                if let Some(occurrence) = occurrence(value.bytes()?)? {
                    doc.occurrences.push(occurrence);
                }
            }
            _ => {}
        }
    }
    Ok(doc)
}

/// `None` for an occurrence without a symbol or a position (nothing to link).
fn occurrence(data: &[u8]) -> Result<Option<Occurrence<'_>>> {
    let (mut start, mut symbol, mut roles) = (None, "", 0);
    let mut reader = Reader::new(data);
    while let Some((number, value)) = reader.next_field()? {
        match number {
            // Deprecated packed `range`: [line, startCol, endCol] or [line, startCol, endLine, endCol].
            1 => start = start.or(packed_start(&value)?),
            2 => symbol = value.text(),
            3 => roles = value.number(),
            // `single_line_range` / `multi_line_range`: line and column are fields 1 and 2 of both.
            8 | 9 => start = start.or(message_start(value.bytes()?)?),
            _ => {}
        }
    }
    let Some((line, col)) = start else {
        return Ok(None);
    };
    Ok((!symbol.is_empty()).then_some(Occurrence {
        line: u32::try_from(line)?,
        col: u32::try_from(col)?,
        symbol,
        definition: roles & DEFINITION != 0,
    }))
}

/// (line, column) from a packed `range`; a lone varint is a line.
fn packed_start(value: &Value) -> Result<Option<(u64, u64)>> {
    match value {
        Value::Varint(n) => Ok(Some((*n, 0))),
        Value::Bytes([]) => Ok(None),
        Value::Bytes(bytes) => {
            let mut reader = Reader::new(bytes);
            let line = reader.varint()?;
            let col = if reader.pos < bytes.len() {
                reader.varint()?
            } else {
                0
            };
            Ok(Some((line, col)))
        }
    }
}

/// (line, column) from a nested range message: fields 1 and 2.
fn message_start(data: &[u8]) -> Result<Option<(u64, u64)>> {
    let (mut line, mut col) = (None, 0);
    let mut reader = Reader::new(data);
    while let Some((number, value)) = reader.next_field()? {
        match (number, value) {
            (1, Value::Varint(n)) => line = Some(n),
            (2, Value::Varint(n)) => col = n,
            _ => {}
        }
    }
    Ok(line.map(|line| (line, col)))
}

enum Value<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
}

impl<'a> Value<'a> {
    fn bytes(&self) -> Result<&'a [u8]> {
        match self {
            Value::Bytes(b) => Ok(b),
            Value::Varint(_) => bail!("expected a length-delimited field"),
        }
    }

    /// UTF-8 text, or "" when the field is not text.
    fn text(&self) -> &'a str {
        match self {
            Value::Bytes(b) => std::str::from_utf8(b).unwrap_or(""),
            Value::Varint(_) => "",
        }
    }

    fn number(&self) -> u64 {
        match self {
            Value::Varint(n) => *n,
            Value::Bytes(_) => 0,
        }
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    fn varint(&mut self) -> Result<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let Some(&byte) = self.data.get(self.pos) else {
                bail!("truncated varint");
            };
            self.pos += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        bail!("varint longer than 10 bytes")
    }

    fn take(&mut self, len: u64) -> Result<&'a [u8]> {
        let len = usize::try_from(len)?;
        let end = self.pos.checked_add(len).filter(|&e| e <= self.data.len());
        let Some(end) = end else {
            bail!("field longer than the data left");
        };
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// Next (field number, value); fixed-width fields are skipped, `None` at the end.
    fn next_field(&mut self) -> Result<Option<(u32, Value<'a>)>> {
        loop {
            if self.pos >= self.data.len() {
                return Ok(None);
            }
            let key = self.varint()?;
            let number = u32::try_from(key >> 3)?;
            match key & 7 {
                0 => return Ok(Some((number, Value::Varint(self.varint()?)))),
                2 => {
                    let len = self.varint()?;
                    return Ok(Some((number, Value::Bytes(self.take(len)?))));
                }
                1 => _ = self.take(8)?,
                5 => _ = self.take(4)?,
                wire => bail!("unsupported protobuf wire type {wire}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    fn field_bytes(number: u32, data: &[u8]) -> Vec<u8> {
        let mut out = varint(u64::from(number) << 3 | 2);
        out.extend(varint(data.len() as u64));
        out.extend(data);
        out
    }

    fn field_varint(number: u32, v: u64) -> Vec<u8> {
        let mut out = varint(u64::from(number) << 3);
        out.extend(varint(v));
        out
    }

    fn packed(numbers: &[u64]) -> Vec<u8> {
        numbers.iter().flat_map(|&n| varint(n)).collect()
    }

    fn occurrence_bytes(range: Vec<u8>, symbol: &str, roles: u64) -> Vec<u8> {
        [
            range,
            field_bytes(2, symbol.as_bytes()),
            field_varint(3, roles),
        ]
        .concat()
    }

    fn index_of(path: &str, occurrences: &[Vec<u8>]) -> Vec<u8> {
        let mut doc = field_bytes(1, path.as_bytes());
        for o in occurrences {
            doc.extend(field_bytes(2, o));
        }
        // Metadata (1) and external_symbols (3) must be skipped without trouble.
        [
            field_bytes(1, b"meta"),
            field_bytes(2, &doc),
            field_bytes(3, b"ext"),
        ]
        .concat()
    }

    #[test]
    fn reads_paths_symbols_lines_and_roles() {
        let def = occurrence_bytes(field_bytes(1, &packed(&[4, 3, 9])), "s f().", 1);
        let call = occurrence_bytes(field_bytes(1, &packed(&[9, 1, 10, 2])), "s f().", 0);
        let data = index_of("src/a.rs", &[def, call]);
        let docs = parse(&data).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].path, "src/a.rs");
        assert_eq!(
            docs[0].occurrences,
            vec![
                Occurrence {
                    line: 4,
                    col: 3,
                    symbol: "s f().",
                    definition: true
                },
                Occurrence {
                    line: 9,
                    col: 1,
                    symbol: "s f().",
                    definition: false
                },
            ]
        );
    }

    #[test]
    fn packed_range_starting_at_line_8_is_not_mistaken_for_a_message() {
        let data = index_of(
            "a.rs",
            &[occurrence_bytes(
                field_bytes(1, &packed(&[8, 5, 9])),
                "x",
                0,
            )],
        );
        assert_eq!(parse(&data).unwrap()[0].occurrences[0].line, 8);
    }

    #[test]
    fn reads_typed_ranges() {
        let single = field_bytes(
            8,
            &[field_varint(1, 7), field_varint(2, 1), field_varint(3, 4)].concat(),
        );
        let multi = field_bytes(
            9,
            &[
                field_varint(1, 12),
                field_varint(2, 0),
                field_varint(3, 13),
                field_varint(4, 2),
            ]
            .concat(),
        );
        let data = index_of(
            "a.ts",
            &[
                occurrence_bytes(single, "x", 0),
                occurrence_bytes(multi, "y", 0),
            ],
        );
        let lines: Vec<_> = parse(&data).unwrap()[0]
            .occurrences
            .iter()
            .map(|o| o.line)
            .collect();
        assert_eq!(lines, vec![7, 12]);
    }

    #[test]
    fn occurrences_without_symbol_or_position_are_dropped() {
        let no_symbol = occurrence_bytes(field_bytes(1, &packed(&[1, 0, 1])), "", 0);
        let no_range = occurrence_bytes(Vec::new(), "x", 0);
        let data = index_of("a.rs", &[no_symbol, no_range]);
        assert!(parse(&data).unwrap()[0].occurrences.is_empty());
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        let good = index_of(
            "a.rs",
            &[occurrence_bytes(
                field_bytes(1, &packed(&[1, 0, 1])),
                "x",
                0,
            )],
        );
        for cut in 1..good.len() {
            let _ = parse(&good[..cut]); // any prefix: error or a shorter parse, never a panic
        }
        assert!(parse(&[0x12, 0x7f, 1, 2]).is_err(), "length past the end");
        assert!(parse(&[0xff; 11]).is_err(), "varint too long");
        assert!(parse(&[0x0b]).is_err(), "group wire type");
        assert!(parse(&[]).unwrap().is_empty());
        assert!(parse(&[0x12]).is_err(), "key without length");
    }

    #[test]
    fn backticks_inside_type_parameters_and_parameters() {
        let name = |s: &str| last_name(s);
        assert_eq!(
            name("rust-analyzer cargo core 1.0 slice/impl#[`[T]`]len()."),
            Some("len".into())
        );
        assert_eq!(
            name("rust-analyzer cargo p 1 m/impl#[`Foo<T>`]new()."),
            Some("new".into())
        );
        assert_eq!(
            name("rust-analyzer cargo p 1 m/f().(`a)b`)"),
            Some("f".into())
        );
        assert_eq!(name("rust-analyzer cargo p 1 m/f().(x"), None);
    }

    #[test]
    fn symbol_names() {
        let name = |s: &str| last_name(s);
        assert_eq!(
            name("rust-analyzer cargo tokio 1.0.0 sync/Notify#notify_waiters()."),
            Some("notify_waiters".into())
        );
        assert_eq!(
            name("rust-analyzer cargo core 1.0.0 option/Option#new()."),
            Some("new".into())
        );
        assert_eq!(
            name("scip-typescript npm pizzeria 1.0.0 src/`price.ts`/formatPrice()."),
            Some("formatPrice".into())
        );
        assert_eq!(
            name("scip-typescript npm my  pkg 1.0.0 src/Cart#"),
            Some("Cart".into())
        );
        assert_eq!(name("rust-analyzer cargo t 1 m/f().(x)"), Some("f".into()));
        assert_eq!(name("rust-analyzer cargo t 1 m/f<T>#"), None);
        assert_eq!(name("local 12"), None);
        assert_eq!(name("garbage"), None);
        assert_eq!(name("a b c d `we``ird`."), Some("we`ird".into()));
    }

    fn rust_index(src: &str) -> Index {
        let facts = crate::lang::extract(crate::lang::Lang::Rust, src).unwrap();
        let mut index = Index::default();
        index.files.insert(
            "a.rs".into(),
            crate::model::FileEntry {
                path: "a.rs".into(),
                calls: facts.calls,
                ..Default::default()
            },
        );
        index
    }

    fn occ<'a>(line: u32, symbol: &'a str, definition: bool) -> Occurrence<'a> {
        Occurrence {
            line,
            col: 0,
            symbol,
            definition,
        }
    }

    fn occ_at<'a>(line: u32, col: u32, symbol: &'a str) -> Occurrence<'a> {
        Occurrence {
            col,
            ..occ(line, symbol, false)
        }
    }

    #[test]
    fn files_the_index_does_not_cover_are_counted() {
        let mut index = rust_index("fn run() { f(); }\n");
        let other = rust_index("fn x() { a(); b(); }\nfn y() {}\n")
            .files
            .remove("a.rs")
            .unwrap();
        index.files.insert(
            "loom.rs".into(),
            crate::model::FileEntry {
                path: "loom.rs".into(),
                ..other
            },
        );
        index.files.insert(
            "empty.rs".into(),
            crate::model::FileEntry {
                path: "empty.rs".into(),
                ..Default::default()
            },
        );
        let docs = [Document {
            path: "a.rs",
            occurrences: vec![occ(0, "r c p 1 a/f().", false)],
        }];
        let stats = apply(&mut index, &docs);
        assert_eq!((stats.uncovered_files, stats.uncovered_calls), (1, 2));
    }

    #[test]
    fn docs_are_never_counted_as_uncovered() {
        let mut index = rust_index("fn run() { f(); }\n");
        let doc =
            crate::lang::extract(crate::lang::Lang::Markdown, "# T\n`run()` `f()`\n").unwrap();
        index.files.insert(
            "README.md".into(),
            crate::model::FileEntry {
                path: "README.md".into(),
                calls: doc.calls,
                ..Default::default()
            },
        );
        let docs = [Document {
            path: "a.rs",
            occurrences: vec![occ(0, "r c p 1 a/f().", false)],
        }];
        let stats = apply(&mut index, &docs);
        assert_eq!((stats.uncovered_files, stats.uncovered_calls), (0, 0));
    }

    #[test]
    fn a_rerun_that_no_longer_covers_a_file_forgets_its_old_answers() {
        let mut index = rust_index("fn run() { f(); }\n");
        let docs = [Document {
            path: "a.rs",
            occurrences: vec![occ(0, "r c p 1 a/f().", false)],
        }];
        assert_eq!(apply(&mut index, &docs).resolved, 1);
        let stats = apply(&mut index, &[]);
        assert!(index.files["a.rs"].resolved.is_empty());
        assert_eq!((stats.uncovered_files, stats.uncovered_calls), (1, 1));
    }

    #[test]
    fn a_repeated_occurrence_matches_only_one_call() {
        let mut index = rust_index("fn run() { f(); f(); }\n");
        let same = || Document {
            path: "a.rs",
            occurrences: vec![occ_at(0, 11, "r c core 1 m/f().")],
        };
        let stats = apply(&mut index, &[same(), same()]);
        assert_eq!(stats.resolved, 1);
    }

    #[test]
    fn duplicates_are_removed_even_when_a_definition_sits_between_them() {
        let mut index = rust_index("fn run() { f(); f(); }\n");
        let docs = [Document {
            path: "a.rs",
            occurrences: vec![
                occ_at(0, 11, "r c core 1 m/f()."),
                Occurrence {
                    definition: true,
                    ..occ_at(0, 11, "r c core 1 m/f().")
                },
                occ_at(0, 11, "r c core 1 m/f()."),
            ],
        }];
        assert_eq!(apply(&mut index, &docs).resolved, 1);
    }

    #[test]
    fn hostile_lines_do_not_overflow() {
        let mut index = rust_index("fn run() { f(); }\n");
        let docs = [Document {
            path: "a.rs",
            occurrences: vec![
                occ(u32::MAX, "r c p 1 a/f().", true),
                occ(u32::MAX, "r c p 1 a/f().", false),
            ],
        }];
        apply(&mut index, &docs); // must not panic; nothing near line u32::MAX matches
        assert!(index.files["a.rs"].resolved.is_empty());
    }

    #[test]
    fn occurrences_are_matched_in_source_order_and_duplicate_documents_merge() {
        // `f(); f();` on one line; the occurrences arrive right-to-left and split over two documents.
        let mut index = rust_index("fn run() { f(); f(); }\n");
        let docs = [
            Document {
                path: "a.rs",
                occurrences: vec![occ_at(0, 17, "r c core 1 m/f().")],
            },
            Document {
                path: "a.rs",
                occurrences: vec![
                    occ_at(0, 11, "r c p 1 m/f()."),
                    occ(1, "r c p 1 m/f().", true),
                ],
            },
        ];
        let stats = apply(&mut index, &docs);
        assert_eq!(
            stats,
            Stats {
                calls: 2,
                resolved: 2,
                external: 1,
                ..Stats::default()
            }
        );
        let file = &index.files["a.rs"];
        assert_eq!(
            file.resolution(0),
            Some(&Resolution::Def {
                path: "a.rs".into(),
                line: 2
            })
        );
        assert_eq!(file.resolution(1), Some(&Resolution::External));
    }

    #[test]
    fn calls_are_resolved_to_project_definitions_or_external() {
        // line 1: fn run() { new(); helper(); }   line 2: fn helper() {}
        let mut index = rust_index("fn run() { new(); helper(); }\nfn helper() {}\n");
        let docs = [Document {
            path: "a.rs",
            occurrences: vec![
                occ(0, "r c p 1 a/run().", true),
                occ(0, "r c core 1 option/Option#new().", false),
                occ(0, "r c p 1 a/helper().", false),
                occ(1, "r c p 1 a/helper().", true),
                occ(0, "local 3", false),
            ],
        }];
        let stats = apply(&mut index, &docs);
        assert_eq!(
            stats,
            Stats {
                calls: 2,
                resolved: 2,
                external: 1,
                ..Stats::default()
            }
        );
        let file = &index.files["a.rs"];
        assert_eq!(file.resolution(0), Some(&Resolution::External));
        assert_eq!(
            file.resolution(1),
            Some(&Resolution::Def {
                path: "a.rs".into(),
                line: 2
            })
        );
    }

    #[test]
    fn a_chain_spread_over_lines_and_same_named_calls_match_in_order() {
        // calls: a.foo() starts on line 1, its `.foo` identifier is on line 3.
        let mut index =
            rust_index("fn run() {\n    a\n        .x()\n        .foo();\n    foo();\n}\n");
        let docs = [Document {
            path: "a.rs",
            occurrences: vec![
                occ(3, "r c p 1 m/T#foo().", false),
                occ(4, "r c p 1 m/foo().", false),
                occ(2, "r c p 1 m/T#x().", false),
            ],
        }];
        let stats = apply(&mut index, &docs);
        assert_eq!(stats.resolved, 3);
        assert_eq!(stats.external, 3);
    }

    #[test]
    fn unknown_documents_are_counted_and_reapplying_replaces() {
        let mut index = rust_index("fn run() { f(); }\n");
        let docs = [
            Document {
                path: "elsewhere.rs",
                occurrences: vec![],
            },
            Document {
                path: "a.rs",
                occurrences: vec![occ(0, "r c p 1 a/f().", false)],
            },
        ];
        assert_eq!(apply(&mut index, &docs).unknown_documents, 1);
        let again = [Document {
            path: "a.rs",
            occurrences: vec![],
        }];
        apply(&mut index, &again);
        assert!(index.files["a.rs"].resolved.is_empty());
    }
}
