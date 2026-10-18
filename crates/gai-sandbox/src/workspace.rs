//! What a workspace holds, so a call can tell which files it changed.

use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

/// Every regular file under a directory (symlinks are skipped), by
/// `/`-separated relative path, with a hash of its contents.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub files: BTreeMap<String, FileInfo>,
    /// Files left out because they would pass the size budget.
    pub skipped: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileInfo {
    pub size: u64,
    pub hash: [u8; 32],
}

pub(crate) fn hash(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

impl Snapshot {
    /// Walk `root`, taking files (smallest first) until `budget` bytes.
    pub fn take(root: &Path, budget: u64) -> io::Result<Snapshot> {
        let mut found = Vec::new();
        if root.exists() {
            walk(root, root, &mut found)?;
        }
        found.sort_by_key(|(_, size)| *size);
        let mut snap = Snapshot::default();
        let mut used = 0u64;
        for (rel, size) in found {
            if used + size > budget {
                snap.skipped.push(rel);
                continue;
            }
            let data = std::fs::read(root.join(&rel))?;
            used += size;
            snap.files.insert(
                rel,
                FileInfo {
                    size,
                    hash: hash(&data),
                },
            );
        }
        snap.skipped.sort();
        Ok(snap)
    }

    /// Files in `after` that are new or different, and files of `self` that
    /// `after` no longer has.
    pub fn diff(&self, after: &Snapshot) -> (Vec<String>, Vec<String>) {
        let changed = after
            .files
            .iter()
            .filter(|(p, f)| self.files.get(*p).is_none_or(|b| b.hash != f.hash))
            .map(|(p, _)| p.clone())
            .collect();
        let deleted = self
            .files
            .keys()
            .filter(|p| !after.files.contains_key(*p))
            .cloned()
            .collect();
        (changed, deleted)
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, u64)>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let path = entry.path();
        if ty.is_dir() {
            walk(root, &path, out)?;
        } else if ty.is_file() {
            let rel = path
                .strip_prefix(root)
                .map_err(io::Error::other)?
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel, entry.metadata()?.len()));
        }
    }
    Ok(())
}

/// `rel` under `root`, or `None` if it is absolute or climbs out.
pub(crate) fn safe_join(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut out = root.to_path_buf();
    let mut depth = 0usize;
    for c in Path::new(rel).components() {
        match c {
            Component::Normal(p) => {
                out.push(p);
                depth += 1;
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    (depth > 0).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_finds_new_changed_and_deleted_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("keep.txt"), "same").unwrap();
        std::fs::write(root.join("a/b/edit.txt"), "old").unwrap();
        std::fs::write(root.join("gone.txt"), "x").unwrap();
        let before = Snapshot::take(root, u64::MAX).unwrap();
        assert_eq!(before.files.len(), 3);

        std::fs::write(root.join("a/b/edit.txt"), "new").unwrap();
        std::fs::remove_file(root.join("gone.txt")).unwrap();
        std::fs::write(root.join("made.csv"), "1,2").unwrap();
        let after = Snapshot::take(root, u64::MAX).unwrap();
        let (changed, deleted) = before.diff(&after);
        assert_eq!(changed, ["a/b/edit.txt", "made.csv"]);
        assert_eq!(deleted, ["gone.txt"]);
    }

    #[test]
    fn budget_skips_the_largest_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("small"), "12").unwrap();
        std::fs::write(dir.path().join("big"), "1234567890").unwrap();
        let s = Snapshot::take(dir.path(), 5).unwrap();
        assert!(s.files.contains_key("small"));
        assert_eq!(s.skipped, ["big"]);
    }

    #[test]
    fn safe_join_refuses_escapes() {
        let root = Path::new("/w");
        assert_eq!(
            safe_join(root, "a/b.txt"),
            Some(PathBuf::from("/w/a/b.txt"))
        );
        assert_eq!(safe_join(root, "../x"), None);
        assert_eq!(safe_join(root, "/etc/passwd"), None);
        assert_eq!(safe_join(root, ""), None);
    }
}
