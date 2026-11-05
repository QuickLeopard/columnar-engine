//! The reader's four gigabytes: what a hostile file asks of it.
//!
//! ```text
//! cargo run --release -p columnar-engine --example hostile_metadata_probe -- \
//!     --out bench-results/hostile-metadata-run1.json
//! ```
//!
//! # The question
//!
//! **A library is read by people who did not write it, and it reads files its users provide.**
//! The engine's own record says what a hostile file can ask of it: *"a hostile Parquet file can
//! ask for four gigabytes before a byte of payload is read"*. **That sentence is a claim, and
//! this probe is what turns it into a figure.**
//!
//! **The cost is not time.** An allocation of four gigabytes is four gigabytes of *address space*,
//! and on this host it takes microseconds to ask for and the process carries the debt until it is
//! returned. So the probe measures **the largest single allocation the reader asked for**, with a
//! counting global allocator -- the number is the reader's, not the probe's -- and the time beside
//! it, so the figure says which of the two the cost is.
//!
//! # The experiment
//!
//! **The hostile file is a real file with one hostile page header.** The probe writes a real
//! Parquet file -- one column, one row group, one page -- with `parquet`'s own writer, and then
//! replaces that page's header with a hand-written one: a Thrift compact header whose statistics
//! field **declares a length** and then stops. **The footer is untouched**, so the file's metadata
//! still describes it, and the reader walks the file exactly as it walks an honest one.
//!
//! **The declared length is a curve, not a number.** 1 MiB, 64 MiB, 1 GiB, 4 GiB. **If the reader
//! asks for what the file declares, the largest allocation is the declared length, and the curve
//! is the proof** -- a reader that refuses the file asks for the same small thing four times, and a
//! reader that allocates what it is told draws a straight line through four gigabytes.
//!
//! **And the honest file is the control**: the same file, unpatched, so the figure says what the
//! reader asks for when there is nothing hostile in it.
//!
//! # The answers are the engine's
//!
//! The file is read through the **engine's own reader** -- `ParquetSource::open`, then the
//! engine's own `read` -- so the figure is about the path an embedder's file takes, and not about
//! `parquet`'s API in the abstract.

#![allow(clippy::indexing_slicing)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use arrow::array::Int64Array;
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use columnar_engine::pushdown::PushdownPlan;
use columnar_engine::{Flavor, ParquetSource};
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};

/// The declared lengths the probe asks for, in bytes.
///
/// **The last one is the published claim**: four gigabytes, the most a Thrift length can declare.
const DECLARED: [u64; 4] = [1 << 20, 64 << 20, 1 << 30, u32::MAX as u64];

/// The column the file writes.
const COLUMN: &str = "k";

/// The rows the file writes.
const ROWS: usize = 3;

/// The largest single allocation the process has asked for.
static LARGEST: AtomicU64 = AtomicU64::new(0);

/// The number of allocations the process has asked for.
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

/// A global allocator that counts, so the figure is the reader's.
struct Counting;

impl Counting {
    /// Records one allocation.
    fn record(size: usize) {
        LARGEST.fetch_max(size as u64, Ordering::Relaxed);
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    }
}

// The allocator is the instrument: it has to be the one every allocation goes through, and the
// unsafe blocks are what that takes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Counting::record(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Counting::record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Counting::record(new_size);
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Milliseconds, for printing.
fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The commit the working tree is at, for the report.
fn commit_sha() -> String {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success());
    output.map_or_else(
        || "unknown".to_owned(),
        |output| String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    )
}

/// The host's load average, for the report.
fn load_average() -> f64 {
    let output = std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .ok()
        .filter(|output| output.status.success());
    output.map_or_else(
        || 0.0,
        |output| {
            String::from_utf8_lossy(&output.stdout)
                .into_owned()
                .replace(['{', '}'], " ")
                .split_whitespace()
                .next()
                .and_then(|first| first.parse::<f64>().ok())
                .unwrap_or(0.0)
        },
    )
}

/// The Thrift compact protocol's unsigned varint.
fn varint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

/// **A hostile page header**: the statistics' `max` field declares `declared` bytes, and then the
/// header stops.
///
/// **The Thrift compact protocol's field header is one byte**: the high nibble is the *delta* from
/// the last field's id and the low nibble is the type. So a field's id is the running sum of the
/// deltas, and the deltas here are 1 for every field except the page header's fifth, which is 2 --
/// because the ids go 1, 2, 3, and then 5.
///
/// The types are the compact protocol's: 5 is `I32` and 12 is `STRUCT`.
fn hostile_page_header(declared: u64) -> Vec<u8> {
    let mut header = Vec::new();
    // The page header: the type, the two page sizes, and the data page header.
    header.extend_from_slice(&[0x15, 0x00]); // field 1: the type, DATA_PAGE
    header.extend_from_slice(&[0x15, 0x00]); // field 2: the uncompressed page size
    header.extend_from_slice(&[0x15, 0x00]); // field 3: the compressed page size
    header.push(0x2C); // field 5: the data page header (the delta is 2)
                       // The data page header: the values, the encoding, the two level encodings, the statistics.
    header.extend_from_slice(&[0x15, 0x00]); // field 1: the number of values
    header.extend_from_slice(&[0x15, 0x00]); // field 2: the encoding, PLAIN
    header.extend_from_slice(&[0x15, 0x06]); // field 3: the definition level encoding, RLE
    header.extend_from_slice(&[0x15, 0x06]); // field 4: the repetition level encoding, RLE
    header.push(0x1C); // field 5: the statistics (the delta is 1)
                       // The statistics: the maximum, and the length that claims everything.
    header.push(0x18); // field 1: the maximum (the type is BINARY, 8)
    varint(declared, &mut header);
    header.push(0x00); // stop: the statistics
    header.push(0x00); // stop: the data page header
    header.push(0x00); // stop: the page header
    header
}

/// **A real file with one hostile page header.**
///
/// The file is written by `parquet`'s own writer, so its metadata describes it honestly, and then
/// the page's header is replaced -- in place, and the file's length does not change, so the
/// footer stays where the footer is and the metadata still points at the chunk it names.
///
/// The size of the hostile header is returned, so the report can say how many bytes of file asked
/// for four gigabytes.
fn hostile_file(path: &Path, declared: Option<u64>) -> usize {
    let schema = Schema::new(vec![Field::new(COLUMN, DataType::Int64, false)]);
    let keys: Int64Array =
        (0..ROWS).map(|row| i64::try_from(row).expect("the row fits")).collect::<Vec<_>>().into();
    let batch = RecordBatch::try_new(schema.clone().into(), vec![std::sync::Arc::new(keys)])
        .expect("the batch is built");
    let properties = WriterProperties::builder()
        .set_compression(Compression::UNCOMPRESSED)
        .set_dictionary_enabled(false)
        .set_statistics_enabled(EnabledStatistics::Chunk)
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .build();

    let mut real = Vec::new();
    {
        let mut writer = ArrowWriter::try_new(&mut real, schema.into(), Some(properties))
            .expect("the writer opens");
        writer.write(&batch).expect("the row is written");
        writer.close().expect("the file closes");
    }

    // Where the chunk is, according to the file's own metadata.
    let scratch = path.with_extension("real");
    std::fs::write(&scratch, &real).expect("the file is written");
    let metadata = parquet::file::metadata::ParquetMetaDataReader::new()
        .parse_and_finish(&File::open(&scratch).expect("the file opens"))
        .expect("the metadata reads");
    let chunk = metadata.row_group(0).column(0);
    let offset = usize::try_from(chunk.data_page_offset()).expect("the offset fits");
    let size = usize::try_from(chunk.compressed_size()).expect("the size fits");
    drop(metadata);
    std::fs::remove_file(&scratch).ok();

    let Some(declared) = declared else {
        // The control: the honest file.
        std::fs::write(path, &real).expect("the file is written");
        return 0;
    };

    let header = hostile_page_header(declared);
    assert!(header.len() <= size, "the hostile header fits the chunk it replaces");
    let mut patched = Vec::with_capacity(real.len());
    patched.extend_from_slice(&real[..offset]);
    patched.extend_from_slice(&header);
    patched.resize(offset + size, 0);
    patched.extend_from_slice(&real[offset + size..]);
    std::fs::write(path, &patched).expect("the file is written");
    header.len()
}

/// What one reading of the file did.
#[derive(Debug, Clone)]
struct Reading {
    /// The largest single allocation the reader asked for.
    largest: u64,
    /// How many allocations the reader asked for.
    allocations: u64,
    /// How long the read took, in milliseconds.
    elapsed_ms: f64,
    /// What the reader said.
    outcome: String,
}

/// Reads the file through the engine's own reader, and says what it asked for.
fn read(path: &Path) -> Reading {
    LARGEST.store(0, Ordering::Relaxed);
    ALLOCATIONS.store(0, Ordering::Relaxed);
    let started = Instant::now();
    let outcome = match ParquetSource::open(path) {
        Err(error) => Err(error.to_string()),
        Ok(source) => source
            .read(&[0], &PushdownPlan::new(), Flavor::Scalar, 1)
            .map(|_| "read".to_owned())
            .map_err(|error| error.to_string()),
    };
    let elapsed = started.elapsed();
    Reading {
        largest: LARGEST.load(Ordering::Relaxed),
        allocations: ALLOCATIONS.load(Ordering::Relaxed),
        elapsed_ms: ms(elapsed),
        outcome: outcome.unwrap_or_else(|error| error),
    }
}

/// The probe's arguments.
struct Arguments {
    /// Where the report goes.
    out: Option<PathBuf>,
    /// Where the hostile files are written.
    work_dir: PathBuf,
}

/// The probe's arguments, from the command line.
fn arguments() -> Arguments {
    let mut out = None;
    let mut work_dir = std::env::temp_dir().join("columnar-hostile-metadata");
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--out" => {
                out = arguments.next().map(PathBuf::from);
            }
            "--work-dir" => {
                work_dir = arguments.next().map_or(work_dir, PathBuf::from);
            }
            other => panic!("an unknown argument: {other}"),
        }
    }
    Arguments { out, work_dir }
}

fn main() {
    let arguments = arguments();
    std::fs::create_dir_all(&arguments.work_dir).expect("the work directory is made");

    println!("=== the control: an honest file");
    let control_path = arguments.work_dir.join("control.parquet");
    hostile_file(&control_path, None);
    let control = read(&control_path);
    println!(
        "  the largest allocation: {} bytes, {} allocations, {:.3} ms, {}",
        control.largest, control.allocations, control.elapsed_ms, control.outcome
    );

    println!("\n=== the hostile file: the page header declares a length");
    let mut rows = Vec::new();
    for declared in DECLARED {
        let path = arguments.work_dir.join(format!("hostile-{declared}.parquet"));
        let header_len = hostile_file(&path, Some(declared));
        let reading = read(&path);
        println!(
            "  declared {:>12}: the largest allocation {:>12}, {} allocations, {:.3} ms, {}",
            declared, reading.largest, reading.allocations, reading.elapsed_ms, reading.outcome
        );
        rows.push(serde_json::json!({
            "declared_bytes": declared,
            "hostile_header_bytes": header_len,
            "largest_allocation_bytes": reading.largest,
            "allocations": reading.allocations,
            "elapsed_ms": reading.elapsed_ms,
            "outcome": reading.outcome,
        }));
    }

    // The control's own row, so the figure has a baseline to be read against.
    let control_row = serde_json::json!({
        "declared_bytes": 0,
        "hostile_header_bytes": 0,
        "largest_allocation_bytes": control.largest,
        "allocations": control.allocations,
        "elapsed_ms": control.elapsed_ms,
        "outcome": control.outcome,
    });

    // The ratio the figure is read by: what the reader asked for, over what the file declared.
    let ratios: Vec<f64> = rows
        .iter()
        .map(|row| {
            let declared = row["declared_bytes"].as_u64().unwrap_or(1);
            let largest = row["largest_allocation_bytes"].as_u64().unwrap_or(0);
            // Both fit in a u32 -- the largest declared length is u32::MAX -- so
            // the share is exact, and the conversion cannot lose precision.
            f64::from(u32::try_from(largest).unwrap_or(0))
                / f64::from(u32::try_from(declared).unwrap_or(1))
        })
        .collect();

    let report = serde_json::json!({
        "probe": "hostile-metadata",
        "commit": commit_sha(),
        "host_load_average": load_average(),
        "control": control_row,
        "hostile": rows,
        "derived": {
            // The share of the declared length the reader asked for. One is "it asked for
            // everything the file declared"; zero is "it refused the file without asking".
            "asked_share": ratios,
            "the_largest_asked": rows
                .iter()
                .map(|row| row["largest_allocation_bytes"].as_u64().unwrap_or(0))
                .max()
                .unwrap_or(0),
            "the_control_asked": control.largest,
        },
    });

    println!("\n--- the report ---");
    println!("{report}");
    if let Some(out) = arguments.out {
        std::fs::write(&out, format!("{report}\n")).expect("the report is written");
        println!("report: {}", out.display());
    }

    std::fs::remove_dir_all(&arguments.work_dir).ok();
}
