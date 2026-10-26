//! The materialised column, taken apart.
//!
//! This is the measurement behind spec 014, and it is a diagnostic, not part
//! of the engine.
//!
//! ```text
//! cargo run --release -p columnar-engine --example materialised_column_probe -- data/lineitem.parquet --out report.json
//! ```
//!
//! # The question
//!
//! Spec 012 took the **predicate's** column apart and found that the decode's
//! cost was the expansion of six million RLE dictionary indices -- work the
//! engine threw away, because the predicate's column is expanded, compared row
//! by row, and dropped.
//!
//! But the predicate's column is not the query. On the real Q6 the query also
//! **materialises** a column: `l_extendedprice`, the column the revenue is
//! computed from. Nobody has measured what that costs, and nobody has
//! measured what it is made of.
//!
//! So this probe does what spec 012's probe did, with the same instrument,
//! on the column that matters more:
//!
//! | | |
//! |---|---|
//! | **P** | the parquet crate's own decode of the column |
//! | **A** | Arrow's decode of the column: what the engine's reader does |
//! | **V** | the probe's own decode |
//! | **I** | the probe's decode of the indices **only**: no gather, no values |
//! | **G** | the expansion: the reader's fresh buffers, and the gather |
//! | **G1** | the reader's fresh buffers, and their zeroing |
//! | **G2** | the gather alone, into a buffer that is already there |
//! | **X** | the **counterfactual**: the same column, written the other way |
//!
//! The counterfactual is the part that is new, and it is the part that
//! answers the question. A dictionary-encoded column can be written **plain**,
//! and a plain column can be written **with a dictionary**. One of the two is
//! a loss, and the probe measures which -- on the real bytes, through the
//! real readers, with the answers checked bit for bit.
//!
//! # The three columns
//!
//! The real Q6 reads three numeric columns and one string column:
//!
//! - **`l_discount`** -- a predicate column *and* an input to the aggregate.
//!   It is dictionary-encoded, with eleven values.
//! - **`l_quantity`** -- a predicate column. It is dictionary-encoded, with
//!   about five thousand values, so its indices are thirteen bits wide and
//!   its gather is not a lookup in a cache line.
//! - **`l_extendedprice`** -- the **materialised** column. The query's output
//!   is computed from it.
//!
//! The probe takes all three apart with the same decomposition, and then runs
//! the **real Q6** through the engine's own lazy path, so the numbers are
//! comparable with the published tables.
//!
//! # The instrument is checked first
//!
//! The probe's own decoder is compared with the parquet crate's, bit for bit,
//! over every value of every row group, **before** anything is measured. An
//! instrument that has not been checked against the reference it is measuring
//! against is not an instrument.

// Every slice and index in this probe is into a buffer the probe has just
// filled, and every length is asserted immediately above the index that uses
// it. The lint is allowed here, and the assertion is the reason.
#![allow(clippy::indexing_slicing)]

mod common;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use arrow::array::{ArrayRef, Float64Array, RecordBatch};
use arrow::datatypes::{DataType as ArrowType, Field, Schema};
use columnar_core::{ComparisonOp, ScalarValue};
use columnar_engine::pushdown::{Pushdown, PushdownPlan};
use columnar_engine::{execute_on, ExecOptions, ParquetSource};
use common::{decode_indices, for_each_page, repeat, split_v1_page, Scratch, Timing};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::arrow::ProjectionMask;
use parquet::basic::{Compression, Encoding};
use parquet::column::page::Page;
use parquet::column::reader::get_typed_column_reader;
use parquet::data_type::DoubleType;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use parquet::file::reader::{FileReader, SerializedFileReader};

/// The file's root index of `l_quantity`.
const QUANTITY: usize = 4;
/// The file's root index of `l_extendedprice`.
const EXTENDED_PRICE: usize = 5;
/// The file's root index of `l_discount`.
const DISCOUNT: usize = 6;
/// The file's root index of `l_shipdate`.
const SHIPDATE: usize = 10;

/// How many times each measurement is repeated.
const ITERATIONS: usize = 5;

/// The rows the parquet reader hands over at a time.
const BATCH_ROWS: usize = 65_536;

/// The rows per row group, which is the dataset generator's constant.
const ROW_GROUP_ROWS: usize = 1_000_000;

/// The dictionary page size limit the dataset generator's writer used.
///
/// The default is one megabyte, and it is the reason the materialised column
/// is written plain: its dictionary is a little larger than that, and the
/// writer gave up on it.
const WRITER_DICTIONARY_LIMIT: usize = 1024 * 1024;

/// What a column is to the query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// A predicate column: the engine compares its values and throws most of
    /// them away.
    Predicate,
    /// The materialised column: the query's output is computed from it.
    Materialised,
}

impl Role {
    /// The role's name, for the report.
    fn name(self) -> &'static str {
        match self {
            Self::Predicate => "predicate",
            Self::Materialised => "materialised",
        }
    }
}

/// A column the probe takes apart.
#[derive(Debug, Clone, Copy)]
struct ColumnSpec {
    /// The file's root index of the column.
    index: usize,
    /// The column's name.
    name: &'static str,
    /// What the column is to the query.
    role: Role,
}

/// The columns of the real Q6 that the probe takes apart.
const COLUMNS: &[ColumnSpec] = &[
    ColumnSpec { index: DISCOUNT, name: "l_discount", role: Role::Predicate },
    ColumnSpec { index: QUANTITY, name: "l_quantity", role: Role::Predicate },
    ColumnSpec { index: EXTENDED_PRICE, name: "l_extendedprice", role: Role::Materialised },
];

// ---------------------------------------------------------------------------
// The shape of a column
// ---------------------------------------------------------------------------

/// What a column's chunk looks like on disk.
#[derive(Debug, Clone)]
struct ColumnShape {
    /// The encoding of the **last** data page.
    ///
    /// A column's chunk is not necessarily written in one encoding: the
    /// writer starts with a dictionary, and when the dictionary page reaches
    /// its size limit it falls back to plain for the rest of the chunk. So
    /// this is the encoding the column *ends* in, and the page counts below
    /// are what says how it got there.
    encoding: Encoding,
    /// How many data pages are dictionary-encoded.
    dictionary_pages: usize,
    /// How many data pages are plain.
    plain_pages: usize,
    /// How many values the dictionary holds, at the end of the chunk.
    dictionary_values: usize,
    /// The widest the indices get.
    bit_width: u8,
    /// How many bytes the dictionary-encoded pages hold.
    dictionary_data_bytes: usize,
    /// How many bytes the plain pages hold.
    plain_data_bytes: usize,
    /// How many bytes the dictionary pages hold.
    dictionary_bytes: usize,
    /// How many bytes the column holds **on disk**, compressed.
    ///
    /// This is the column's own size in the file, and it is what the
    /// counterfactual is measured against: the counterfactual's file size and
    /// this number are the same thing, measured the same way.
    compressed_bytes: usize,
    /// How many rows the column holds.
    rows: usize,
    /// The column's maximum repetition and definition levels.
    levels: (i16, i16),
}

impl ColumnShape {
    /// How many bytes the data pages hold, in all.
    fn data_bytes(&self) -> usize {
        self.dictionary_data_bytes + self.plain_data_bytes
    }
}

/// Reads a column's shape: its encoding, its dictionary, and its level bounds.
fn read_shape(path: &Path, column: usize) -> ColumnShape {
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    let schema = reader.metadata().file_metadata().schema_descr();
    let max_def_level = schema.column(column).max_def_level();
    let max_rep_level = schema.column(column).max_rep_level();

    let mut encoding = Encoding::PLAIN;
    let mut dictionary_pages = 0usize;
    let mut plain_pages = 0usize;
    let mut dictionary_values = 0usize;
    let mut bit_width = 0u8;
    let mut dictionary_data_bytes = 0usize;
    let mut plain_data_bytes = 0usize;
    let mut dictionary_bytes = 0usize;
    let mut compressed_bytes = 0usize;
    let mut rows = 0usize;
    let mut levels: Vec<u16> = Vec::new();

    for group_number in 0..reader.num_row_groups() {
        let group = reader.get_row_group(group_number).expect("the row group reads");
        compressed_bytes += usize::try_from(
            reader.metadata().row_group(group_number).column(column).compressed_size(),
        )
        .expect("a column's compressed size fits in a usize");
        let mut pages = group.get_column_page_reader(column).expect("the page reader opens");
        while let Some(page) = pages.get_next_page().expect("the page reads") {
            match page {
                Page::DictionaryPage { buf, num_values, .. } => {
                    dictionary_values = num_values as usize;
                    dictionary_bytes += buf.len();
                }
                Page::DataPage { buf, num_values, encoding: page_encoding, .. } => {
                    encoding = page_encoding;
                    let page_rows = num_values as usize;
                    let (_, section) =
                        split_v1_page(&buf, max_rep_level, max_def_level, page_rows, &mut levels);
                    if page_encoding == Encoding::RLE_DICTIONARY {
                        dictionary_pages += 1;
                        dictionary_data_bytes += section.len();
                        bit_width = bit_width.max(section[0]);
                    } else {
                        plain_pages += 1;
                        plain_data_bytes += section.len();
                    }
                    rows += page_rows;
                }
                Page::DataPageV2 { .. } => panic!("the file holds a v2 data page"),
            }
        }
    }

    ColumnShape {
        encoding,
        dictionary_pages,
        plain_pages,
        dictionary_values,
        bit_width,
        dictionary_data_bytes,
        plain_data_bytes,
        dictionary_bytes,
        compressed_bytes,
        rows,
        levels: (max_rep_level, max_def_level),
    }
}

// ---------------------------------------------------------------------------
// The decodes
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

/// The probe's own decode of one column.
///
/// The same work the parquet crate does: the pages are fetched and
/// decompressed, and then the values are produced -- by decoding the RLE
/// indices and gathering the dictionary through them, or by copying the
/// values out of a plain page.
fn probe_decode(
    path: &Path,
    column: usize,
    shape: &ColumnShape,
    scratch: &mut Scratch,
) -> (Duration, Vec<f64>) {
    scratch.values.clear();
    let (max_rep_level, max_def_level) = shape.levels;

    let started = Instant::now();
    for_each_page(
        path,
        column,
        max_rep_level,
        max_def_level,
        |encoding, dictionary, _levels, values, section| {
            if encoding == Encoding::RLE_DICTIONARY {
                decode_indices(section, values, &mut scratch.indices);
                scratch.values.extend(
                    scratch.indices[..values].iter().map(|&index| dictionary[index as usize]),
                );
            } else {
                scratch.values.extend(plain_page_values(section, values));
            }
        },
    );
    let elapsed = started.elapsed();
    let values = scratch.values.clone();
    (elapsed, values)
}

/// The probe's own decode of one column's **indices only**.
///
/// The pages are fetched and decompressed, and the indices are decoded. The
/// dictionary is never touched. This is the decode the hypothesis proposes --
/// and it only exists for a dictionary-encoded column, because a plain column
/// has no indices.
fn probe_indices(
    path: &Path,
    column: usize,
    shape: &ColumnShape,
    scratch: &mut Scratch,
) -> Option<(Duration, usize)> {
    if shape.encoding != Encoding::RLE_DICTIONARY {
        return None;
    }
    let (max_rep_level, max_def_level) = shape.levels;
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
    Some((elapsed, count))
}

// ---------------------------------------------------------------------------
// The expansion
// ---------------------------------------------------------------------------

/// Which part of the expansion to measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpansionPart {
    /// The reader's fresh buffers, and their zeroing.
    Buffers,
    /// The gather alone.
    Gather,
    /// Both.
    Both,
}

/// The values of one plain page.
///
/// A plain page of doubles is eight bytes per value, little-endian, and the
/// page holds exactly as many of them as it says.
fn plain_page_values(section: &[u8], count: usize) -> impl Iterator<Item = f64> + '_ {
    assert_eq!(section.len(), count * 8, "a plain page of doubles is eight bytes each");
    section.chunks_exact(8).map(|bytes| f64::from_le_bytes(bytes.try_into().expect("eight bytes")))
}

/// A phase's three parts: its dictionary, its indices, and its values.
///
/// A phase is one or the other -- a dictionary phase has no values, and a
/// plain phase has no indices -- and the three travel together so that the
/// expansion does not have to know which it is holding.
type Parts<'a> = (Option<&'a [f64]>, Option<&'a [u32]>, Option<&'a [f64]>);

/// A run of pages that share one dictionary, or that are plain.
///
/// A chunk is not written in one encoding: the writer starts with a
/// dictionary and falls back to plain when the dictionary page reaches its
/// size limit. So the column is a sequence of **phases**, and each phase is
/// either a run of dictionary pages -- with the dictionary they index into --
/// or a run of plain pages.
enum Phase {
    /// Dictionary-encoded pages, and the indices into their dictionary.
    Dictionary {
        /// The dictionary's values.
        dictionary: Vec<f64>,
        /// The indices, one per row.
        indices: Vec<u32>,
    },
    /// Plain pages, and the values they hold.
    Plain {
        /// The values.
        values: Vec<f64>,
    },
}

impl Phase {
    /// The phase's three parts.
    fn parts(&self) -> Parts<'_> {
        match self {
            Self::Dictionary { dictionary, indices } => (Some(dictionary), Some(indices), None),
            Self::Plain { values } => (None, None, Some(values)),
        }
    }
}

/// What the expansion costs, in the three parts it is made of.
///
/// Arrow's reader does two things the parquet crate's own column reader does
/// not, and both of them are part of what it costs to materialise the values:
///
/// - It **allocates a fresh buffer for every batch**. The record reader hands
///   its values buffer to the array it builds and takes an empty one, so the
///   buffer is new memory every time.
/// - It **zeroes that buffer before the decoder writes into it**, because
///   `ColumnValueDecoderImpl::read` resizes the output buffer with the
///   default value.
///
/// So the expansion is three things, and this measures all three: the
/// allocation and the zeroing (**G1**), the gather itself (**G2**), and the
/// two together (**G**).
fn expansion(phases: &[Phase], part: ExpansionPart) -> (Duration, usize) {
    let mut written = 0usize;
    let mut buffer: Vec<f64> = Vec::new();
    let started = Instant::now();
    for phase in phases {
        let (dictionary, indices, values) = phase.parts();
        let rows = indices.map_or_else(|| values.map_or(0, <[f64]>::len), <[u32]>::len);

        for offset in (0..rows).step_by(BATCH_ROWS) {
            let take = (rows - offset).min(BATCH_ROWS);
            match part {
                ExpansionPart::Buffers => {
                    // A fresh buffer, zeroed, as the reader makes it.
                    let mut fresh: Vec<f64> = Vec::new();
                    fresh.resize(take, 0.0);
                    written += fresh.len();
                    std::hint::black_box(&fresh);
                }
                ExpansionPart::Gather => {
                    // The buffer is already there, already faulted in: the
                    // gather on its own.
                    if buffer.len() < take {
                        buffer.resize(take, 0.0);
                    }
                    fill(&mut buffer[..take], dictionary, indices, values, offset);
                    written += take;
                }
                ExpansionPart::Both => {
                    let mut fresh: Vec<f64> = Vec::new();
                    fresh.resize(take, 0.0);
                    fill(&mut fresh, dictionary, indices, values, offset);
                    written += fresh.len();
                    std::hint::black_box(&fresh);
                }
            }
        }
    }
    let elapsed = started.elapsed();
    (elapsed, written)
}

/// Fills `out` with the values of a phase's rows, from `offset` onwards.
fn fill(
    out: &mut [f64],
    dictionary: Option<&[f64]>,
    indices: Option<&[u32]>,
    values: Option<&[f64]>,
    offset: usize,
) {
    if let Some(indices) = indices {
        let dictionary = dictionary.expect("a dictionary phase has a dictionary");
        for (slot, &index) in out.iter_mut().zip(&indices[offset..]) {
            *slot = dictionary[index as usize];
        }
    } else {
        let values = values.expect("a plain phase has values");
        out.copy_from_slice(&values[offset..offset + out.len()]);
    }
}

/// The column's phases: the dictionary pages, and the plain ones.
fn collect_phases(
    path: &Path,
    column: usize,
    shape: &ColumnShape,
    scratch: &mut Scratch,
) -> Vec<Phase> {
    let (max_rep_level, max_def_level) = shape.levels;
    let mut phases: Vec<Phase> = Vec::new();
    let mut dictionary: Vec<f64> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut plain: Vec<f64> = Vec::new();

    for_each_page(
        path,
        column,
        max_rep_level,
        max_def_level,
        |encoding, page_dictionary, _levels, values, section| {
            if encoding == Encoding::RLE_DICTIONARY {
                // A new dictionary means a new phase: the rows before it
                // index into a different one.
                if !indices.is_empty() && dictionary != page_dictionary {
                    phases.push(Phase::Dictionary {
                        dictionary: std::mem::take(&mut dictionary),
                        indices: std::mem::take(&mut indices),
                    });
                }
                if dictionary.is_empty() {
                    dictionary = page_dictionary.to_vec();
                }
                decode_indices(section, values, &mut scratch.indices);
                indices.extend_from_slice(&scratch.indices[..values]);
            } else {
                if !indices.is_empty() {
                    phases.push(Phase::Dictionary {
                        dictionary: std::mem::take(&mut dictionary),
                        indices: std::mem::take(&mut indices),
                    });
                }
                if !plain.is_empty() {
                    phases.push(Phase::Plain { values: std::mem::take(&mut plain) });
                }
                plain.extend(
                    section
                        .chunks_exact(8)
                        .map(|bytes| f64::from_le_bytes(bytes.try_into().expect("eight bytes"))),
                );
            }
        },
    );
    if !indices.is_empty() {
        phases.push(Phase::Dictionary { dictionary, indices });
    }
    if !plain.is_empty() {
        phases.push(Phase::Plain { values: plain });
    }
    phases
}

// ---------------------------------------------------------------------------
// The counterfactual
// ---------------------------------------------------------------------------

/// How the counterfactual file is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    /// The column, PLAIN: no dictionary at all.
    Plain,
    /// The column, with a dictionary, and a dictionary page big enough to keep
    /// it.
    Dictionary,
}

impl Style {
    /// The style's name, for the report.
    fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Dictionary => "dictionary",
        }
    }
}

/// Writes the same column again, the other way.
///
/// This is the half of the question the other half does not answer: a
/// dictionary-encoded column can be written plain, and a plain column can be
/// written with a dictionary. One of the two is a loss, and the probe measures
/// which -- on the same values, through the same readers.
///
/// The writer's properties are the dataset generator's, so the only
/// difference between this file and the original is the encoding.
fn write_counterfactual(path: &Path, name: &str, style: Style, values: &[f64]) {
    let schema = Arc::new(Schema::new(vec![Field::new(name, ArrowType::Float64, false)]));
    let mut builder = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_max_row_group_size(ROW_GROUP_ROWS)
        .set_statistics_enabled(EnabledStatistics::Chunk)
        .set_writer_version(WriterVersion::PARQUET_1_0);
    builder = match style {
        Style::Plain => builder.set_dictionary_enabled(false).set_encoding(Encoding::PLAIN),
        Style::Dictionary => {
            builder.set_dictionary_enabled(true).set_dictionary_page_size_limit(64 * 1024 * 1024)
        }
    };

    let file = File::create(path).expect("the counterfactual file is created");
    let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(builder.build()))
        .expect("the writer builds");
    for chunk in values.chunks(BATCH_ROWS) {
        let array: ArrayRef = Arc::new(Float64Array::from(chunk.to_vec()));
        let batch = RecordBatch::try_new(schema.clone(), vec![array]).expect("the batch");
        writer.write(&batch).expect("the batch is written");
    }
    writer.close().expect("the file is closed");
}

// ---------------------------------------------------------------------------
// The engine's own measurements
// ---------------------------------------------------------------------------

/// The Q6 predicate set, as the engine pushes it.
fn q6_pushdown() -> PushdownPlan {
    let mut plan = PushdownPlan::new();

    let mut shipdate = Pushdown::new(
        SHIPDATE,
        "l_shipdate",
        ComparisonOp::GtEq,
        ScalarValue::Utf8("1994-01-01".to_owned()),
    );
    shipdate.and(ComparisonOp::Lt, ScalarValue::Utf8("1995-01-01".to_owned()));
    plan.push(shipdate);

    let mut discount =
        Pushdown::new(DISCOUNT, "l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.05));
    discount.and(ComparisonOp::LtEq, ScalarValue::Float64(0.07));
    plan.push(discount);

    plan.push(Pushdown::new(QUANTITY, "l_quantity", ComparisonOp::Lt, ScalarValue::Float64(24.0)));

    plan
}

/// The engine's own scan, with a set of columns materialised.
///
/// Returns the elapsed time, the number of surviving rows, and the time the
/// predicates took.
fn engine_scan(
    path: &Path,
    columns: &[usize],
    pushdown: &PushdownPlan,
) -> (Duration, usize, Duration) {
    let source = ParquetSource::open(path).expect("the source opens");
    let started = Instant::now();
    let dataset =
        source.read(columns, pushdown, columnar_kernels::best(), 1).expect("the read runs");
    let elapsed = started.elapsed();
    let rows = dataset.len();
    let predicate = pushdown.metrics().predicate_time();
    std::hint::black_box(&dataset);
    (elapsed, rows, predicate)
}

/// The engine's own scan, timed.
fn timed_scan(path: &Path, columns: &[usize], pushdown: &PushdownPlan) -> (Duration, usize) {
    let (elapsed, rows, _) = engine_scan(path, columns, pushdown);
    (elapsed, rows)
}

/// The real Q6, through the engine's own lazy path.
///
/// The whole query: the file is read during the query, the predicates are
/// pushed into the reader, and the aggregate is computed over the rows that
/// survived.
fn engine_q6(path: &Path) -> (Duration, f64, f64, f64) {
    let source = ParquetSource::open(path).expect("the source opens");
    let options = ExecOptions { threads: Some(1), ..ExecOptions::default() };
    let definition = columnar_engine::queries::QUERIES
        .iter()
        .find(|definition| definition.query == columnar_engine::BuiltinQuery::Q6Revenue)
        .expect("the revenue query is a builtin query");
    let plan = (definition.plan)();

    let started = Instant::now();
    let result = execute_on(&plan, &source, &options).expect("the query runs");
    let elapsed = started.elapsed();

    let scan = result
        .plan_output
        .stages
        .iter()
        .find(|stage| stage.stage == "scan")
        .map_or(0.0, |stage| stage.elapsed.as_secs_f64() * 1000.0);
    let operators = result
        .plan_output
        .stages
        .iter()
        .filter(|stage| stage.stage != "scan")
        .map(|stage| stage.elapsed.as_secs_f64() * 1000.0)
        .sum::<f64>();
    // The revenue query's answer is a float. An integer or a NULL aggregate
    // is not an answer to it, and it is reported as one rather than
    // converted into one.
    let value =
        result.groups.first().and_then(|group| group.values.first()).map_or(f64::NAN, |value| {
            match value {
                columnar_engine::AggregateValue::Float(value) => *value,
                _ => f64::NAN,
            }
        });

    (elapsed, scan, operators, value)
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

/// Everything the probe measured about one column.
#[derive(Debug)]
struct ColumnDecodes {
    /// The column's name.
    name: &'static str,
    /// What the column is to the query.
    role: Role,
    /// The encoding the column ends in.
    encoding: Encoding,
    /// How many data pages are dictionary-encoded.
    dictionary_pages: usize,
    /// How many data pages are plain.
    plain_pages: usize,
    /// How many values the dictionary holds.
    dictionary_values: usize,
    /// How many bits the indices are packed into.
    bit_width: u8,
    /// How many bytes the dictionary-encoded pages hold.
    dictionary_data_bytes: usize,
    /// How many bytes the plain pages hold.
    plain_data_bytes: usize,
    /// How many bytes the dictionary pages hold.
    dictionary_bytes: usize,
    /// How many bytes the column holds on disk, compressed.
    compressed_bytes: usize,
    /// How many rows the column holds.
    rows: usize,
    /// The parquet crate's own decode.
    parquet: Timing,
    /// Arrow's decode.
    arrow: Timing,
    /// The probe's own decode.
    probe: Timing,
    /// The probe's decode of the indices alone.
    indices: Option<Timing>,
    /// The expansion.
    expansion: Timing,
    /// The reader's fresh buffers.
    buffers: Timing,
    /// The gather alone.
    gather: Timing,
    /// The counterfactual's decode, through the parquet crate.
    counterfactual_parquet: Timing,
    /// The counterfactual's decode, through Arrow.
    counterfactual_arrow: Timing,
    /// The counterfactual's encoding.
    counterfactual_encoding: Encoding,
    /// How many bytes the counterfactual file holds.
    counterfactual_bytes: u64,
    /// The style the counterfactual was written in.
    counterfactual_style: Style,
}

/// Measures one column's decodes.
fn measure_column(path: &Path, spec: &ColumnSpec, scratch: &mut Scratch) -> ColumnDecodes {
    let shape = read_shape(path, spec.index);
    let (parquet, values) = repeat(ITERATIONS, || parquet_decode(path, spec.index));
    let (arrow, _) = repeat(ITERATIONS, || arrow_decode(path, spec.index));
    let (probe, probe_values) =
        repeat(ITERATIONS, || probe_decode(path, spec.index, &shape, scratch));

    // The instrument is checked against the reference before anything is
    // measured: every value, of every row group, bit for bit.
    assert_eq!(values.len(), probe_values.len(), "the same number of values");
    for (index, (left, right)) in values.iter().zip(probe_values.iter()).enumerate() {
        assert_eq!(
            left.to_bits(),
            right.to_bits(),
            "the probe's decode differs from the parquet crate's at value {index}"
        );
    }

    let indices =
        probe_indices(path, spec.index, &shape, scratch).map(|(elapsed, _)| Timing::of(&[elapsed]));
    let (expansion, buffers, gather) = measure_expansion(path, spec.index, &shape, scratch);

    // The counterfactual: the same column, written the other way.
    let style =
        if shape.encoding == Encoding::RLE_DICTIONARY { Style::Plain } else { Style::Dictionary };
    let counterfactual_path =
        PathBuf::from(format!("target/materialised-probe-{}-{}.parquet", spec.name, style.name()));
    if let Some(parent) = counterfactual_path.parent() {
        std::fs::create_dir_all(parent).expect("the target directory");
    }
    write_counterfactual(&counterfactual_path, spec.name, style, &values);
    let counterfactual_bytes =
        std::fs::metadata(&counterfactual_path).expect("the counterfactual file").len();
    let counterfactual_shape = read_shape(&counterfactual_path, 0);
    let (counterfactual_parquet, counterfactual_values) =
        repeat(ITERATIONS, || parquet_decode(&counterfactual_path, 0));
    let (counterfactual_arrow, _) = repeat(ITERATIONS, || arrow_decode(&counterfactual_path, 0));
    assert_eq!(
        counterfactual_values.len(),
        values.len(),
        "the counterfactual holds the same values"
    );
    for (index, (left, right)) in values.iter().zip(counterfactual_values.iter()).enumerate() {
        assert_eq!(
            left.to_bits(),
            right.to_bits(),
            "the counterfactual differs from the original at value {index}"
        );
    }

    ColumnDecodes {
        name: spec.name,
        role: spec.role,
        encoding: shape.encoding,
        dictionary_pages: shape.dictionary_pages,
        plain_pages: shape.plain_pages,
        dictionary_values: shape.dictionary_values,
        bit_width: shape.bit_width,
        dictionary_data_bytes: shape.dictionary_data_bytes,
        plain_data_bytes: shape.plain_data_bytes,
        dictionary_bytes: shape.dictionary_bytes,
        compressed_bytes: shape.compressed_bytes,
        rows: shape.rows,
        parquet,
        arrow,
        probe,
        indices,
        expansion,
        buffers,
        gather,
        counterfactual_parquet,
        counterfactual_arrow,
        counterfactual_encoding: counterfactual_shape.encoding,
        counterfactual_bytes,
        counterfactual_style: style,
    }
}

/// The expansion, in its three parts.
fn measure_expansion(
    path: &Path,
    column: usize,
    shape: &ColumnShape,
    scratch: &mut Scratch,
) -> (Timing, Timing, Timing) {
    let phases = collect_phases(path, column, shape, scratch);
    let (both, _) = repeat(ITERATIONS, || expansion(&phases, ExpansionPart::Both));
    let (buffers, _) = repeat(ITERATIONS, || expansion(&phases, ExpansionPart::Buffers));
    let (gather, _) = repeat(ITERATIONS, || expansion(&phases, ExpansionPart::Gather));
    (both, buffers, gather)
}

/// Everything the probe measured about the query.
#[derive(Debug)]
struct QueryMeasurements {
    /// The Q6's predicates alone: nothing materialised.
    predicates: Timing,
    /// The Q6's predicates, and the materialised column.
    predicates_and_price: Timing,
    /// The Q6's predicates, and the discount column.
    predicates_and_discount: Timing,
    /// The Q6's predicates, and both columns.
    predicates_and_both: Timing,
    /// The real Q6.
    q6: Timing,
    /// The Q6's scan stage.
    q6_scan: f64,
    /// The Q6's operator stages.
    q6_operators: f64,
    /// The Q6's answer.
    q6_value: f64,
    /// How many rows survive the Q6's predicates.
    survivors: usize,
}

/// Measures the query: the ladder, and then the real Q6.
fn measure_query(path: &Path) -> QueryMeasurements {
    let pushdown = q6_pushdown();

    let (predicates, rows) = repeat(ITERATIONS, || timed_scan(path, &[], &pushdown));
    let (predicates_and_price, _) =
        repeat(ITERATIONS, || timed_scan(path, &[EXTENDED_PRICE], &pushdown));
    let (predicates_and_discount, _) =
        repeat(ITERATIONS, || timed_scan(path, &[DISCOUNT], &pushdown));
    let (predicates_and_both, _) =
        repeat(ITERATIONS, || timed_scan(path, &[DISCOUNT, EXTENDED_PRICE], &pushdown));

    let mut q6_scan = 0.0;
    let mut q6_operators = 0.0;
    let mut q6_value = f64::NAN;
    let (q6, ()) = repeat(ITERATIONS, || {
        let (elapsed, scan, operators, value) = engine_q6(path);
        q6_scan = scan;
        q6_operators = operators;
        q6_value = value;
        (elapsed, ())
    });
    let _ = &q6;

    QueryMeasurements {
        predicates,
        predicates_and_price,
        predicates_and_discount,
        predicates_and_both,
        q6,
        q6_scan,
        q6_operators,
        q6_value,
        survivors: rows,
    }
}

// ---------------------------------------------------------------------------
// The output
// ---------------------------------------------------------------------------

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

/// Prints the shape of every column.
fn print_shapes(shapes: &[(String, ColumnShape)]) {
    println!("--- the columns, over the whole file ---");
    println!(
        "{:>16} {:>14} {:>9} {:>9} {:>5} {:>13} {:>13} {:>12}",
        "column",
        "ends in",
        "dict pages",
        "plain pages",
        "bits",
        "dict bytes",
        "plain bytes",
        "dictionary"
    );
    for (name, shape) in shapes {
        println!(
            "{:>16} {:>14} {:>9} {:>9} {:>5} {:>13} {:>13} {:>12}",
            name,
            format!("{:?}", shape.encoding),
            shape.dictionary_pages,
            shape.plain_pages,
            shape.bit_width,
            shape.dictionary_data_bytes,
            shape.plain_data_bytes,
            shape.dictionary_values
        );
        println!(
            "{:>16} the dictionary page holds {} bytes, and the data pages hold {}",
            "",
            shape.dictionary_bytes,
            shape.data_bytes()
        );
    }
    println!();
}

/// Prints one column's decomposition.
fn print_column(decodes: &ColumnDecodes) {
    println!("--- {} ({}) ---", decodes.name, decodes.role.name());
    let rows: [(&str, &str, &Timing); 8] = [
        ("P", "the parquet crate's decode", &decodes.parquet),
        ("A", "Arrow's decode", &decodes.arrow),
        ("V", "the probe's decode", &decodes.probe),
        ("G", "the expansion", &decodes.expansion),
        ("G1", "the reader's fresh buffers", &decodes.buffers),
        ("G2", "the gather, or the copy", &decodes.gather),
        ("X", "the counterfactual, parquet crate", &decodes.counterfactual_parquet),
        ("X", "the counterfactual, Arrow", &decodes.counterfactual_arrow),
    ];
    for (name, label, timing) in rows {
        println!(
            "  {name:<6}{label:<38}{:>7.2} ms  ({:.0}-{:.0})",
            timing.median, timing.min, timing.max
        );
    }
    if let Some(indices) = decodes.indices {
        println!(
            "  {:<6}{:<38}{:>7.2} ms  ({:.0}-{:.0})",
            "I", "the probe's indices, no gather", indices.median, indices.min, indices.max
        );
    } else {
        println!("  {:<6}{:<38}{:>7}", "I", "no indices: the column is plain", "-");
    }
    println!("       the column on disk              : {} bytes", decodes.compressed_bytes);
    println!(
        "       the counterfactual              : {} ({} bytes)",
        decodes.counterfactual_style.name(),
        decodes.counterfactual_bytes
    );
    println!();
}

/// Prints the query's ladder.
fn print_query(query: &QueryMeasurements) {
    println!("--- the real Q6 ---");
    let rows: [(&str, &str, &Timing); 5] = [
        ("Q0", "the predicates alone", &query.predicates),
        ("Q1", "the predicates, and the price", &query.predicates_and_price),
        ("Q2", "the predicates, and the discount", &query.predicates_and_discount),
        ("Q3", "the predicates, and both", &query.predicates_and_both),
        ("Q4", "the whole query", &query.q6),
    ];
    for (name, label, timing) in rows {
        println!(
            "  {name:<6}{label:<38}{:>7.2} ms  ({:.0}-{:.0})",
            timing.median, timing.min, timing.max
        );
    }
    println!(
        "       the materialised column's share : {:7.2} ms   (Q1 - Q0)",
        query.predicates_and_price.median - query.predicates.median
    );
    println!(
        "       the aggregate's own share       : {:7.2} ms   (Q4 - Q3)",
        query.q6.median - query.predicates_and_both.median
    );
    println!("       the scan stage                  : {:7.2} ms", query.q6_scan);
    println!("       the operator stages             : {:7.2} ms", query.q6_operators);
    println!("       the answer                      : {}", query.q6_value);
}

/// Writes the report.
fn write_report(out: &str, path: &Path, decodes: &[ColumnDecodes], query: &QueryMeasurements) {
    let columns: Vec<serde_json::Value> = decodes
        .iter()
        .map(|column| {
            let mut measurements = serde_json::Map::new();
            let mut spreads = serde_json::Map::new();
            let mut timings: Vec<(&str, &Timing)> = vec![
                ("parquet_decode", &column.parquet),
                ("arrow_decode", &column.arrow),
                ("probe_decode", &column.probe),
                ("expansion", &column.expansion),
                ("expansion_buffers", &column.buffers),
                ("expansion_gather", &column.gather),
                ("counterfactual_parquet_decode", &column.counterfactual_parquet),
                ("counterfactual_arrow_decode", &column.counterfactual_arrow),
            ];
            if let Some(indices) = &column.indices {
                timings.push(("probe_indices", indices));
            }
            for (name, timing) in timings {
                measurements.insert(name.to_owned(), serde_json::json!(timing.median));
                spreads.insert(name.to_owned(), serde_json::json!([timing.min, timing.max]));
            }
            serde_json::json!({
                "name": column.name,
                "role": column.role.name(),
                "encoding": format!("{:?}", column.encoding),
                "dictionary_pages": column.dictionary_pages,
                "plain_pages": column.plain_pages,
                "dictionary_data_bytes": column.dictionary_data_bytes,
                "plain_data_bytes": column.plain_data_bytes,
                "dictionary_page_bytes": column.dictionary_bytes,
                "compressed_bytes": column.compressed_bytes,
                "dictionary_values": column.dictionary_values,
                "bit_width": column.bit_width,
                "rows": column.rows,
                "counterfactual_encoding": format!("{:?}", column.counterfactual_encoding),
                "counterfactual_style": column.counterfactual_style.name(),
                "counterfactual_bytes": column.counterfactual_bytes,
                "measurements_ms": measurements,
                "spreads_ms": spreads,
            })
        })
        .collect();

    let report = serde_json::json!({
        "probe": "materialised-column",
        "file": path.display().to_string(),
        "commit": commit(),
        "machine": format!("{} logical cores", cores()),
        "iterations": ITERATIONS,
        "host_load_average": load_average(),
        "writer_dictionary_page_limit": WRITER_DICTIONARY_LIMIT,
        "rows": decodes.first().map_or(0, |column| column.rows),
        "columns": columns,
        "query": {
            "name": "q6-revenue",
            "sql": columnar_engine::queries::QUERIES
                .iter()
                .find(|definition| definition.query == columnar_engine::BuiltinQuery::Q6Revenue)
                .map_or("", |definition| definition.sql),
            "survivors": query.survivors,
            "value": query.q6_value,
            "measurements_ms": {
                "predicates_only": query.predicates.median,
                "predicates_and_price": query.predicates_and_price.median,
                "predicates_and_discount": query.predicates_and_discount.median,
                "predicates_and_both": query.predicates_and_both.median,
                "q6_revenue": query.q6.median,
                "q6_revenue_scan": query.q6_scan,
                "q6_revenue_operators": query.q6_operators,
            },
            "spreads_ms": {
                "predicates_only": [query.predicates.min, query.predicates.max],
                "predicates_and_price": [
                    query.predicates_and_price.min, query.predicates_and_price.max
                ],
                "predicates_and_discount": [
                    query.predicates_and_discount.min, query.predicates_and_discount.max
                ],
                "predicates_and_both": [
                    query.predicates_and_both.min, query.predicates_and_both.max
                ],
                "q6_revenue": [query.q6.min, query.q6.max],
            },
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

    let mut scratch = Scratch::new(1_000_000);
    let mut shapes = Vec::new();
    let mut decodes = Vec::new();
    for spec in COLUMNS {
        shapes.push((spec.name.to_owned(), read_shape(&path, spec.index)));
        decodes.push(measure_column(&path, spec, &mut scratch));
    }
    print_shapes(&shapes);
    for column in &decodes {
        print_column(column);
    }

    let query = measure_query(&path);
    print_query(&query);

    if let Some(out) = out {
        write_report(&out, &path, &decodes, &query);
    }
}

#[cfg(test)]
mod tests {
    use super::{expansion, ExpansionPart, Phase};

    /// A dictionary gather moves the values, and the count says how many.
    #[test]
    fn the_dictionary_gather_moves_every_value() {
        let phase =
            Phase::Dictionary { dictionary: vec![1.0, 2.0, 3.0], indices: vec![0, 2, 1, 2, 0] };
        let (elapsed, written) = expansion(&[phase], ExpansionPart::Gather);
        assert_eq!(written, 5);
        assert!(elapsed.as_nanos() > 0);
    }

    /// A plain column's copy moves every value too.
    #[test]
    fn the_plain_copy_moves_every_value() {
        let phase = Phase::Plain { values: vec![1.0, 2.0, 3.0, 4.0] };
        let (elapsed, written) = expansion(&[phase], ExpansionPart::Both);
        assert_eq!(written, 4);
        assert!(elapsed.as_nanos() > 0);
    }

    /// The gather moves the values the indices point at, not the values in
    /// the order they sit in the dictionary.
    #[test]
    fn the_gather_follows_the_indices() {
        let phase =
            Phase::Dictionary { dictionary: vec![10.0, 20.0, 30.0], indices: vec![2, 0, 1] };
        let mut out = vec![0.0f64; 3];
        super::fill(&mut out, Some(&[10.0, 20.0, 30.0]), Some(&[2, 0, 1]), None, 0);
        assert_eq!(out, vec![30.0, 10.0, 20.0]);
    }
}
