use super::{WorkspaceError as E, WorkspaceResult as R};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    path::{Component, Path, PathBuf},
};

pub type FileTree = BTreeMap<String, Vec<u8>>;
pub const DIRECTORIES: [&str; 9] = [
    "domains",
    "topics",
    "concepts",
    "units",
    "questions",
    "resources",
    "records",
    "sessions",
    "assets",
];
pub fn managed(path: &str) -> bool {
    if path
        .rsplit('/')
        .next()
        .is_some_and(|name| name.starts_with(".lumen-") && name.ends_with(".tmp"))
    {
        return false;
    }
    matches!(path, "repository.yaml" | ".gitignore" | ".gitattributes")
        || DIRECTORIES
            .iter()
            .any(|d| path.starts_with(&format!("{d}/")))
            && !path.ends_with(".open")
}
pub fn safe_path(root: &Path, relative: &str) -> R<PathBuf> {
    let p = Path::new(relative);
    if relative.is_empty() || p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(E::InvalidPath(relative.into()));
    }
    let mut out = root.to_path_buf();
    for c in p.components() {
        out.push(c);
        if fs::symlink_metadata(&out)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(E::InvalidPath(relative.into()));
        }
    }
    Ok(out)
}
pub fn scan(root: &Path) -> R<FileTree> {
    let mut out = BTreeMap::new();
    for name in ["repository.yaml", ".gitignore", ".gitattributes"]
        .into_iter()
        .chain(DIRECTORIES)
    {
        let p = safe_path(root, name)?;
        if p.exists() {
            walk(root, &p, &mut out)?
        }
    }
    Ok(out)
}
fn walk(root: &Path, path: &Path, out: &mut FileTree) -> R<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(E::InvalidPath(path.display().to_string()));
    }
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            walk(root, &entry?.path(), out)?
        }
    } else if meta.is_file() {
        let rel = path
            .strip_prefix(root)
            .map_err(|_| E::InvalidPath(path.display().to_string()))?
            .to_str()
            .ok_or_else(|| E::InvalidPath("non-UTF8 path".into()))?
            .replace('\\', "/");
        if managed(&rel) {
            out.insert(rel, fs::read(path)?);
        }
    }
    Ok(())
}
pub fn fingerprint(tree: &FileTree) -> String {
    let mut hash = blake3::Hasher::new();
    for (path, bytes) in tree {
        hash.update(&(path.len() as u64).to_le_bytes());
        hash.update(path.as_bytes());
        hash.update(&(bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    hash.finalize().to_hex().to_string()
}
pub fn atomic_write(root: &Path, path: &str, bytes: &[u8]) -> R<()> {
    let dest = safe_path(root, path)?;
    let parent = dest.parent().ok_or_else(|| E::InvalidPath(path.into()))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".lumen-{}.tmp", uuid::Uuid::now_v7()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp, &dest)?;
    sync_directory(parent)?;
    Ok(())
}
fn sync_directory(path: &Path) -> R<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
#[derive(Serialize, Deserialize)]
struct Transaction {
    entries: Vec<Entry>,
}
#[derive(Serialize, Deserialize)]
struct Entry {
    path: String,
    before: Option<String>,
    after: Option<Vec<u8>>,
}
pub fn apply(root: &Path, changes: BTreeMap<String, Option<Vec<u8>>>) -> R<Vec<String>> {
    apply_checked(root, changes, BTreeMap::new())
}
pub fn apply_checked(
    root: &Path,
    changes: BTreeMap<String, Option<Vec<u8>>>,
    expected: BTreeMap<String, Option<String>>,
) -> R<Vec<String>> {
    let mut entries = vec![];
    for (path, after) in changes {
        let p = safe_path(root, &path)?;
        let before = if p.exists() {
            Some(blake3::hash(&fs::read(p)?).to_hex().to_string())
        } else {
            None
        };
        if let Some(expected) = expected.get(&path) {
            if *expected != before {
                return Err(E::RevisionConflict(path));
            }
        }
        entries.push(Entry {
            path,
            before,
            after,
        });
    }
    let tx = Transaction { entries };
    let name = format!(".lumen/transactions/{}.json", uuid::Uuid::now_v7());
    atomic_write(root, &name, &serde_json::to_vec(&tx)?)?;
    finish(root, &tx)?;
    fs::remove_file(safe_path(root, &name)?)?;
    sync_directory(&root.join(".lumen/transactions"))?;
    Ok(tx.entries.into_iter().map(|e| e.path).collect())
}

fn finish(root: &Path, tx: &Transaction) -> R<()> {
    for e in &tx.entries {
        let p = safe_path(root, &e.path)?;
        let current = if p.exists() {
            Some(blake3::hash(&fs::read(&p)?).to_hex().to_string())
        } else {
            None
        };
        let target = e
            .after
            .as_ref()
            .map(|b| blake3::hash(b).to_hex().to_string());
        if current == target {
            continue;
        }
        if current != e.before {
            return Err(E::RecoveryRequired(format!(
                "external change at {}",
                e.path
            )));
        }
        match &e.after {
            Some(bytes) => atomic_write(root, &e.path, bytes)?,
            None => {
                if p.exists() {
                    fs::remove_file(&p)?;
                    if let Some(parent) = p.parent() {
                        sync_directory(parent)?
                    }
                }
            }
        }
    }
    Ok(())
}
pub fn recover(root: &Path) -> R<usize> {
    let dir = safe_path(root, ".lumen/transactions")?;
    fs::create_dir_all(&dir)?;
    let mut paths = fs::read_dir(&dir)?
        .map(|e| e.map(|x| x.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    let mut count = 0;
    for p in paths {
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if fs::symlink_metadata(&p)?.file_type().is_symlink() {
            return Err(E::InvalidPath(p.display().to_string()));
        }
        let tx: Transaction = serde_json::from_slice(&fs::read(&p)?)?;
        finish(root, &tx)?;
        fs::remove_file(p)?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_transaction_does_not_overwrite_an_external_edit() {
        let root = tempfile::tempdir().unwrap();
        atomic_write(root.path(), "concepts/test.md", b"external edit").unwrap();
        let result = apply_checked(
            root.path(),
            BTreeMap::from([("concepts/test.md".into(), Some(b"replacement".to_vec()))]),
            BTreeMap::from([(
                "concepts/test.md".into(),
                Some(blake3::hash(b"old content").to_hex().to_string()),
            )]),
        );
        assert!(matches!(result, Err(E::RevisionConflict(_))));
        assert_eq!(
            fs::read(root.path().join("concepts/test.md")).unwrap(),
            b"external edit"
        );
    }
}
