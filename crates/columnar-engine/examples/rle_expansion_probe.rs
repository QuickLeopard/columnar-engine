//! Is the decode's cost the expansion of six million RLE dictionary indices?
//!
//! This is the measurement behind spec 012's T3, and it is a diagnostic, not
//! part of the engine.
//!
//! ```text
//! cargo run --release -p columnar-engine --example rle_expansion_probe -- data/lineitem.parquet --out report.json
//! ```
//!
//! # The question
//!
//! Spec 012 refused to own the decode: the parquet crate's own typed reader
//! costs 10.30 ms and Arrow's decode of the same two columns costs 9.86 ms,
//! so Arrow's layer is worth 0.3 ms and owning the decode would buy nothing.
//! That refusal left one hypothesis standing, and this probe is the test of
//! it:
//!
//! > The decode's cost is the **expansion of six million RLE dictionary
//! > indices into six million values**, and the engine throws most of it away
//! > because the predicate's column is expanded, compared row by row, and
//! > dropped.
//!
//! # What is measured
//!
//! Every figure is on the **same** file, in the **same** process, warm, and
//! is the median of five runs.
//!
//! | | |
//! |---|---|
//! | **P** | the parquet crate's own decode of the column: the indices *and* the gather |
//! | **A** | Arrow's decode of the column: what the engine's reader does |
//! | **PLAIN** | the decode of the **same column written without a dictionary** |
//! | **V** | the probe's decode: the indices *and* the gather, done by the probe |
//! | **I** | the probe's decode of the indices **only**: no gather, no values |
//! | **G** | the expansion: the reader's fresh buffers, and the gather |
//! | **G1** | the reader's fresh buffers, and their zeroing |
//! | **G2** | the gather alone, into a buffer that is already there |
//! | **M** | the mask the dictionary comparison would have to build |
//! | **C** | the probe, then the predicate applied to the **dictionary** |
//! | **S1** | the engine's scan of the column, with the predicate |
//! | **S2** | the engine's scan of the whole query |
//!
//! - **V − I** is the expansion, measured with the probe's own decoder.
//! - **P − V** is the gap between the parquet crate's RLE decoder and the
//!   probe's. It is reported so that nobody has to take the probe's decoder
//!   on trust.
//! - **G** is the expansion as the engine pays it.
//! - **M** is what the hypothesis has to pay for what it saves: the mask is
//!   still one bit per row, and the row's bit is the predicate's answer for
//!   the row's index.
//! - **C** is the hypothesis's shape, end to end.
//!
//! # Why the probe owns the RLE
//!
//! The parquet crate's RLE decoder is private: `encodings/rle.rs` is a
//! `pub(crate)` module, and the `RleDecoder` it holds cannot be reached from
//! outside the crate even with the `experimental` feature on. The only
//! public way to read a dictionary-encoded column is the whole expansion:
//! `DictDecoder::get` decodes the indices *and* gathers the dictionary
//! through them, in one call, with no seam between the two.
//!
//! So the probe owns the RLE: the Parquet format's RLE/bit-packed hybrid, in
//! about a hundred lines. And it is checked against the parquet crate
//! itself -- the probe expands every value of every row group with its own
//! decoder and compares it, bit for bit, with what the parquet crate's own
//! reader produced. If the two disagree, the probe says so and stops. An
//! instrument that has not been checked against the reference it is measuring
//! against is not an instrument.
//!
//! # The slices and the indexes
//!
//! Every slice and index in this probe is into a buffer the probe has just
//! filled, and every length is asserted immediately above the index that
//! uses it. The lint is allowed here, and the assertion is the reason.

#![allow(clippy::indexing_slicing)]

mod common;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use arrow::array::{ArrayRef, Float64Array, RecordBatch};
use arrow::datatypes::{DataType as ArrowType, Field, Schema};
use columnar_core::ComparisonOp;
use columnar_engine::pushdown::{Pushdown, PushdownPlan};
use columnar_engine::ParquetSource;
use common::{
    decode_dictionary, decode_indices, for_each_page, repeat, split_v1_page, Scratch, Timing,
};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::arrow::ProjectionMask;
use parquet::basic::Encoding;
use parquet::column::page::Page;
use parquet::column::reader::get_typed_column_reader;
use parquet::data_type::DoubleType;
use parquet::file::properties::WriterProperties;
use parquet::file::reader::{FileReader, SerializedFileReader};

/// The file's root index of `l_extendedprice`.
const EXTENDED_PRICE: usize = 5;
/// The file's root index of `l_discount`.
const DISCOUNT: usize = 6;

/// How many times each measurement is repeated.
const ITERATIONS: usize = 5;

/// The rows the parquet reader hands over at a time.
const BATCH_ROWS: usize = 65_536;

// ---------------------------------------------------------------------------
// The file's pages
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The measurements
// ---------------------------------------------------------------------------

/// The parquet crate's own decode of one column, into plain values.
///
/// This is the crate's own column reader: it fetches the pages, decompresses
/// them, decodes the RLE indices and gathers the dictionary through them.
fn parquet_decode(path: &Path, column: usize) -> (Duration, Vec<f64>) {
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    let mut values: Vec<f64> = Vec::with_capacity(BATCH_ROWS);
    let mut levels: Vec<i16> = Vec::with_capacity(BATCH_ROWS);

    let started = Instant::now();
    for group in 0..reader.num_row_groups() {
        let group = reader.get_row_group(group).expect("the row group reads");
        let column_reader = group.get_column_reader(column).expect("the column reader opens");
        let mut typed = get_typed_column_reader::<DoubleType>(column_reader);
        loop {
            levels.clear();
            levels.resize(BATCH_ROWS, 0);
            let before = values.len();
            let (records, values_read, _) = typed
                .read_records(BATCH_ROWS, Some(&mut levels), None, &mut values)
                .expect("the read runs");
            if records == 0 {
                break;
            }
            assert_eq!(values_read, records, "this file has no NULLs");
            // The parquet reader *appends* to the values buffer, so the
            // count that came back is the count that was added.
            assert_eq!(values.len() - before, values_read, "the reader appended what it read");
            if records < BATCH_ROWS {
                break;
            }
        }
    }
    let elapsed = started.elapsed();
    std::hint::black_box(&values);
    (elapsed, values)
}

/// Arrow's decode of one column: what the engine's reader does.
fn arrow_decode(path: &Path, column: usize) -> (Duration, usize) {
    let file = File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let mask = ProjectionMask::roots(builder.parquet_schema(), [column]);
    let reader = builder
        .with_projection(mask)
        .with_batch_size(BATCH_ROWS)
        .build()
        .expect("the reader builds");

    let started = Instant::now();
    let mut rows = 0usize;
    for batch in reader {
        let batch = batch.expect("the batch");
        rows += batch.num_rows();
    }
    (started.elapsed(), rows)
}

/// The probe's own decode of one column, with the gather.
///
/// The same work the parquet crate does: the pages are fetched and
/// decompressed, the indices are decoded by the probe's RLE decoder, and the
/// dictionary is gathered through them.
fn probe_decode(
    path: &Path,
    column: usize,
    max_rep_level: i16,
    max_def_level: i16,
    scratch: &mut Scratch,
) -> (Duration, Vec<f64>) {
    scratch.values.clear();
    scratch.values.reserve(6_000_000);

    let started = Instant::now();
    for_each_page(
        path,
        column,
        max_rep_level,
        max_def_level,
        |_encoding, dictionary, _levels, values, section| {
            decode_indices(section, values, &mut scratch.indices);
            scratch
                .values
                .extend(scratch.indices[..values].iter().map(|&index| dictionary[index as usize]));
        },
    );
    let elapsed = started.elapsed();
    let values = scratch.values.clone();
    (elapsed, values)
}

/// The probe's own decode of one column, **without** the gather.
///
/// The pages are fetched and decompressed, and the indices are decoded. The
/// dictionary is never touched. This is the decode the hypothesis proposes.
fn probe_indices(
    path: &Path,
    column: usize,
    max_rep_level: i16,
    max_def_level: i16,
    scratch: &mut Scratch,
) -> (Duration, usize) {
    let mut count = 0usize;

    let started = Instant::now();
    for_each_page(
        path,
        column,
        max_rep_level,
        max_def_level,
        |_encoding, _dictionary, _levels, values, section| {
            decode_indices(section, values, &mut scratch.indices);
            count += values;
        },
    );
    let elapsed = started.elapsed();
    std::hint::black_box(&scratch.indices);
    (elapsed, count)
}

/// What the expansion costs, in the three parts it is made of.
///
/// Arrow's reader does two things the parquet crate's own column reader does
/// not, and both of them are part of what it costs to materialise the values:
///
/// - It **allocates a fresh buffer for every batch**. The record reader hands
///   its values buffer to the array it builds and takes an empty one, so the
///   buffer is new memory every time: 48 MB of fresh pages over the column.
/// - It **zeroes that buffer before the decoder writes into it**, because
///   `ColumnValueDecoderImpl::read` resizes the output buffer with the
///   default value.
///
/// So the expansion is three things, and this measures all three: the
/// allocation and the zeroing (**G1**), the gather itself (**G2**), and the
/// two together (**G**).
fn expansion(dictionary: &[f64], indices: &[u32], part: ExpansionPart) -> (Duration, usize) {
    let mut written = 0usize;
    let mut buffer: Vec<f64> = Vec::new();
    let started = Instant::now();
    for chunk in indices.chunks(BATCH_ROWS) {
        match part {
            ExpansionPart::Buffers => {
                // A fresh buffer, zeroed, as the reader makes it.
                let mut values: Vec<f64> = Vec::new();
                values.resize(chunk.len(), 0.0);
                written += values.len();
                std::hint::black_box(&values);
            }
            ExpansionPart::Gather => {
                // The buffer is already there, already faulted in: the
                // gather on its own.
                if buffer.len() < chunk.len() {
                    buffer.resize(chunk.len(), 0.0);
                }
                for (slot, &index) in buffer[..chunk.len()].iter_mut().zip(chunk) {
                    *slot = dictionary[index as usize];
                }
                written += chunk.len();
            }
            ExpansionPart::Both => {
                let mut values: Vec<f64> = Vec::new();
                values.resize(chunk.len(), 0.0);
                for (slot, &index) in values.iter_mut().zip(chunk) {
                    *slot = dictionary[index as usize];
                }
                written += values.len();
                std::hint::black_box(&values);
            }
        }
    }
    let elapsed = started.elapsed();
    (elapsed, written)
}

/// Which part of the expansion to measure.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ExpansionPart {
    /// The reader's fresh buffers, and their zeroing.
    Buffers,
    /// The gather alone.
    Gather,
    /// Both.
    Both,
}

/// The mask of the rows that pass, one bit per row, LSB-first.
struct Mask {
    words: Vec<u64>,
    len: usize,
}

impl Mask {
    /// A mask over `len` rows.
    fn new(len: usize) -> Self {
        Self { words: vec![0u64; len.div_ceil(64)], len }
    }

    /// How many rows the mask keeps.
    fn count(&self) -> usize {
        let mut count = 0;
        for (index, &word) in self.words.iter().enumerate() {
            let bits = self.len.saturating_sub(index * 64).min(64);
            if bits == 64 {
                count += word.count_ones() as usize;
            } else {
                count += (word & ((1u64 << bits) - 1)).count_ones() as usize;
            }
        }
        count
    }
}

/// The mask the dictionary comparison would have to build.
///
/// The hypothesis replaces a comparison over the values with a comparison
/// over the **dictionary**, and then has to map the indices through the
/// answer. The mapping is per row: one bit per row, and the row's bit is
/// the predicate's answer for the row's index.
///
/// This measures that mapping on the real indices, with the predicate's
/// answer already computed. It is the cost the hypothesis has to pay for
/// what it saves.
///
/// The answer is **per row group**, because every row group has its own
/// dictionary: the writer builds one per column chunk, and the order of the
/// entries in it is the order the values were first seen in that chunk. Two
/// row groups hold the same eleven values in a different order, and an
/// answer computed for one of them is wrong for the other.
fn mask_from_indices(groups: &[(u64, Vec<u32>)]) -> (Duration, usize) {
    let mut survivors = 0usize;
    let rows = groups.iter().map(|(_, indices)| indices.len()).max().unwrap_or(0);
    let mut mask = Mask::new(rows);
    let started = Instant::now();
    for (pass_word, indices) in groups {
        mask_words(*pass_word, indices, &mut mask.words);
        mask.len = indices.len();
        survivors += mask.count();
        std::hint::black_box(&mask);
    }
    let elapsed = started.elapsed();
    (elapsed, survivors)
}

/// The mask's words: one bit per row, out of the indices.
///
/// `pass_word` is the predicate's answer for the dictionary, one bit per
/// dictionary entry, and the row's bit is the answer for the row's index.
fn mask_words(pass_word: u64, indices: &[u32], words: &mut [u64]) {
    for (word_index, chunk) in indices.chunks(64).enumerate() {
        let mut bits = 0u64;
        for (bit, &index) in chunk.iter().enumerate() {
            // The index is masked to six bits because the answer is one
            // word, and a word holds sixty-four dictionary entries.
            bits |= ((pass_word >> (index & 63)) & 1) << bit;
        }
        words[word_index] = bits;
    }
}

/// The indices of every row group, with the predicate's answer for that
/// group's own dictionary.
fn indices_by_group(
    path: &Path,
    column: usize,
    max_rep_level: i16,
    max_def_level: i16,
    scratch: &mut Scratch,
) -> Vec<(u64, Vec<u32>)> {
    let flavor = columnar_kernels::best();
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    let mut groups: Vec<(u64, Vec<u32>)> = Vec::new();
    let mut levels: Vec<u16> = Vec::new();
    for group_number in 0..reader.num_row_groups() {
        let group = reader.get_row_group(group_number).expect("the row group reads");
        let mut pages = group.get_column_page_reader(column).expect("the page reader opens");
        let mut dictionary: Vec<f64> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        while let Some(page) = pages.get_next_page().expect("the page reads") {
            match page {
                Page::DictionaryPage { buf, num_values, .. } => {
                    dictionary = decode_dictionary(&buf, num_values);
                }
                Page::DataPage { buf, num_values, .. } => {
                    let rows = num_values as usize;
                    let (values, section) =
                        split_v1_page(&buf, max_rep_level, max_def_level, rows, &mut levels);
                    decode_indices(section, values, &mut scratch.indices);
                    indices.extend_from_slice(&scratch.indices[..values]);
                }
                Page::DataPageV2 { .. } => panic!("the file holds a v2 data page"),
            }
        }
        let pass_word = pass_word_for(&dictionary, flavor);
        groups.push((pass_word, indices));
    }
    groups
}

/// The predicate's answer for a dictionary, as one word of bits.
///
/// Bit `i` is set when the dictionary's `i`th value passes.
fn pass_word_for(dictionary: &[f64], flavor: columnar_kernels::Flavor) -> u64 {
    pass_word_for_threshold(dictionary, flavor, 0.04)
}

/// The predicate's answer for a dictionary, at a threshold.
///
/// Bit `i` is set when the dictionary's `i`th value passes.
fn pass_word_for_threshold(
    dictionary: &[f64],
    flavor: columnar_kernels::Flavor,
    threshold: f64,
) -> u64 {
    let mut pass = vec![0u64; dictionary.len().div_ceil(64).max(1)];
    columnar_kernels::compare_f64_into(
        flavor,
        dictionary,
        ComparisonOp::GtEq,
        threshold,
        None,
        &mut pass,
    );
    let mut upper = vec![0u64; dictionary.len().div_ceil(64).max(1)];
    columnar_kernels::compare_f64_into(
        flavor,
        dictionary,
        ComparisonOp::LtEq,
        0.07,
        None,
        &mut upper,
    );
    for (word, upper_word) in pass.iter_mut().zip(upper.iter()) {
        *word &= upper_word;
    }
    pass[0]
}

/// The probe's decode, then the predicate applied to the **dictionary**.
///
/// This is the shape the hypothesis proposes: the predicate is evaluated
/// once per distinct value, and the indices are mapped through the answer.
fn probe_dictionary_compare(
    path: &Path,
    column: usize,
    max_rep_level: i16,
    max_def_level: i16,
    threshold: f64,
    scratch: &mut Scratch,
) -> (Duration, usize) {
    let flavor = columnar_kernels::best();
    let max_level = max_def_level.unsigned_abs();
    let mut survivors = 0usize;

    let started = Instant::now();
    for_each_page(
        path,
        column,
        max_rep_level,
        max_def_level,
        |_encoding, dictionary, levels, values, section| {
            // The predicate, once per distinct value.
            let pass_word = pass_word_for_threshold(dictionary, flavor, threshold);
            let mut lookup = [0u8; 256];
            for (bit, slot) in lookup.iter_mut().enumerate() {
                *slot = u8::from((pass_word >> bit) & 1 == 1);
            }

            decode_indices(section, values, &mut scratch.indices);

            // The mask: one bit per row, built a word at a time.
            let mut mask = Mask::new(levels.len());
            for (word_index, chunk) in scratch.indices[..values].chunks(64).enumerate() {
                let mut bits = 0u64;
                for (bit, &index) in chunk.iter().enumerate() {
                    bits |= u64::from(lookup[(index & 0xFF) as usize]) << bit;
                }
                mask.words[word_index] = bits;
            }
            // A NULL row never matches, for every operator.
            for (offset, &level) in levels.iter().enumerate() {
                if level != max_level {
                    mask.words[offset / 64] &= !(1u64 << (offset % 64));
                }
            }
            survivors += mask.count();
        },
    );
    let elapsed = started.elapsed();
    std::hint::black_box(&scratch.indices);
    (elapsed, survivors)
}

/// Writes the same column again, without a dictionary.
///
/// This is the other half of spec 012's R1: "the decode of a
/// dictionary-encoded column against the decode of the same column without
/// one". The file it writes holds the same six million values, PLAIN
/// encoded, so the only difference is the dictionary.
fn write_plain(path: &Path, values: &[f64]) {
    let schema = Arc::new(Schema::new(vec![Field::new("l_discount", ArrowType::Float64, false)]));
    let properties = WriterProperties::builder()
        .set_dictionary_enabled(false)
        .set_encoding(Encoding::PLAIN)
        .build();
    let file = File::create(path).expect("the plain file is created");
    let mut writer =
        ArrowWriter::try_new(file, schema.clone(), Some(properties)).expect("the writer builds");
    for chunk in values.chunks(BATCH_ROWS) {
        let array: ArrayRef = Arc::new(Float64Array::from(chunk.to_vec()));
        let batch = RecordBatch::try_new(schema.clone(), vec![array]).expect("the batch");
        writer.write(&batch).expect("the batch is written");
    }
    writer.close().expect("the file is closed");
}

/// The engine's predicates, as the engine pushes them.
fn plan() -> PushdownPlan {
    let mut plan = PushdownPlan::new();
    let mut discount = Pushdown::new(
        DISCOUNT,
        "l_discount",
        ComparisonOp::GtEq,
        columnar_core::ScalarValue::Float64(0.04),
    );
    discount.and(ComparisonOp::LtEq, columnar_core::ScalarValue::Float64(0.07));
    plan.push(discount);
    plan
}

/// The engine's own scan of the predicate's column, end to end.
///
/// The plan materialises nothing, so the scan reads `l_discount`, applies
/// the predicate to the reader's values, and counts the survivors. It is the
/// engine's shape, and it is what the dictionary comparison would replace.
///
/// Returns the elapsed time, the number of surviving rows, and the time the
/// predicate itself took, which the engine measures.
fn engine_scan(path: &Path, pushdown: &PushdownPlan) -> (Duration, usize, Duration) {
    let source = ParquetSource::open(path).expect("the source opens");
    let started = Instant::now();
    let dataset = source.read(&[], pushdown, columnar_kernels::best(), 1).expect("the read runs");
    let elapsed = started.elapsed();
    let rows = dataset.len();
    let predicate = pushdown.metrics().predicate_time();
    std::hint::black_box(&dataset);
    (elapsed, rows, predicate)
}

/// The engine's own scan of the whole query.
fn engine_scan_full(path: &Path) -> (Duration, usize) {
    let source = ParquetSource::open(path).expect("the source opens");
    let started = Instant::now();
    let dataset = source
        .read(&[EXTENDED_PRICE], &plan(), columnar_kernels::best(), 1)
        .expect("the read runs");
    let elapsed = started.elapsed();
    let rows = dataset.len();
    std::hint::black_box(&dataset);
    (elapsed, rows)
}

/// The commit the working tree is at, for the report.
fn commit() -> String {
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
    output
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .and_then(|text| {
            text.replace(['{', '}'], " ")
                .split_whitespace()
                .next()
                .and_then(|first| first.parse::<f64>().ok())
        })
        .unwrap_or(0.0)
}

/// The number of logical cores, for the report.
fn cores() -> usize {
    std::thread::available_parallelism().map_or(0, usize::from)
}

/// The predicate's column, per row group.
struct GroupShape {
    /// How many rows the group holds.
    rows: usize,
    /// How many distinct values its dictionary holds.
    dictionary_values: usize,
    /// How many bits the indices are packed into.
    bit_width: u8,
    /// How many values it holds.
    values: usize,
    /// How many bytes its pages hold, the dictionary's included.
    bytes: usize,
    /// How many bytes its dictionary page holds.
    dictionary_bytes: usize,
}

/// Everything the probe measured about the decode.
struct Decodes {
    /// The parquet crate's own decode.
    parquet: Timing,
    /// Arrow's decode.
    arrow: Timing,
    /// The decode of the same column written without a dictionary.
    plain: Timing,
    /// The same, through Arrow.
    plain_arrow: Timing,
    /// The probe's own decode.
    probe: Timing,
    /// The probe's decode of the indices only.
    indices: Timing,
    /// The expansion.
    expansion: Timing,
    /// The reader's fresh buffers.
    buffers: Timing,
    /// The gather alone.
    gather: Timing,
    /// The mask the hypothesis would build.
    mask: Timing,
    /// The hypothesis's shape, end to end.
    prototype: Timing,
    /// How many bytes the file written without a dictionary holds.
    plain_bytes: u64,
    /// How many rows the file holds.
    rows: usize,
    /// How many rows survive the predicate.
    survivors: usize,
}

/// The arguments: the file, and where to write the report.
fn arguments() -> (PathBuf, Option<String>) {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().unwrap_or_else(|| "data/lineitem.parquet".to_owned());
    let mut out: Option<String> = None;
    while let Some(argument) = arguments.next() {
        if argument == "--out" {
            out = arguments.next();
        }
    }
    (PathBuf::from(path), out)
}

/// The file's shape, and the level bounds its reader needs.
fn read_shape(path: &Path) -> (Vec<GroupShape>, i16, i16) {
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    let schema = reader.metadata().file_metadata().schema_descr();
    let max_def_level = schema.column(DISCOUNT).max_def_level();
    let max_rep_level = schema.column(DISCOUNT).max_rep_level();

    let mut groups: Vec<GroupShape> = Vec::new();
    let mut levels: Vec<u16> = Vec::new();
    for group_number in 0..reader.num_row_groups() {
        let group = reader.get_row_group(group_number).expect("the row group reads");
        let mut pages = group.get_column_page_reader(DISCOUNT).expect("the page reader opens");
        let mut shape = GroupShape {
            rows: 0,
            dictionary_values: 0,
            bit_width: 0,
            values: 0,
            bytes: 0,
            dictionary_bytes: 0,
        };
        while let Some(page) = pages.get_next_page().expect("the page reads") {
            match page {
                Page::DictionaryPage { buf, num_values, .. } => {
                    shape.dictionary_values = num_values as usize;
                    shape.dictionary_bytes += buf.len();
                    shape.bytes += buf.len();
                }
                Page::DataPage { buf, num_values, .. } => {
                    let rows = num_values as usize;
                    let (values, section) =
                        split_v1_page(&buf, max_rep_level, max_def_level, rows, &mut levels);
                    if shape.bit_width == 0 {
                        shape.bit_width = section[0];
                    }
                    shape.rows += rows;
                    shape.values += values;
                    shape.bytes += section.len();
                }
                Page::DataPageV2 { .. } => panic!("the file holds a v2 data page"),
            }
        }
        groups.push(shape);
    }
    (groups, max_rep_level, max_def_level)
}

/// Measures the decodes.
fn measure_decodes(path: &Path, max_rep_level: i16, max_def_level: i16) -> Decodes {
    let mut scratch = Scratch::new(1_000_000);
    let (parquet, values) = repeat(ITERATIONS, || parquet_decode(path, DISCOUNT));
    let (arrow, _) = repeat(ITERATIONS, || arrow_decode(path, DISCOUNT));
    let (probe, probe_values) = repeat(ITERATIONS, || {
        probe_decode(path, DISCOUNT, max_rep_level, max_def_level, &mut scratch)
    });
    let (indices, rows) = repeat(ITERATIONS, || {
        probe_indices(path, DISCOUNT, max_rep_level, max_def_level, &mut scratch)
    });

    // The probe's decoder is checked against the parquet crate's, bit for bit.
    assert_eq!(values.len(), probe_values.len(), "the same number of values");
    for (index, (left, right)) in values.iter().zip(probe_values.iter()).enumerate() {
        assert_eq!(
            left.to_bits(),
            right.to_bits(),
            "the probe's decode differs from the parquet crate's at value {index}"
        );
    }

    // The same column, written without a dictionary.
    let plain_path = Path::new("target/rle-expansion-plain.parquet");
    if let Some(parent) = plain_path.parent() {
        std::fs::create_dir_all(parent).expect("the target directory");
    }
    write_plain(plain_path, &values);
    let plain_bytes = std::fs::metadata(plain_path).expect("the plain file").len();
    let (plain, plain_values) = repeat(ITERATIONS, || parquet_decode(plain_path, 0));
    assert_eq!(plain_values.len(), values.len(), "the same number of values");
    let (plain_arrow, _) = repeat(ITERATIONS, || arrow_decode(plain_path, 0));

    // The expansion, in its three parts.
    let groups = indices_by_group(path, DISCOUNT, max_rep_level, max_def_level, &mut scratch);
    let all_indices: Vec<u32> =
        groups.iter().flat_map(|(_, indices)| indices.iter().copied()).collect();
    assert_eq!(all_indices.len(), rows, "the same number of indices");
    let dictionary = first_dictionary(path);
    let (expansion_time, expanded) =
        repeat(ITERATIONS, || expansion(&dictionary, &all_indices, ExpansionPart::Both));
    assert_eq!(expanded, rows, "the expansion wrote every value");
    let (buffers, _) =
        repeat(ITERATIONS, || expansion(&dictionary, &all_indices, ExpansionPart::Buffers));
    let (gather, _) =
        repeat(ITERATIONS, || expansion(&dictionary, &all_indices, ExpansionPart::Gather));

    // The mask the dictionary comparison would have to build.
    let (mask, survivors) = repeat(ITERATIONS, || mask_from_indices(&groups));

    // The hypothesis's shape, end to end.
    let (prototype, prototype_rows) = repeat(ITERATIONS, || {
        probe_dictionary_compare(path, DISCOUNT, max_rep_level, max_def_level, 0.04, &mut scratch)
    });
    assert_eq!(survivors, prototype_rows, "the mask and the prototype agree");

    Decodes {
        parquet,
        arrow,
        plain,
        plain_arrow,
        probe,
        indices,
        expansion: expansion_time,
        buffers,
        gather,
        mask,
        prototype,
        plain_bytes,
        rows,
        survivors,
    }
}

/// The dictionary of the file's first row group.
fn first_dictionary(path: &Path) -> Vec<f64> {
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    let group = reader.get_row_group(0).expect("the row group reads");
    let mut pages = group.get_column_page_reader(DISCOUNT).expect("the page reader opens");
    let mut dictionary = Vec::new();
    while let Some(page) = pages.get_next_page().expect("the page reads") {
        if let Page::DictionaryPage { buf, num_values, .. } = page {
            dictionary = decode_dictionary(&buf, num_values);
        }
    }
    dictionary
}

/// Prints the file's shape.
fn print_shape(groups: &[GroupShape]) {
    println!("--- the predicate's column, per row group ---");
    println!(
        "{:>6} {:>10} {:>8} {:>8} {:>10} {:>12}",
        "group", "rows", "dict", "bitwidth", "values", "index bytes"
    );
    for (number, group) in groups.iter().enumerate() {
        println!(
            "{:>6} {:>10} {:>8} {:>8} {:>10} {:>12}",
            number, group.rows, group.dictionary_values, group.bit_width, group.values, group.bytes
        );
    }
    println!();
}

/// Prints the decode's decomposition.
fn print_decode(decodes: &Decodes) {
    let rows = [
        ("P", "the parquet crate's decode", &decodes.parquet),
        ("A", "Arrow's decode", &decodes.arrow),
        ("PLAIN", "the same column, no dict", &decodes.plain),
        ("PLAIN", "the same column, through Arrow", &decodes.plain_arrow),
        ("V", "the probe's decode", &decodes.probe),
        ("I", "the probe's indices, no gather", &decodes.indices),
        ("G", "the expansion", &decodes.expansion),
        ("G1", "the reader's fresh buffers", &decodes.buffers),
        ("G2", "the gather alone", &decodes.gather),
        ("M", "the mask from the indices", &decodes.mask),
        ("C", "the probe, predicate on the dict", &decodes.prototype),
    ];
    println!("--- the decode of the predicate's column, decomposed ---");
    for (name, label, timing) in rows {
        println!(
            "  {name:<6}{label:<38}{:>7.2} ms  ({:.0}-{:.0})",
            timing.median, timing.min, timing.max
        );
    }
    println!();
    println!(
        "  the expansion, by the probe        : {:7.2} ms   (V - I)",
        decodes.probe.median - decodes.indices.median
    );
    println!(
        "  the parquet crate's RLE, by the gap: {:7.2} ms   (P - V)",
        decodes.parquet.median - decodes.probe.median
    );
    println!();
}

/// Prints the engine's scan.
fn print_scan(scan: &Timing, predicate: &Timing, query: &Timing, rows: usize) {
    println!("--- the engine's scan ---");
    println!(
        "  S1   the decode and the predicate  : {:7.2} ms  ({:.0}-{:.0})   ({rows} rows survive)",
        scan.median, scan.min, scan.max
    );
    println!("       the predicate's own share     : {:7.2} ms", predicate.median);
    println!("       the decode, by subtraction    : {:7.2} ms", scan.median - predicate.median);
    println!(
        "  S2   the whole query               : {:7.2} ms  ({:.0}-{:.0})   ({rows} rows survive)",
        query.median, query.min, query.max
    );
}

/// Writes the report.
fn write_report(
    out: &str,
    path: &Path,
    groups: &[GroupShape],
    decodes: &Decodes,
    scan: &Timing,
    predicate: &Timing,
    query: &Timing,
) {
    let decode = scan.median - predicate.median;
    let net_crate = decodes.expansion.median - (decodes.mask.median - predicate.median);
    let net_probe = net_crate - (decodes.parquet.median - decodes.probe.median);
    let report = serde_json::json!({
        "file": path.display().to_string(),
        "commit": commit(),
        "machine": format!("{} logical cores", cores()),
        "iterations": ITERATIONS,
        "rows": decodes.rows,
        "survivors": decodes.survivors,
        "dictionary_values": groups[0].dictionary_values,
        "bit_width": groups[0].bit_width,
        "index_bytes_per_row_group": groups[0].bytes - groups[0].dictionary_bytes,
        "dictionary_bytes_per_row_group": groups[0].dictionary_bytes,
        "plain_file_bytes": decodes.plain_bytes,
        "host_load_average": load_average(),
        "measurements_ms": {
            "parquet_decode": decodes.parquet.median,
            "arrow_decode": decodes.arrow.median,
            "plain_decode": decodes.plain.median,
            "plain_arrow_decode": decodes.plain_arrow.median,
            "probe_decode": decodes.probe.median,
            "probe_indices": decodes.indices.median,
            "expansion": decodes.expansion.median,
            "expansion_buffers": decodes.buffers.median,
            "expansion_gather": decodes.gather.median,
            "mask_from_indices": decodes.mask.median,
            "prototype": decodes.prototype.median,
            "engine_scan": scan.median,
            "engine_predicate": predicate.median,
            "engine_query": query.median,
        },
        "spreads_ms": {
            "parquet_decode": [decodes.parquet.min, decodes.parquet.max],
            "arrow_decode": [decodes.arrow.min, decodes.arrow.max],
            "plain_decode": [decodes.plain.min, decodes.plain.max],
            "plain_arrow_decode": [decodes.plain_arrow.min, decodes.plain_arrow.max],
            "probe_decode": [decodes.probe.min, decodes.probe.max],
            "probe_indices": [decodes.indices.min, decodes.indices.max],
            "expansion": [decodes.expansion.min, decodes.expansion.max],
            "expansion_buffers": [decodes.buffers.min, decodes.buffers.max],
            "expansion_gather": [decodes.gather.min, decodes.gather.max],
            "mask_from_indices": [decodes.mask.min, decodes.mask.max],
            "prototype": [decodes.prototype.min, decodes.prototype.max],
            "engine_scan": [scan.min, scan.max],
            "engine_predicate": [predicate.min, predicate.max],
            "engine_query": [query.min, query.max],
        },
        "derived_ms": {
            "expansion_by_probe": decodes.probe.median - decodes.indices.median,
            "rle_gap": decodes.parquet.median - decodes.probe.median,
            "decode_by_subtraction": decode,
            "expansion_share_of_decode": (decodes.expansion.median / decode) * 100.0,
            "expansion_share_of_query": (decodes.expansion.median / query.median) * 100.0,
            "predicate_share_of_scan": (predicate.median / scan.median) * 100.0,
            "mask_minus_predicate": decodes.mask.median - predicate.median,
            "net_with_the_crates_decoder": net_crate,
            "net_with_the_probes_decoder": net_probe,
        },
    });
    let text = serde_json::to_string_pretty(&report).expect("the report is written");
    std::fs::write(out, text).expect("the report is written");
    println!("\nreport: {out}");
}

fn main() {
    let (path, out) = arguments();
    println!("file: {}", path.display());
    println!("iterations: {ITERATIONS}\n");

    let (groups, max_rep_level, max_def_level) = read_shape(&path);
    print_shape(&groups);

    let decodes = measure_decodes(&path, max_rep_level, max_def_level);
    print_decode(&decodes);

    let mut predicate_samples: Vec<Duration> = Vec::with_capacity(ITERATIONS);
    let (scan, _) = repeat(ITERATIONS, || {
        let (elapsed, rows, predicate) = engine_scan(&path, &plan());
        predicate_samples.push(predicate);
        (elapsed, (rows, ()))
    });
    let predicate = Timing::of(&predicate_samples);
    let (query, query_rows) = repeat(ITERATIONS, || engine_scan_full(&path));
    assert_eq!(query_rows, decodes.survivors, "the scan and the prototype agree");
    print_scan(&scan, &predicate, &query, decodes.survivors);

    if let Some(out) = out {
        write_report(&out, &path, &groups, &decodes, &scan, &predicate, &query);
    }
}

#[cfg(test)]
mod tests {
    //! The probe's decoder, checked against the format.
    //!
    //! The probe's decoder is an instrument, and an instrument that has not
    //! been checked against the thing it measures against is not an
    //! instrument. These tests check it against the Parquet format's own
    //! reference vector and against hand-built streams.
    //!
    //! The last two are spec 012's requirement, applied to the decoder: a
    //! dictionary whose values are **not sorted**, and whose indices are
    //! **not monotonic**. An implementation that assumed the dictionary was
    //! sorted would pass on the real file and fail on these.

    use super::{mask_from_indices, mask_words, pass_word_for};

    /// The mask the hypothesis builds: one bit per row, out of the indices.
    ///
    /// The answer is a word of bits over the dictionary, and the row's bit
    /// is the answer for the row's index. The highest entries of the word
    /// are the ones a mask over a *byte* would lose.
    #[test]
    fn builds_the_mask_from_the_indices() {
        // The answer: the first entry and the sixty-fourth pass.
        let pass_word = 1u64 | (1u64 << 63);
        let indices = vec![0u32, 1, 63, 62, 0, 1];
        let (_, survivors) = mask_from_indices(&[(pass_word, indices.clone())]);
        assert_eq!(survivors, 3, "the rows whose index passes, and no others");

        // The bits themselves, and not only the count: the rows that pass
        // are the first, the third and the fifth.
        let mut words = vec![0u64; 1];
        mask_words(pass_word, &indices, &mut words);
        assert_eq!(words[0], 0b0000_0000_0000_0000_0000_0000_0001_0101);
    }

    /// An index outside the dictionary's range is masked, not a panic.
    ///
    /// A dictionary holds at most sixty-four entries, so a well-formed file
    /// cannot produce a larger index -- but the mask is what keeps a
    /// malformed one from taking the process down.
    #[test]
    fn masks_an_index_outside_the_dictionary() {
        let mut words = vec![0u64; 1];
        mask_words(1, &[64], &mut words);
        assert_eq!(words[0], 1, "the index is masked to the dictionary's width");
    }

    /// The predicate's answer is per dictionary: an unsorted dictionary
    /// gives a different answer than a sorted one with the same values.
    #[test]
    fn the_predicate_answers_the_dictionary_it_was_given() {
        let flavor = columnar_kernels::best();
        let sorted = [0.0f64, 0.05, 0.06, 0.1];
        let unsorted = [0.06, 0.1, 0.0, 0.05];
        let sorted_word = pass_word_for(&sorted, flavor);
        let unsorted_word = pass_word_for(&unsorted, flavor);
        assert_ne!(
            sorted_word, unsorted_word,
            "the same values in a different order give a different answer"
        );
        // In the sorted dictionary, 0.05 and 0.06 are the entries that pass.
        assert_eq!(sorted_word & 0b0110, 0b0110);
        // In the unsorted one, they are the first and the last.
        assert_eq!(unsorted_word & 0b1001, 0b1001);
    }
}
