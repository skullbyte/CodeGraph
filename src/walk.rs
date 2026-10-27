//! Finds the source files to index (respects .gitignore).

use std::path::Path;

use anyhow::Result;
use ignore::WalkBuilder;
use ignore::gitignore::Gitignore;

use crate::lang::Lang;

const SKIP_DIRS: [&str; 5] = ["node_modules", "target", "Library", "build", "dist"];

/// Supported source files under `root`, as sorted `/`-separated paths relative to `root`.
pub fn list_files(root: &Path) -> Result<Vec<String>> {
    let walker = WalkBuilder::new(root)
        .require_git(false)
        .filter_entry(|e| !e.file_type().is_some_and(|t| t.is_dir()) || !is_skipped(e.file_name()))
        .build();
    let mut files = Vec::new();
    for entry in walker {
        // An unreadable folder shouldn't abort the whole index.
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        if Lang::from_path(&rel).is_some() {
            files.push(rel);
        }
    }
    files.sort();
    Ok(files)
}

/// Would `list_files` include `rel`? Used by `update` so a single-file update follows the same rules
/// (supported language, no hidden or skipped folders, root `.gitignore`).
pub fn is_indexable(root: &Path, rel: &str) -> bool {
    if Lang::from_path(rel).is_none() {
        return false;
    }
    if rel
        .split('/')
        .any(|c| c.starts_with('.') || SKIP_DIRS.contains(&c))
    {
        return false;
    }
    let (gitignore, _) = Gitignore::new(root.join(".gitignore"));
    !gitignore
        .matched_path_or_any_parents(rel, false)
        .is_ignore()
}

fn is_skipped(name: &std::ffi::OsStr) -> bool {
    SKIP_DIRS.iter().any(|d| name == *d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(root: &Path, rel: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    #[test]
    fn lists_only_wanted_sources() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for f in [
            "src/main.rs",
            "web/app.tsx",
            "web/types.d.ts",
            "web/notes.md",
            "node_modules/x/index.ts",
            "target/debug/gen.rs",
            "dist/out.ts",
            "ignored/skip.rs",
            ".claude/hook.rs",
        ] {
            touch(root, f);
        }
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        assert_eq!(
            list_files(root).unwrap(),
            vec!["src/main.rs", "web/app.tsx"]
        );
    }
}
