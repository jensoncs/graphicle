//! The files of a pinned snapshot as an [`oxc_resolver::FileSystem`], so
//! module resolution sees exactly those bytes: a path outside the snapshot,
//! `node_modules` included, does not exist. There are no symlinks.
use oxc_resolver::{FileMetadata, FileSystem, ResolveError};
use rustc_hash::{FxHashMap, FxHashSet};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[derive(Default)]
pub struct SnapshotFs {
    files: FxHashMap<PathBuf, Arc<str>>,
    dirs: FxHashSet<PathBuf>,
}

impl SnapshotFs {
    /// `files` by absolute path; every ancestor of a file is a directory.
    pub fn from_files(files: impl IntoIterator<Item = (PathBuf, Arc<str>)>) -> Self {
        let mut out = Self::default();
        for (path, text) in files {
            let path = normalize(&path);
            out.dirs
                .extend(path.ancestors().skip(1).map(Path::to_path_buf));
            out.files.insert(path, text);
        }
        out
    }
    fn missing(path: &Path) -> io::Error {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} is not in the snapshot", path.display()),
        )
    }
}

/// `.` and `..` removed lexically: the snapshot has no symlinks to follow.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

impl FileSystem for SnapshotFs {
    fn new() -> Self {
        Self::default()
    }
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.read_to_string(path).map(String::into_bytes)
    }
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.files
            .get(&normalize(path))
            .map(|text| text.to_string())
            .ok_or_else(|| Self::missing(path))
    }
    fn metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        let path = normalize(path);
        if self.files.contains_key(&path) {
            Ok(FileMetadata::new(true, false, false))
        } else if self.dirs.contains(&path) {
            Ok(FileMetadata::new(false, true, false))
        } else {
            Err(Self::missing(&path))
        }
    }
    fn symlink_metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        self.metadata(path)
    }
    fn read_link(&self, path: &Path) -> Result<PathBuf, ResolveError> {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a symlink", path.display()),
        )
        .into())
    }
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.metadata(path).map(|_| normalize(path))
    }
}

#[cfg(test)]
mod tests {
    use super::SnapshotFs;
    use crate::extractors::precise_js::resolver_with;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    fn snapshot(root: &Path, files: &[(&str, &str)]) -> SnapshotFs {
        SnapshotFs::from_files(
            files
                .iter()
                .map(|(p, t)| (root.join(p), Arc::<str>::from(*t))),
        )
    }

    #[test]
    fn resolves_only_snapshot_files_with_the_extractors_rules() {
        // A real directory, so a file on disk that is not in the snapshot
        // would be found by an OS file system.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/disk.ts"), "export const x = 1;\n").unwrap();
        let fs = snapshot(
            &root,
            &[
                (
                    "tsconfig.json",
                    "{\"compilerOptions\":{\"baseUrl\":\".\",\"paths\":{\"@lib/*\":[\"src/lib/*\"]}}}",
                ),
                ("src/main.ts", "import { price } from './lib/price.js';\n"),
                (
                    "src/lib/price.ts",
                    "export function price() { return 1; }\n",
                ),
            ],
        );
        let resolver = resolver_with(fs, Some(root.join("tsconfig.json")));
        let found = |spec: &str| -> Option<PathBuf> {
            resolver
                .resolve(root.join("src"), spec)
                .ok()
                .map(|r| r.full_path())
        };
        let price = Some(root.join("src/lib/price.ts"));
        assert_eq!(found("./lib/price.js"), price, "extension alias");
        assert_eq!(found("./lib/price"), price, "extensionless");
        assert_eq!(found("@lib/price"), price, "tsconfig paths");
        assert_eq!(found("./disk"), None, "a file outside the snapshot");
        assert_eq!(found("./missing"), None);
    }
}
