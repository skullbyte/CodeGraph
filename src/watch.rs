//! Change detection for `codegraph watch`: compare snapshots of file modification times.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::SystemTime;

use anyhow::Result;

use crate::walk;

/// Path -> (modified time, size) for every indexable file.
pub type Snapshot = BTreeMap<String, (SystemTime, u64)>;

pub fn snapshot(root: &Path) -> Result<Snapshot> {
    let mut snap = Snapshot::new();
    for rel in walk::list_files(root)? {
        // A file can vanish between listing and stat; it simply shows up as removed next time.
        let Ok(meta) = fs::metadata(root.join(&rel)) else {
            continue;
        };
        if let Ok(modified) = meta.modified() {
            snap.insert(rel, (modified, meta.len()));
        }
    }
    // tsconfig.json isn't indexed, but its alias table is, so changes to it must be noticed too.
    if let Ok(meta) = fs::metadata(root.join("tsconfig.json"))
        && let Ok(modified) = meta.modified()
    {
        snap.insert("tsconfig.json".to_string(), (modified, meta.len()));
    }
    Ok(snap)
}

/// Paths that were added, modified or removed between two snapshots, sorted.
pub fn changed(old: &Snapshot, new: &Snapshot) -> Vec<String> {
    let touched = new
        .iter()
        .filter(|(path, stamp)| old.get(*path) != Some(stamp));
    let removed = old.keys().filter(|path| !new.contains_key(*path));
    let all: BTreeSet<&String> = touched.map(|(path, _)| path).chain(removed).collect();
    all.into_iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    #[test]
    fn changed_reports_added_modified_and_removed() {
        let t0 = SystemTime::UNIX_EPOCH;
        let t1 = t0 + Duration::from_secs(5);
        let old = Snapshot::from([
            ("keep".into(), (t0, 1)),
            ("edit".into(), (t0, 1)),
            ("gone".into(), (t0, 1)),
        ]);
        let new = Snapshot::from([
            ("keep".into(), (t0, 1)),
            ("edit".into(), (t1, 1)),
            ("new".into(), (t0, 1)),
        ]);
        assert_eq!(changed(&old, &new), vec!["edit", "gone", "new"]);
        assert!(changed(&old, &old).is_empty());
    }

    #[test]
    fn snapshot_sees_only_indexable_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "fn a() {}").unwrap();
        fs::write(dir.path().join("notes.md"), "x").unwrap();
        let snap = snapshot(dir.path()).unwrap();
        assert_eq!(snap.keys().collect::<Vec<_>>(), vec!["a.rs"]);
        assert_eq!(snap["a.rs"].1, 9);
    }

    #[test]
    fn snapshot_includes_tsconfig_so_alias_edits_are_noticed() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("tsconfig.json"), "{}").unwrap();
        assert!(snapshot(dir.path()).unwrap().contains_key("tsconfig.json"));
    }
}
