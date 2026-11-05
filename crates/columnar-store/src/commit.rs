//! The commit: the five steps, in the order a commit has to take them.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use crate::error::Result;
use crate::manifest::Manifest;

/// The manifest's name, inside a table's directory.
pub const MANIFEST_FILE: &str = "manifest.json";

/// The manifest's name before the rename.
pub const MANIFEST_TMP: &str = "manifest.json.tmp";

/// The commit.
///
/// The data file is made durable **first**, then the manifest that names it is
/// written, made durable, and `rename`d over the old one, and the directory is
/// made durable **last**, so that the rename itself survives.
///
/// **A reader that sees the new manifest sees the new data; a reader that sees
/// the old one sees the old data.** There is no third answer, and there is no
/// write-ahead log: a commit is a rename, and the rename is the answer.
///
/// # Errors
///
/// Returns [`crate::error::Error::Io`] if the file system refuses, and
/// [`crate::error::Error::Json`] if the manifest cannot be written.
pub fn commit(dir: &Path, manifest: &Manifest) -> Result<()> {
    if let Some(file) = manifest.files.last() {
        fsync_file(&dir.join(&file.name))?;
    }

    let tmp = dir.join(MANIFEST_TMP);
    let mut handle = File::create(&tmp)?;
    handle.write_all(manifest.to_json()?.as_bytes())?;
    handle.flush()?;
    drop(handle);
    fsync_file(&tmp)?;

    fs::rename(&tmp, dir.join(MANIFEST_FILE))?;
    fsync_dir(dir)?;
    Ok(())
}

/// Makes a file durable.
fn fsync_file(path: &Path) -> Result<()> {
    let file = File::open(path)?;
    file.sync_all()?;
    Ok(())
}

/// Makes a directory's entries durable.
fn fsync_dir(path: &Path) -> Result<()> {
    let dir = File::open(path)?;
    dir.sync_all()?;
    Ok(())
}
