//! The readers that are alive, and the files they hold.
//!
//! **A compaction removes the files the old manifest named**, because the new
//! file holds every live row and the old ones hold nothing an application can
//! see. **But a reader that pinned the old files still has to read them**, so
//! the files cannot go while it is there.
//!
//! **So the database counts its readers.** A snapshot registers the files it
//! pinned when it is taken and gives them back when it is dropped, and a
//! compaction asks whether any live reader holds one of the files it wants to
//! remove. **The answer is exact rather than a count of readers**, because the
//! question is about the files and not about the readers: a snapshot taken
//! after the compaction holds the new file and not the old ones, so it does
//! not stand in the way of removing them.
//!
//! **And a file that is not removed is not lost.** It stays in the table's
//! directory, outside the manifest, and it is removed at the next compaction
//! that no reader stands in the way of. The manifest is the table's truth, so
//! a file outside it is a file no reader will ever open.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// The readers that are alive.
#[derive(Debug, Default)]
pub(crate) struct Readers {
    inner: Mutex<Inner>,
}

/// The readers' own bookkeeping.
#[derive(Debug, Default)]
struct Inner {
    /// The id the next reader gets.
    next_id: u64,
    /// The files each live reader holds.
    live: BTreeMap<u64, Vec<PathBuf>>,
}

impl Readers {
    /// Registers a reader, and returns the id it gives back.
    pub(crate) fn register(&self, files: Vec<PathBuf>) -> u64 {
        let mut inner = self.lock();
        let id = inner.next_id;
        inner.next_id += 1;
        inner.live.insert(id, files);
        id
    }

    /// Gives a reader's files back.
    pub(crate) fn release(&self, id: u64) {
        let mut inner = self.lock();
        inner.live.remove(&id);
    }

    /// Whether a live reader holds one of these files.
    pub(crate) fn blocks(&self, files: &[PathBuf]) -> bool {
        let inner = self.lock();
        inner.live.values().any(|held| held.iter().any(|file| files.contains(file)))
    }

    /// The readers' lock, and the poisoning is not the readers' fault.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
