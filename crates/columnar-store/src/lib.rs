//! An append-only store for the columnar engine.
//!
//! The engine reads Parquet files. This crate **writes** them, and it keeps
//! the catalog that says which files make up a table.
//!
//! # The shape
//!
//! - A **table** is a **set of files**. A commit appends a file and never
//!   modifies an old one, so the store grows by adding to the set and never
//!   rewrites it.
//! - The **manifest** is the table's truth: its files, their rows and their
//!   bytes, and the table's totals. It is small, it is JSON, and it is the one
//!   thing a reader has to believe.
//! - The **commit** is five steps -- the data file made durable, the manifest
//!   written, made durable and renamed over the old one, and the directory
//!   made durable -- and **there is no write-ahead log**: a commit is a rename,
//!   and the rename is the answer. A reader that sees the new manifest sees the
//!   new data; a reader that sees the old one sees the old data.
//!
//! # The grid
//!
//! **The file's bytes depend on the batch size the rows arrive in.** The Parquet
//! writer splits every batch it is handed into mini-batches of 1,024 values,
//! and the dictionary's fallback to plain is decided at one of those
//! boundaries. The same rows in a different batch size are a different file.
//!
//! So the store has a grid -- [`BATCH_ROWS`], the one the published file was
//! written in -- and it **checks** it: every batch but the last has to be
//! exactly that many rows. A file that comes out differently is not a bug in
//! the store but a caller that batched differently, and the store says so
//! rather than writing a file whose bytes nobody can predict.
//!
//! # What it is not
//!
//! **No SQL. No transactions with rollback. No MVCC, no snapshot isolation. No
//! concurrent writers. No row-level updates. No compaction. One writer.** A
//! table grows by appending a file and committing it; it does not shrink, and
//! it does not change in place.
//!
//! # Where it sits
//!
//! **Below the engine.** This crate does not depend on `columnar-engine`: it
//! writes the files the engine reads, and the engine's read path cannot move
//! because of anything here. The glue between them is a path.

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod commit;
pub mod error;
pub mod manifest;
pub mod rows;
pub mod store;
pub mod writer;

pub use commit::{commit, MANIFEST_FILE, MANIFEST_TMP};
pub use error::{Error, Result};
pub use manifest::{FileEntry, Manifest, FORMAT};
pub use store::{Store, Table};
pub use writer::{writer_properties, Writer, BATCH_ROWS, ROW_GROUP_ROWS};
