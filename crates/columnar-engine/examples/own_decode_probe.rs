//! What would the engine's own reader cost?
//!
//! This is the instrument of **spec 052**, and it is the probe
//! [ADR-0018](../../../docs/adr/0018-the-scan-reads-the-parquet-decode.md)'s decision was
//! taken with -- **run again against the code that is here now**.
//!
//! ```text
//! cargo run --release -p columnar-engine --example own_decode_probe -- data/lineitem.parquet --out report.json
//! ```
//!
//! # The question
//!
//! ADR-0018 prototyped the rewrite -- the parquet crate's own typed column reader, the engine's
//! kernel over the decoded buffer, the engine's own compaction into its own column -- and found it
//! **equal** to what the engine already did. Arrow's whole layer was worth **0.3 ms** of a
//! **15.7 ms** scan, and the decode was the parquet crate's and **the floor**. **So the rewrite was
//! rejected.**
//!
//! **And the refusal is stale.** It was taken on 2026-10-26, and three things moved since, each of
//! which touches the shape it measured:
//!
//! - **the `parquet` crate bumped**, from 54 to **58.4.1**, and the bump moved the decode;
//! - **the default's rule moved**, from a row limit to a **byte budget**, so the engine's own batch
//!   for the narrow shapes is now a **whole row group** -- and this probe never read one;
//! - **the pushdown gained a dictionary path**, and the shape ADR-0018 measured compared the
//!   **expanded** values.
//!
//! # What is measured
//!
//! Every figure is on the **same** file, in the **same** process, warm, and is the median of five
//! runs.
//!
//! | | |
//! |---|---|
//! | **A** | the parquet crate's own decode of the two columns, into plain buffers |
//! | **B** | Arrow's decode of the same two columns |
//! | **C** | A, plus the engine's predicate over the decoded buffer, plus the compaction of the survivors |
//! | **D** | the engine's own scan, as it is today |
//! | **E** | **the engine's own reader**: the page walk, the dictionary, the indices, the predicate over the **dictionary**, and the compaction -- with **no expansion at all** |
//!
//! **E is the shape the rewrite would have.** It is the one that could win, and it is the one the
//! old probe did not measure: the predicate's column is dictionary-encoded, so the engine's own
//! reader would compare the **dictionary** -- eleven values -- and select the rows **by the
//! indices**, and the expansion of six million floats would never happen.
//!
//! **And E is checked against D**: the survivors are compared, value by value, and the probe stops
//! if they disagree. **An instrument that has not been checked against the reference it is
//! measuring against is not an instrument.**
//!
//! # The batch sizes
//!
//! **The rule's own**, and not the old probe's: the engine's default is a **byte budget**, and for
//! the narrow shapes the rule gives a **whole row group** -- **1,000,000** rows. **A decode measured
//! only at small batches cannot say what a whole-row-group batch costs**, and that is the batch the
//! engine now reads with.
//!
//! # The slices and the indexes below
//!
//! They are into buffers this probe has just filled, and their lengths are asserted immediately
//! above every one of them. The lint is allowed here, and the assertion is the reason.

#![allow(clippy::indexing_slicing)]

mod common;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use columnar_core::ComparisonOp;
use columnar_engine::dictionary::{map_mask, mask_to_bools};
use columnar_engine::pushdown::{Pushdown, PushdownPlan};
use columnar_engine::ParquetSource;
use common::{decode_dictionary, decode_indices, for_each_page, repeat, split_v1_page, Scratch};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ProjectionMask;
use parquet::column::page::Page;
use parquet::column::reader::{get_typed_column_reader, ColumnReader};
use parquet::data_type::DoubleType;
use parquet::file::reader::{FileReader, SerializedFileReader};

/// The file's root index of `l_quantity`.
const QUANTITY: usize = 4;
/// The file's root index of `l_discount`.
const DISCOUNT: usize = 6;

/// How many times each measurement is repeated.
const ITERATIONS: usize = 5;

/// The batch sizes the rule produces, and the ones the old probe read.
///
/// **The rule's own are the first three** -- the narrow shapes get a whole row group at one worker,
/// a quarter of one at four, and an eighth at eight -- and the old probe's are kept, so that the
/// two sets can be compared.
const BATCH_ROWS: [usize; 7] = [1_000_000, 500_000, 250_000, 62_500, 4_096, 16_384, 65_536];

/// The predicate's lower bound.
const LOWER: f64 = 0.04;
/// The predicate's upper bound.
const UPPER: f64 = 0.07;

/// The predicates, as the engine pushes them.
fn plan() -> PushdownPlan {
    let mut plan = PushdownPlan::new();
    let mut discount = Pushdown::new(
        DISCOUNT,
        "l_discount",
        ComparisonOp::GtEq,
        columnar_core::ScalarValue::Float64(LOWER),
    );
    discount.and(ComparisonOp::LtEq, columnar_core::ScalarValue::Float64(UPPER));
    plan.push(discount);
    plan
}

/// A typed reader over one column.
type Typed = parquet::column::reader::ColumnReaderImpl<DoubleType>;

/// The reader over one column, typed.
fn typed(reader: ColumnReader) -> Typed {
    get_typed_column_reader::<DoubleType>(reader)
}

/// **A** — the parquet crate's own decode of the two columns.
///
/// The parquet reader decodes a column chunk into a plain buffer; the
/// definition levels say which rows are NULL, and with no NULLs in the file
/// the values are already one per row.
fn parquet_decode(path: &Path, batch_rows: usize) -> (Duration, usize) {
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    let row_groups = reader.metadata().num_row_groups();

    let mut values: Vec<f64> = Vec::with_capacity(batch_rows);
    let mut levels: Vec<i16> = Vec::with_capacity(batch_rows);
    let mut rows = 0usize;

    let started = Instant::now();
    for group in 0..row_groups {
        let group = reader.get_row_group(group).expect("the row group reads");
        for &column in &[DISCOUNT, QUANTITY] {
            let mut reader = typed(group.get_column_reader(column).expect("the column reader"));

            loop {
                values.clear();
                values.reserve(batch_rows);
                levels.clear();
                levels.resize(batch_rows, 0);

                let (records, values_read, levels_read) = reader
                    .read_records(batch_rows, Some(&mut levels), None, &mut values)
                    .expect("the read runs");
                if records == 0 {
                    break;
                }
                assert_eq!(
                    values_read, records,
                    "this probe assumes a file without NULLs; the manifest says null_rate 0.0"
                );
                assert_eq!(levels_read, records, "the levels and the records disagree");
                rows += records;
                if records < batch_rows {
                    break;
                }
            }
        }
    }
    let elapsed = started.elapsed();
    std::hint::black_box(&values);
    (elapsed, rows)
}

/// **C** — the parquet crate's own decode, the engine's predicate over the
/// decoded buffer, and the compaction of the survivors into the engine's own
/// column.
///
/// This is the shape an engine-owned read path would have had in 2026-10:
/// the values are decoded once, compared once, and the survivors are written
/// once.
///
/// The two columns are read **in sequence**, not in lockstep: the discount
/// column of the row group first, with its masks held, and then the quantity
/// column, compacted through them. Reading them in lockstep keeps two page
/// readers alive at once, which is a cost the engine's own path does not pay
/// and which would make the comparison unfair.
fn parquet_decode_and_compact(path: &Path, batch_rows: usize) -> (Duration, Vec<f64>) {
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    let flavor = columnar_kernels::best();
    let row_groups = reader.metadata().num_row_groups();

    let mut values: Vec<f64> = Vec::with_capacity(batch_rows);
    let mut levels: Vec<i16> = Vec::with_capacity(batch_rows);
    let mut masks: Vec<u64> = Vec::new();
    let mut chunk: Vec<f64> = Vec::new();
    let mut lower: Vec<u64> = Vec::new();
    let mut upper: Vec<u64> = Vec::new();

    let started = Instant::now();
    for group in 0..row_groups {
        let group = reader.get_row_group(group).expect("the row group reads");

        // --- the predicate's column: decoded, compared, and dropped --------
        masks.clear();
        {
            let mut reader = typed(group.get_column_reader(DISCOUNT).expect("the column reader"));
            loop {
                values.clear();
                values.reserve(batch_rows);
                levels.clear();
                levels.resize(batch_rows, 0);

                let (records, values_read, levels_read) = reader
                    .read_records(batch_rows, Some(&mut levels), None, &mut values)
                    .expect("the read runs");
                if records == 0 {
                    break;
                }
                assert_eq!(values_read, records, "this probe assumes a file without NULLs");
                assert_eq!(levels_read, records, "the levels and the records disagree");

                let words = records.div_ceil(64);
                lower.clear();
                lower.resize(words, 0);
                upper.clear();
                upper.resize(words, 0);
                columnar_kernels::compare_f64_into(
                    flavor,
                    &values[..records],
                    ComparisonOp::GtEq,
                    LOWER,
                    None,
                    &mut lower,
                );
                columnar_kernels::compare_f64_into(
                    flavor,
                    &values[..records],
                    ComparisonOp::LtEq,
                    UPPER,
                    None,
                    &mut upper,
                );
                masks.extend(lower.iter().zip(upper.iter()).map(|(a, b)| a & b));
                if records < batch_rows {
                    break;
                }
            }
        }

        // --- the materialised column: decoded, and compacted ---------------
        {
            let mut reader = typed(group.get_column_reader(QUANTITY).expect("the column reader"));
            let mut offset = 0usize;
            loop {
                values.clear();
                values.reserve(batch_rows);
                levels.clear();
                levels.resize(batch_rows, 0);

                let (records, values_read, levels_read) = reader
                    .read_records(batch_rows, Some(&mut levels), None, &mut values)
                    .expect("the read runs");
                if records == 0 {
                    break;
                }
                assert_eq!(values_read, records, "this probe assumes a file without NULLs");
                assert_eq!(levels_read, records, "the levels and the records disagree");

                for word_index in 0..records.div_ceil(64) {
                    let base = word_index * 64;
                    let mut bits = masks[offset + word_index];
                    while bits != 0 {
                        let bit = bits.trailing_zeros() as usize;
                        let row = base + bit;
                        if row < records {
                            chunk.push(values[row]);
                        }
                        bits &= bits - 1;
                    }
                }
                offset += records.div_ceil(64);
                if records < batch_rows {
                    break;
                }
            }
        }
    }
    let elapsed = started.elapsed();
    std::hint::black_box(&chunk);
    (elapsed, chunk)
}

/// **B** — Arrow's decode of the same two columns.
///
/// The same work as A, through Arrow's reader instead of the parquet crate's
/// own. Arrow's reader is a thin wrapper over the same code -- it decodes a
/// column chunk into a buffer and *moves* that buffer into an Arrow array --
/// so the difference between A and B is the whole of Arrow's cost.
fn arrow_decode(path: &Path, batch_rows: usize) -> (Duration, usize) {
    let file = File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let mask =
        ProjectionMask::roots(builder.parquet_schema(), [DISCOUNT, QUANTITY].iter().copied());
    let reader = builder
        .with_projection(mask)
        .with_batch_size(batch_rows)
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

/// **D** — the engine's own scan, end to end.
fn engine_scan(path: &Path, batch_rows: Option<usize>) -> (Duration, Vec<f64>) {
    let mut source = ParquetSource::open(path).expect("the source opens");
    if let Some(batch_rows) = batch_rows {
        source = source.with_reader_batch_rows(batch_rows);
    }
    let started = Instant::now();
    let dataset =
        source.read(&[QUANTITY], &plan(), columnar_kernels::best(), 1).expect("the read runs");
    let elapsed = started.elapsed();
    let columnar_core::batch::BatchData::Float64(values) =
        dataset.chunk("l_quantity").expect("the chunk").decode().data
    else {
        panic!("the materialised column is not a float64 column");
    };
    let values = values.clone();
    std::hint::black_box(&dataset);
    (elapsed, values)
}

/// **E** — the engine's own reader.
///
/// The page walk, the dictionary, the indices, the predicate over the
/// **dictionary**, and the compaction -- **with no expansion at all**.
///
/// The predicate's column is dictionary-encoded, so the reader compares the
/// **dictionary** -- eleven values -- once, and the rows are selected **by the
/// indices**. The expansion of six million floats never happens, and the
/// materialised column is gathered through its own dictionary.
///
/// **The map is the engine's own code**: [`map_mask`] is the engine's
/// dictionary path, and the comparison is the engine's own kernel over the
/// dictionary's values.
///
/// **And the mask is the file's rows, in one piece.** The two columns' pages
/// do not end on the same rows, so the mask is placed by **absolute row** and
/// not by page-relative word: a page's words are scattered into the mask at
/// the page's first row, and the materialised column reads the mask by the
/// row it is on. **A mask that assumed the two columns' pages ended together
/// would answer about a file that does not exist.**
///
/// **And the shape comes in two.** With `expand` set, the predicate's column is
/// **expanded** through its dictionary and compared row by row -- which is what
/// the parquet crate does today -- and the reader still owns the page walk.
/// The two are the same reader with and without the expansion, so the
/// difference between them is **the expansion's price**, measured inside the
/// reader that would not pay it.
fn own_reader(path: &Path, expand: bool) -> (Duration, Vec<f64>) {
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    let schema = reader.metadata().file_metadata().schema_descr();
    let rows = usize::try_from(reader.metadata().file_metadata().num_rows()).unwrap_or(0);

    // **Each column's own levels.** The predicate's column is optional and the
    // materialised one is required, so a page of one does not hold the level
    // section of the other.
    let discount_levels =
        (schema.column(DISCOUNT).max_rep_level(), schema.column(DISCOUNT).max_def_level());
    let quantity_levels =
        (schema.column(QUANTITY).max_rep_level(), schema.column(QUANTITY).max_def_level());

    let mut own = Reader::new(expand);
    let mut masks: Vec<u64> = vec![0; rows.div_ceil(64)];
    let mut chunk: Vec<f64> = Vec::new();

    let started = Instant::now();

    // --- the predicate's column: the dictionary, the indices, the mask ------
    let mut offset = 0usize;
    for_each_page(
        path,
        DISCOUNT,
        discount_levels.0,
        discount_levels.1,
        |encoding, dictionary, _levels, values, section| {
            assert_eq!(
                encoding,
                parquet::basic::Encoding::RLE_DICTIONARY,
                "the predicate's column is dictionary-encoded"
            );
            own.scratch.indices.clear();
            own.scratch.indices.resize(values, 0);
            decode_indices(section, values, &mut own.scratch.indices);
            own.signed.clear();
            own.signed
                .extend(own.scratch.indices.iter().map(|&index| i32::try_from(index).unwrap_or(0)));

            let words = own.mask(dictionary, values);
            place(&mut masks, offset, &words);
            offset += values;
        },
    );

    // --- the materialised column: the indices, and the compaction ----------
    let mut offset = 0usize;
    for_each_page(
        path,
        QUANTITY,
        quantity_levels.0,
        quantity_levels.1,
        |encoding, dictionary, _levels, values, section| {
            assert_eq!(
                encoding,
                parquet::basic::Encoding::RLE_DICTIONARY,
                "the materialised column is dictionary-encoded"
            );
            own.scratch.indices.clear();
            own.scratch.indices.resize(values, 0);
            decode_indices(section, values, &mut own.scratch.indices);

            // **The engine's own compaction**: the mask's words, a word at a
            // time, and the survivors it holds -- the same iteration
            // `RowMask::for_each` and `push_masked_float64` make. **The mask
            // is the file's rows**, so the page's rows are cut out of it, and
            // the survivors are gathered through the dictionary.
            Reader::gather(dictionary, &own.scratch.indices, &masks, offset, values, &mut chunk);
            offset += values;
        },
    );

    let elapsed = started.elapsed();
    std::hint::black_box(&chunk);
    (elapsed, chunk)
}

/// **E3** — the engine's own reader, **row group by row group**.
///
/// The same reader as **E**, and the shape the engine's own read path would
/// have: the predicate's column of a row group, then the materialised column
/// of the same row group, with the mask **that row group's** -- 125 KB, not
/// the file's 750 KB -- and the file read **once**, in the order it is laid
/// out.
///
/// **E is the whole file's shape** and it pays twice for it: the mask is the
/// whole file's, and the materialised column is read in a second pass over
/// the file. **So the difference between E and E3 is the shape's own
/// handicap**, measured rather than assumed.
fn own_reader_by_row_group(path: &Path, expand: bool) -> (Duration, Vec<f64>) {
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    let schema = reader.metadata().file_metadata().schema_descr();
    let discount_levels =
        (schema.column(DISCOUNT).max_rep_level(), schema.column(DISCOUNT).max_def_level());
    let quantity_levels =
        (schema.column(QUANTITY).max_rep_level(), schema.column(QUANTITY).max_def_level());

    let mut own = Reader::new(expand);
    let mut chunk: Vec<f64> = Vec::new();
    let mut level_buffer: Vec<u16> = Vec::new();

    let started = Instant::now();
    for group_index in 0..reader.num_row_groups() {
        let group = reader.get_row_group(group_index).expect("the row group reads");
        let group_rows =
            usize::try_from(reader.metadata().row_group(group_index).num_rows()).unwrap_or(0);
        let mut masks: Vec<u64> = vec![0; group_rows.div_ceil(64)];

        // The predicate's column of this row group, and then the materialised
        // column of the same row group: the file is read **once**, in the order
        // it is laid out, and the mask is **that row group's**.
        own.predicate_phase(group.as_ref(), discount_levels, &mut level_buffer, &mut masks);
        own.materialised_phase(
            group.as_ref(),
            quantity_levels,
            &mut level_buffer,
            &masks,
            &mut chunk,
        );
    }
    let elapsed = started.elapsed();
    std::hint::black_box(&chunk);
    (elapsed, chunk)
}

/// Places a page's mask words into the mask, at the page's first row.
///
/// The page's words are page-relative; the mask's are absolute. **The page
/// does not have to start on a word boundary**, so the words are scattered
/// and the one that straddles the boundary is split.
fn place(masks: &mut [u64], page_start: usize, words: &[u64]) {
    for (index, &word) in words.iter().enumerate() {
        let first = page_start + index * 64;
        let word_index = first / 64;
        let shift = first % 64;
        if let Some(slot) = masks.get_mut(word_index) {
            *slot |= word << shift;
        }
        if shift != 0 {
            if let Some(slot) = masks.get_mut(word_index + 1) {
                *slot |= word >> (64 - shift);
            }
        }
    }
}

/// The reader's own state, across the pages of a read.
///
/// **The scratch is reused**: a decode that allocates a buffer a page measures
/// the allocator as much as the decoder.
struct Reader {
    /// The decoded indices, and the expanded values.
    scratch: Scratch,
    /// The indices, signed -- the engine's own map takes them signed.
    signed: Vec<i32>,
    /// The lower and upper bounds' masks.
    lower: Vec<u64>,
    /// The upper bound's mask.
    upper: Vec<u64>,
    /// Whether the predicate's column is expanded.
    expand: bool,
    /// The kernel's flavor.
    flavor: columnar_kernels::Flavor,
}

impl Reader {
    /// A reader over the pages of a read.
    fn new(expand: bool) -> Self {
        Self {
            scratch: Scratch::new(1_000_000),
            signed: Vec::with_capacity(1_000_000),
            lower: Vec::new(),
            upper: Vec::new(),
            expand,
            flavor: columnar_kernels::best(),
        }
    }

    /// A page's mask: the rows of the page that match the predicate.
    ///
    /// **Two shapes**, and the difference between them is the whole of the
    /// idea:
    ///
    /// - **expanded**: the indices gathered through the dictionary, and the
    ///   values compared **row by row** -- which is what the parquet crate
    ///   does today, because it hands the engine an expanded array;
    /// - **not expanded**: the **dictionary** compared once -- eleven values --
    ///   and the answer **mapped through the indices**, so the expansion of
    ///   six million floats never happens.
    fn mask(&mut self, dictionary: &[f64], values: usize) -> Vec<u64> {
        if !self.expand {
            // No expansion: the dictionary compared once, and the answer mapped
            // through the indices. **The map is the engine's own code.**
            let distinct = dictionary_matches(dictionary);
            return map_mask(&distinct, &self.signed, values);
        }

        // The expansion: the indices gathered through the dictionary, and the
        // values compared row by row.
        self.scratch.values.clear();
        self.scratch.values.resize(values, 0.0);
        for row in 0..values {
            let entry = u32::try_from(self.signed[row]).unwrap_or(0);
            self.scratch.values[row] = dictionary[usize::try_from(entry).unwrap_or(0)];
        }
        let word_count = values.div_ceil(64);
        self.lower.clear();
        self.lower.resize(word_count, 0);
        self.upper.clear();
        self.upper.resize(word_count, 0);
        columnar_kernels::compare_f64_into(
            self.flavor,
            &self.scratch.values,
            ComparisonOp::GtEq,
            LOWER,
            None,
            &mut self.lower,
        );
        columnar_kernels::compare_f64_into(
            self.flavor,
            &self.scratch.values,
            ComparisonOp::LtEq,
            UPPER,
            None,
            &mut self.upper,
        );
        self.lower.iter().zip(self.upper.iter()).map(|(a, b)| a & b).collect()
    }

    /// The predicate's column of one row group: the dictionary, the indices,
    /// and the mask.
    fn predicate_phase(
        &mut self,
        group: &dyn parquet::file::reader::RowGroupReader,
        levels: (i16, i16),
        level_buffer: &mut Vec<u16>,
        masks: &mut [u64],
    ) {
        let mut pages = group.get_column_page_reader(DISCOUNT).expect("the page reader opens");
        let mut dictionary: Vec<f64> = Vec::new();
        let mut offset = 0usize;
        while let Some(page) = pages.get_next_page().expect("the page reads") {
            match page {
                Page::DictionaryPage { buf, num_values, .. } => {
                    dictionary = decode_dictionary(&buf, num_values);
                }
                Page::DataPage { buf, num_values, encoding, .. } => {
                    let rows = num_values as usize;
                    let (values, section) =
                        split_v1_page(&buf, levels.0, levels.1, rows, level_buffer);
                    assert_eq!(values, rows, "this probe assumes a file without NULLs");
                    assert_eq!(
                        encoding,
                        parquet::basic::Encoding::RLE_DICTIONARY,
                        "the predicate's column is dictionary-encoded"
                    );
                    self.scratch.indices.clear();
                    self.scratch.indices.resize(values, 0);
                    decode_indices(section, values, &mut self.scratch.indices);
                    self.signed.clear();
                    self.signed.extend(
                        self.scratch.indices.iter().map(|&index| i32::try_from(index).unwrap_or(0)),
                    );
                    let words = self.mask(&dictionary, values);
                    place(masks, offset, &words);
                    offset += values;
                }
                Page::DataPageV2 { .. } => {
                    panic!("the file holds a v2 data page, which this probe does not read");
                }
            }
        }
    }

    /// The materialised column of one row group: the indices, and the
    /// compaction of the survivors.
    fn materialised_phase(
        &mut self,
        group: &dyn parquet::file::reader::RowGroupReader,
        levels: (i16, i16),
        level_buffer: &mut Vec<u16>,
        masks: &[u64],
        chunk: &mut Vec<f64>,
    ) {
        let mut pages = group.get_column_page_reader(QUANTITY).expect("the page reader opens");
        let mut dictionary: Vec<f64> = Vec::new();
        let mut offset = 0usize;
        while let Some(page) = pages.get_next_page().expect("the page reads") {
            match page {
                Page::DictionaryPage { buf, num_values, .. } => {
                    dictionary = decode_dictionary(&buf, num_values);
                }
                Page::DataPage { buf, num_values, encoding, .. } => {
                    let rows = num_values as usize;
                    let (values, section) =
                        split_v1_page(&buf, levels.0, levels.1, rows, level_buffer);
                    assert_eq!(values, rows, "this probe assumes a file without NULLs");
                    assert_eq!(
                        encoding,
                        parquet::basic::Encoding::RLE_DICTIONARY,
                        "the materialised column is dictionary-encoded"
                    );
                    self.scratch.indices.clear();
                    self.scratch.indices.resize(values, 0);
                    decode_indices(section, values, &mut self.scratch.indices);
                    Self::gather(&dictionary, &self.scratch.indices, masks, offset, values, chunk);
                    offset += values;
                }
                Page::DataPageV2 { .. } => {
                    panic!("the file holds a v2 data page, which this probe does not read");
                }
            }
        }
    }

    /// A page's survivors, gathered through the dictionary and pushed into the
    /// column.
    ///
    /// **The engine's own compaction**: the mask's words, a word at a time, and
    /// the survivors they hold -- the same iteration `RowMask::for_each` and
    /// `push_masked_float64` make. **The mask is the file's rows**, so the
    /// page's rows are cut out of it.
    fn gather(
        dictionary: &[f64],
        indices: &[u32],
        masks: &[u64],
        offset: usize,
        values: usize,
        chunk: &mut Vec<f64>,
    ) {
        let first = offset / 64;
        let last = (offset + values - 1) / 64;
        for (index, &word) in masks.iter().enumerate().take(last + 1).skip(first) {
            let mut bits = word;
            let base = index * 64;
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                let row = base + bit;
                if row >= offset && row < offset + values {
                    let entry = indices[row - offset];
                    let value = dictionary[usize::try_from(entry).unwrap_or(0)];
                    chunk.push(value);
                }
                bits &= bits - 1;
            }
        }
    }
}

/// The dictionary's answer: one boolean a dictionary entry.
///
/// **The engine's own kernel**, over the dictionary's values -- the same
/// comparison the engine makes over a plain column, made once a distinct
/// value instead of once a row.
fn dictionary_matches(dictionary: &[f64]) -> Vec<bool> {
    let flavor = columnar_kernels::best();
    let mut lower: Vec<u64> = vec![0; dictionary.len().div_ceil(64)];
    let mut upper: Vec<u64> = vec![0; dictionary.len().div_ceil(64)];
    columnar_kernels::compare_f64_into(
        flavor,
        dictionary,
        ComparisonOp::GtEq,
        LOWER,
        None,
        &mut lower,
    );
    columnar_kernels::compare_f64_into(
        flavor,
        dictionary,
        ComparisonOp::LtEq,
        UPPER,
        None,
        &mut upper,
    );
    let merged: Vec<u64> = lower.iter().zip(upper.iter()).map(|(a, b)| a & b).collect();
    mask_to_bools(&merged)
}

/// The commit the working tree is at, for the report.
fn revision() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or_else(|| "unknown".to_owned(), |text| text.trim().to_owned())
}

/// The host's load average, for the report.
///
/// The house's own reading of it: `sysctl` prints the three averages inside
/// braces, and the first number is the one minute's.
fn load_average() -> f64 {
    let output = std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .ok()
        .filter(|output| output.status.success());
    output.map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).map_or_else(
        || 0.0,
        |text| {
            text.replace(['{', '}'], " ")
                .split_whitespace()
                .next()
                .and_then(|first| first.parse::<f64>().ok())
                .unwrap_or(0.0)
        },
    )
}

/// Where the report is written.
fn output_path() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--out" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

/// The shapes at each batch size: A, B, C and D.
///
/// **And C is checked against D** -- the old shape and the engine's own scan
/// must agree on every survivor, or the probe stops.
fn measure_the_batches(path: &Path) -> Vec<serde_json::Value> {
    let mut measurements: Vec<serde_json::Value> = Vec::new();
    for batch_rows in BATCH_ROWS {
        println!("--- batch {batch_rows} rows ---");

        let (a, a_rows) = repeat(ITERATIONS, || parquet_decode(path, batch_rows));
        println!("  A  the parquet crate's decode  : {:7.2} ms   ({a_rows} rows)", a.median);
        let (b, b_rows) = repeat(ITERATIONS, || arrow_decode(path, batch_rows));
        println!("  B  Arrow's decode              : {:7.2} ms   ({b_rows} rows)", b.median);
        let (c, c_values) = repeat(ITERATIONS, || parquet_decode_and_compact(path, batch_rows));
        println!(
            "  C  the decode and the compaction: {:5.2} ms   ({} rows survive)",
            c.median,
            c_values.len()
        );
        let (d, d_values) = repeat(ITERATIONS, || engine_scan(path, Some(batch_rows)));
        println!(
            "  D  the engine's own scan       : {:7.2} ms   ({} rows survive)",
            d.median,
            d_values.len()
        );

        // **C is checked against D.** The same rows, the same values.
        assert_eq!(c_values, d_values, "the old shape and the engine's scan disagree");

        measurements.push(serde_json::json!({
            "batch_rows": batch_rows,
            "a": a.median,
            "b": b.median,
            "c": c.median,
            "d": d.median,
            "the_spreads": {
                "a": [a.min, a.max],
                "b": [b.min, b.max],
                "c": [c.min, c.max],
                "d": [d.min, d.max],
            },
            "rows": a_rows,
            "survivors": d_values.len(),
        }));
        println!();
    }
    measurements
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "data/lineitem.parquet".to_owned());
    let path = Path::new(&path);

    println!("file: {}", path.display());
    println!("iterations: {ITERATIONS}");
    println!("load average: {:.2}", load_average());
    println!();

    // --- the shapes, at each batch size -----------------------------------
    let measurements = measure_the_batches(path);

    // --- the engine's own reader, and the engine's own default -------------
    println!("--- the engine's own reader ---");
    let (e, e_values) = repeat(ITERATIONS, || own_reader(path, false));
    println!(
        "  E  the engine's own reader     : {:7.2} ms   ({} rows survive)",
        e.median,
        e_values.len()
    );
    let (e2, e2_values) = repeat(ITERATIONS, || own_reader(path, true));
    println!(
        "  E2 the same reader, expanded   : {:7.2} ms   ({} rows survive)",
        e2.median,
        e2_values.len()
    );
    let (e3, e3_values) = repeat(ITERATIONS, || own_reader_by_row_group(path, false));
    println!(
        "  E3 the same reader, by row group: {:7.2} ms   ({} rows survive)",
        e3.median,
        e3_values.len()
    );
    let (e4, e4_values) = repeat(ITERATIONS, || own_reader_by_row_group(path, true));
    println!(
        "  E4 that one, expanded            : {:7.2} ms   ({} rows survive)",
        e4.median,
        e4_values.len()
    );
    let (d_default, d_default_values) = repeat(ITERATIONS, || engine_scan(path, None));
    println!(
        "  D  the engine's own scan       : {:7.2} ms   ({} rows survive)",
        d_default.median,
        d_default_values.len()
    );

    // **E is checked against D.** The same rows, the same values.
    assert_eq!(
        e_values, d_default_values,
        "the engine's own reader and the engine's scan disagree"
    );
    assert_eq!(e2_values, d_default_values, "the expanded reader and the engine's scan disagree");
    assert_eq!(e3_values, d_default_values, "the row-group reader and the engine's scan disagree");
    assert_eq!(e4_values, d_default_values, "the expanded row-group reader disagrees");

    let report = serde_json::json!({
        "the_probe": "own_decode_probe",
        "the_spec": "052",
        "the_file": path.display().to_string(),
        "the_revision": revision(),
        "the_iterations": ITERATIONS,
        "the_host_load_average": load_average(),
        "the_batch_rows": BATCH_ROWS,
        "the_measurements": measurements,
        "the_engine_s_own_reader": {
            "e": e.median,
            "the_e_spread": [e.min, e.max],
            "e2": e2.median,
            "the_e2_spread": [e2.min, e2.max],
            "e3": e3.median,
            "the_e3_spread": [e3.min, e3.max],
            "e4": e4.median,
            "the_e4_spread": [e4.min, e4.max],
            "the_engine_s_own_default": d_default.median,
            "the_default_s_spread": [d_default.min, d_default.max],
            "survivors": d_default_values.len(),
        },
    });

    println!("--- the report ---");
    println!("{report}");
    if let Some(out) = output_path() {
        std::fs::write(&out, format!("{report}\n")).expect("the report is written");
        println!("report: {}", out.display());
    }
}
