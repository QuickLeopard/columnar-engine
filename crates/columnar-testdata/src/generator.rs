//! Row generation.
//!
//! The generator is a pure function of `(scale_factor, seed, null_rate)`: the
//! same three numbers always produce the same rows, in the same order, with
//! the same bits. Nothing here depends on the wall clock, the thread count,
//! the iteration order of a hash map, or the version of a dependency.
//!
//! Rows are produced in bounded batches. Each batch is materialised as an
//! Arrow record batch and handed to the writer, so the peak footprint of the
//! generator is one batch rather than the whole dataset.
//!
//! ## Value distributions
//!
//! | Column | Distribution |
//! |---|---|
//! | `l_orderkey` | integer in `[1, rows]` |
//! | `l_partkey` | integer in `[1, 5_000_000]` |
//! | `l_suppkey` | integer in `[1, 10_000]` |
//! | `l_linenumber` | integer in `[1, 7]` |
//! | `l_quantity` | decimal in `[1.00, 50.00]` |
//! | `l_extendedprice` | decimal in `[900.00, 105_000.00]` |
//! | `l_discount` | decimal in `[0.00, 0.10]`, NULL at the injected rate |
//! | `l_tax` | decimal in `[0.00, 0.08]` |
//! | `l_returnflag` | one of `A`, `N`, `R` |
//! | `l_linestatus` | one of `F`, `O` |
//! | `l_shipdate` | date in `[1992-01-02, 1998-12-01]` |
//! | `l_commitdate` | `l_shipdate` plus 0..121 days |
//! | `l_receiptdate` | `l_shipdate` plus 1..30 days |
//! | `l_shipinstruct` | one of four TPC-H instructions |
//! | `l_shipmode` | one of seven TPC-H modes |
//! | `l_comment` | free text, NULL at the injected rate |
//!
//! Decimal-valued columns are drawn in integer cents and divided by 100 once,
//! so every value has exactly two decimal places and the aggregate result is
//! reproducible and comparable across runs.
//!
//! ## The null injection
//!
//! The null check is drawn from the PRNG at a fixed point in the per-row
//! sequence, whether or not `null_rate` is zero. The stream of random numbers
//! is therefore identical for every `null_rate`, and the non-null columns of
//! a dataset with an injected null rate hold the same values as those of a
//! dataset without one. Two datasets then differ only in the columns the
//! null rate was supposed to change.
//!
//! ## The second table
//!
//! The dataset holds TPC-H's `orders` beside its `lineitem`: nine columns,
//! a quarter of the rows, and a join key drawn from the same key space as
//! `l_orderkey`. The table has its own PRNG stream, so generating it does
//! not move a single `lineitem` row -- see [`orders_seed`].
//!
//! | Column | Distribution |
//! |---|---|
//! | `o_orderkey` | **distinct** integer in `[1, lineitem_rows]` |
//! | `o_custkey` | integer in `[1, 150_000 * scale_factor]` |
//! | `o_orderstatus` | one of `O`, `F`, `P` |
//! | `o_totalprice` | decimal in `[900.00, 105_000.00]` |
//! | `o_orderdate` | date in `[1992-01-02, 1998-12-01]` |
//! | `o_orderpriority` | one of five TPC-H priorities |
//! | `o_clerk` | integer in `[1, 1_000 * scale_factor]` |
//! | `o_shippriority` | `0` or `1` |
//! | `o_comment` | free text, NULL at the injected rate |
//!
//! The key is distinct because it is the table's primary key, and the
//! join's match rate depends on it: a key that appeared twice would make
//! one `lineitem` row join to two `orders` rows, and the join's row count
//! would stop being a fraction of `lineitem`'s.

use crate::prng::{splitmix64, Xoshiro256};
use crate::schema::{arrow_schema, orders_arrow_schema};
use crate::{DatasetError, DatasetSpec};
use arrow::array::{
    ArrayRef, DictionaryArray, Float64Builder, Int32Array, Int64Array, StringArray, StringBuilder,
};
use arrow::record_batch::RecordBatch;
use std::sync::Arc;

/// The first `l_shipdate` in the dataset.
pub const SHIP_DATE_START: (i32, u32, u32) = (1992, 1, 2);

/// The last `l_shipdate` in the dataset.
pub const SHIP_DATE_END: (i32, u32, u32) = (1998, 12, 1);

/// The largest number of days `l_commitdate` may trail `l_shipdate` by.
pub const COMMIT_DATE_MAX_OFFSET_DAYS: u64 = 121;

/// The smallest number of days `l_receiptdate` may trail `l_shipdate` by.
pub const RECEIPT_DATE_MIN_OFFSET_DAYS: u64 = 1;

/// The largest number of days `l_receiptdate` may trail `l_shipdate` by.
pub const RECEIPT_DATE_MAX_OFFSET_DAYS: u64 = 30;

/// The distinct values of `l_returnflag`.
pub const RETURN_FLAGS: [&str; 3] = ["A", "N", "R"];

/// The distinct values of `l_linestatus`.
pub const LINE_STATUSES: [&str; 2] = ["F", "O"];

/// The distinct values of `l_shipinstruct`.
pub const SHIP_INSTRUCTIONS: [&str; 4] =
    ["DELIVER IN PERSON", "COLLECT COD", "NONE", "TAKE BACK RETURN"];

/// The distinct values of `l_shipmode`.
pub const SHIP_MODES: [&str; 7] = ["REG AIR", "AIR", "RAIL", "SHIP", "TRUCK", "MAIL", "FOB"];

/// The distinct values of `o_orderstatus`.
pub const ORDER_STATUSES: [&str; 3] = ["O", "F", "P"];

/// The distinct values of `o_orderpriority`.
pub const ORDER_PRIORITIES: [&str; 5] =
    ["1-URGENT", "2-HIGH", "3-MEDIUM", "4-NOT SPECIFIED", "5-LOW"];

/// The number of customers per unit of scale factor.
///
/// TPC-H's `customer` table holds 150,000 rows per unit of scale factor,
/// and `o_custkey` is a foreign key into it.
pub const CUSTOMERS_PER_SCALE_FACTOR: u64 = 150_000;

/// The number of clerks per unit of scale factor.
///
/// TPC-H's `clerk` table holds 1,000 rows per unit of scale factor, and
/// `o_clerk` is a foreign key into it.
pub const CLERKS_PER_SCALE_FACTOR: u64 = 1_000;

/// The number of distinct `o_shippriority` values.
const SHIP_PRIORITY_COUNT: u64 = 2;

/// The domain constant that separates the `orders` stream from the
/// `lineitem` stream.
///
/// The value spells `ORDERS_1` in ASCII. It is a fixed constant and not a
/// hash of anything: its only job is to make the two streams unrelated.
const ORDERS_SEED_DOMAIN: u64 = 0x4F52_4445_5253_5F31;

/// The vocabulary `l_comment` is assembled from.
const COMMENT_WORDS: [&str; 32] = [
    "carefully",
    "blithely",
    "quickly",
    "fluffily",
    "slowly",
    "boldly",
    "bravely",
    "deftly",
    "eagerly",
    "finely",
    "gently",
    "hastily",
    "jolly",
    "keenly",
    "lazily",
    "merrily",
    "oddly",
    "promptly",
    "quietly",
    "rapidly",
    "sharply",
    "smoothly",
    "softly",
    "swiftly",
    "tightly",
    "warmly",
    "welly",
    "wildly",
    "zealously",
    "noisily",
    "special",
    "pending",
];

/// The fixed part count, independent of the scale factor.
const PART_COUNT: u64 = 5_000_000;

/// The fixed supplier count, independent of the scale factor.
const SUPPLIER_COUNT: u64 = 10_000;

/// The number of distinct `l_linenumber` values.
const LINE_NUMBER_COUNT: u64 = 7;

/// The length of a fixed-size list of values, as a `u64` bound.
fn len_u64(items: &[&str]) -> u64 {
    u64::try_from(items.len()).expect("the length fits in a u64")
}

/// The number of distinct values in the `l_comment` number.
const COMMENT_NUMBER_RANGE: u64 = 1_000_000;

/// The number of distinct `l_quantity` values, in cents.
const QUANTITY_LOW_CENTS: i32 = 100;
/// The number of distinct `l_quantity` values, in cents.
const QUANTITY_SPAN_CENTS: u64 = 4_901;

/// The smallest `l_extendedprice` value, in cents.
const EXTENDED_PRICE_LOW_CENTS: i32 = 90_000;
/// The number of distinct `l_extendedprice` values, in cents.
const EXTENDED_PRICE_SPAN_CENTS: u64 = 10_410_001;

/// The number of distinct `l_discount` values, in cents.
const DISCOUNT_SPAN_CENTS: u64 = 11;

/// The number of distinct `l_tax` values, in cents.
const TAX_SPAN_CENTS: u64 = 9;

/// An upper bound on the byte length of one `l_comment`, used to size the
/// string builder. The real bound is "furiously " + 6 digits + 2 words of at
/// most 9 characters, which is comfortably under this.
const COMMENT_MAX_BYTES: usize = 48;

/// The day number of a civil date, counted from 1970-01-01.
///
/// This is Howard Hinnant's `days_from_civil`: it is exact for every date in
/// the proleptic Gregorian calendar and needs no lookup table.
#[must_use]
pub fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(year);
    let m = i64::from(month);
    let d = i64::from(day);

    // Shift the year so that the leap day is the last day of the year.
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let year_of_era = y - era * 400;
    let month_of_era = if m > 2 { m - 3 } else { m + 9 };
    let day_of_year = (153 * month_of_era + 2) / 5 + d - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;

    era * 146_097 + day_of_era - 719_468
}

/// The civil date of a day number counted from 1970-01-01.
///
/// The inverse of [`days_from_civil`].
#[must_use]
pub fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_of_era = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_of_era + 2) / 5 + 1;
    let month = if month_of_era < 10 { month_of_era + 3 } else { month_of_era - 9 };
    let year = if month <= 2 { year + 1 } else { year };

    (
        i32::try_from(year).expect("the year fits in an i32"),
        u32::try_from(month).expect("the month fits in a u32"),
        u32::try_from(day).expect("the day fits in a u32"),
    )
}

/// Formats a day number as `YYYY-MM-DD`.
#[must_use]
pub fn format_civil(days: i64) -> String {
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The number of days between the first and the last `l_shipdate`.
#[must_use]
pub fn ship_date_span_days() -> u64 {
    let start = days_from_civil(SHIP_DATE_START.0, SHIP_DATE_START.1, SHIP_DATE_START.2);
    let end = days_from_civil(SHIP_DATE_END.0, SHIP_DATE_END.1, SHIP_DATE_END.2);
    u64::try_from(end - start).expect("the ship date range is positive")
}

/// The table of consecutive dates the date columns are drawn from.
///
/// One table serves every date column of the dataset: the commit date may
/// trail the ship date by 121 days and the receipt date by 30, so the table
/// is that much longer than the ship date range. The `orders` dates cover
/// the same window and are drawn from the same table, so the two tables'
/// dates are the same dates.
fn date_table() -> Vec<String> {
    let start = days_from_civil(SHIP_DATE_START.0, SHIP_DATE_START.1, SHIP_DATE_START.2);
    let span = ship_date_span_days();

    let table_len = span + COMMIT_DATE_MAX_OFFSET_DAYS + 1;
    (0..table_len)
        .map(|offset| {
            let day = start + i64::try_from(offset).expect("the offset fits in an i64");
            format_civil(day)
        })
        .collect()
}

/// The shared dictionary value arrays.
///
/// These are built once and reused by every batch: the Arrow dictionary
/// values are the expensive part of a dictionary-encoded column, and
/// rebuilding them per batch would throw that work away.
#[derive(Debug)]
struct Dictionaries {
    returnflag: Arc<StringArray>,
    linestatus: Arc<StringArray>,
    shipdate: Arc<StringArray>,
    commitdate: Arc<StringArray>,
    receiptdate: Arc<StringArray>,
    shipinstruct: Arc<StringArray>,
    shipmode: Arc<StringArray>,
}

impl Dictionaries {
    fn new() -> Self {
        let span = ship_date_span_days();
        let dates = date_table();

        let ship_len = usize::try_from(span + 1).expect("the ship date count fits in a usize");
        let receipt_len = usize::try_from(span + RECEIPT_DATE_MAX_OFFSET_DAYS + 1)
            .expect("the receipt date count fits in a usize");

        let ship_values =
            dates.get(..ship_len).expect("the ship date slice is inside the date table");
        let receipt_values =
            dates.get(..receipt_len).expect("the receipt date slice is inside the date table");
        let ship = StringArray::from(ship_values.to_vec());
        let receipt = StringArray::from(receipt_values.to_vec());
        let commit = StringArray::from(dates);

        Self {
            returnflag: Arc::new(StringArray::from_iter_values(RETURN_FLAGS)),
            linestatus: Arc::new(StringArray::from_iter_values(LINE_STATUSES)),
            shipdate: Arc::new(ship),
            commitdate: Arc::new(commit),
            receiptdate: Arc::new(receipt),
            shipinstruct: Arc::new(StringArray::from_iter_values(SHIP_INSTRUCTIONS)),
            shipmode: Arc::new(StringArray::from_iter_values(SHIP_MODES)),
        }
    }
}

/// Generates the rows of a dataset.
#[derive(Debug)]
pub struct DatasetGenerator {
    rng: Xoshiro256,
    total_rows: u64,
    emitted: u64,
    null_rate: f64,
    ship_date_count: u64,
    dictionaries: Dictionaries,
}

impl DatasetGenerator {
    /// Builds a generator that will produce `rows` rows.
    ///
    /// # Errors
    ///
    /// Returns [`DatasetError::InvalidSpec`] if `null_rate` is outside
    /// `[0.0, 1.0]` or is not a number at all.
    pub fn new(spec: &DatasetSpec, rows: u64) -> Result<Self, DatasetError> {
        if !(0.0..=1.0).contains(&spec.null_rate) {
            return Err(DatasetError::InvalidSpec(format!(
                "null_rate {} is not in [0.0, 1.0]",
                spec.null_rate
            )));
        }

        Ok(Self {
            rng: Xoshiro256::seed_from_u64(spec.seed),
            total_rows: rows,
            emitted: 0,
            null_rate: spec.null_rate,
            ship_date_count: ship_date_span_days() + 1,
            dictionaries: Dictionaries::new(),
        })
    }

    /// The number of rows generated so far.
    #[must_use]
    pub fn rows_emitted(&self) -> u64 {
        self.emitted
    }

    /// The number of rows this generator will produce in total.
    #[must_use]
    pub fn total_rows(&self) -> u64 {
        self.total_rows
    }

    /// The number of distinct `l_shipdate` values.
    #[must_use]
    pub fn ship_date_count(&self) -> u64 {
        self.ship_date_count
    }

    /// Generates the next batch of at most `len` rows.
    ///
    /// The batch holds the columns in schema order and uses the schema from
    /// [`crate::schema::arrow_schema`].
    ///
    /// # Errors
    ///
    /// Returns [`DatasetError::Parquet`] if the Arrow arrays cannot be
    /// assembled into a record batch.
    pub fn next_batch(&mut self, len: usize) -> Result<RecordBatch, DatasetError> {
        let remaining = self.total_rows.saturating_sub(self.emitted);
        let requested = u64::try_from(len).unwrap_or(u64::MAX);
        let rows = usize::try_from(remaining.min(requested)).unwrap_or(len);

        let mut orderkey: Vec<i64> = Vec::with_capacity(rows);
        let mut partkey: Vec<i64> = Vec::with_capacity(rows);
        let mut suppkey: Vec<i64> = Vec::with_capacity(rows);
        let mut linenumber: Vec<i64> = Vec::with_capacity(rows);
        let mut quantity = Float64Builder::with_capacity(rows);
        let mut extendedprice = Float64Builder::with_capacity(rows);
        let mut discount = Float64Builder::with_capacity(rows);
        let mut tax = Float64Builder::with_capacity(rows);
        let mut returnflag: Vec<i32> = Vec::with_capacity(rows);
        let mut linestatus: Vec<i32> = Vec::with_capacity(rows);
        let mut shipdate: Vec<i32> = Vec::with_capacity(rows);
        let mut commitdate: Vec<i32> = Vec::with_capacity(rows);
        let mut receiptdate: Vec<i32> = Vec::with_capacity(rows);
        let mut shipinstruct: Vec<i32> = Vec::with_capacity(rows);
        let mut shipmode: Vec<i32> = Vec::with_capacity(rows);
        let mut comment = StringBuilder::with_capacity(rows, rows * COMMENT_MAX_BYTES);

        let orderkey_bound = self.total_rows.max(1);
        let ship_count = self.ship_date_count;
        let commit_span = COMMIT_DATE_MAX_OFFSET_DAYS + 1;
        let receipt_span = RECEIPT_DATE_MAX_OFFSET_DAYS;
        let receipt_min =
            i32::try_from(RECEIPT_DATE_MIN_OFFSET_DAYS).expect("the offset fits in an i32");

        for _ in 0..rows {
            orderkey.push(self.below_i64(orderkey_bound) + 1);
            partkey.push(self.below_i64(PART_COUNT) + 1);
            suppkey.push(self.below_i64(SUPPLIER_COUNT) + 1);
            linenumber.push(self.below_i64(LINE_NUMBER_COUNT) + 1);

            quantity.append_value(self.below_cents(QUANTITY_LOW_CENTS, QUANTITY_SPAN_CENTS));
            extendedprice.append_value(
                self.below_cents(EXTENDED_PRICE_LOW_CENTS, EXTENDED_PRICE_SPAN_CENTS),
            );

            // The null check is always drawn, so that the random stream does
            // not depend on the null rate.
            let discount_is_null = self.rng.next_f64() < self.null_rate;
            let discount_value = self.below_cents(0, DISCOUNT_SPAN_CENTS);
            if discount_is_null {
                discount.append_null();
            } else {
                discount.append_value(discount_value);
            }

            tax.append_value(self.below_cents(0, TAX_SPAN_CENTS));

            returnflag.push(self.below_i32(len_u64(&RETURN_FLAGS)));
            linestatus.push(self.below_i32(len_u64(&LINE_STATUSES)));

            let ship_offset = self.below_i32(ship_count);
            let commit_offset = self.below_i32(commit_span);
            let receipt_offset = receipt_min + self.below_i32(receipt_span);
            shipdate.push(ship_offset);
            commitdate.push(ship_offset + commit_offset);
            receiptdate.push(ship_offset + receipt_offset);

            shipinstruct.push(self.below_i32(len_u64(&SHIP_INSTRUCTIONS)));
            shipmode.push(self.below_i32(len_u64(&SHIP_MODES)));

            let comment_is_null = self.rng.next_f64() < self.null_rate;
            let number = self.below_i64(COMMENT_NUMBER_RANGE);
            let first = self.comment_word();
            let second = self.comment_word();
            if comment_is_null {
                comment.append_null();
            } else {
                comment.append_value(format!("furiously {number} {first} {second}"));
            }
        }

        let columns: Vec<ArrayRef> = vec![
            Arc::new(Int64Array::from(orderkey)),
            Arc::new(Int64Array::from(partkey)),
            Arc::new(Int64Array::from(suppkey)),
            Arc::new(Int64Array::from(linenumber)),
            Arc::new(quantity.finish()),
            Arc::new(extendedprice.finish()),
            Arc::new(discount.finish()),
            Arc::new(tax.finish()),
            dictionary_column(returnflag, &self.dictionaries.returnflag)?,
            dictionary_column(linestatus, &self.dictionaries.linestatus)?,
            dictionary_column(shipdate, &self.dictionaries.shipdate)?,
            dictionary_column(commitdate, &self.dictionaries.commitdate)?,
            dictionary_column(receiptdate, &self.dictionaries.receiptdate)?,
            dictionary_column(shipinstruct, &self.dictionaries.shipinstruct)?,
            dictionary_column(shipmode, &self.dictionaries.shipmode)?,
            Arc::new(comment.finish()),
        ];

        self.emitted += u64::try_from(rows).expect("the row count fits in a u64");

        RecordBatch::try_new(arrow_schema(), columns)
            .map_err(|error| DatasetError::Parquet(format!("arrow record batch: {error}")))
    }

    /// A value in `[0, bound)`, as an `i32`.
    fn below_i32(&mut self, bound: u64) -> i32 {
        draw_below_i32(&mut self.rng, bound)
    }

    /// A value in `[0, bound)`, as an `i64`.
    fn below_i64(&mut self, bound: u64) -> i64 {
        draw_below_i64(&mut self.rng, bound)
    }

    /// A value in `[low, low + span)` cents, as a decimal with two places.
    fn below_cents(&mut self, low: i32, span: u64) -> f64 {
        draw_cents(&mut self.rng, low, span)
    }

    /// A word from the comment vocabulary.
    fn comment_word(&mut self) -> &'static str {
        draw_comment_word(&mut self.rng)
    }
}

/// A value in `[0, bound)`, as an `i32`.
fn draw_below_i32(rng: &mut Xoshiro256, bound: u64) -> i32 {
    i32::try_from(rng.below(bound)).expect("the bound is below i32::MAX")
}

/// A value in `[0, bound)`, as an `i64`.
fn draw_below_i64(rng: &mut Xoshiro256, bound: u64) -> i64 {
    i64::try_from(rng.below(bound)).expect("the bound is below i64::MAX")
}

/// A value in `[low, low + span)` cents, as a decimal with two places.
///
/// The value is drawn in integer cents and divided by 100 once, so every
/// decimal column of the dataset has exactly two decimal places and the
/// same value in every run.
fn draw_cents(rng: &mut Xoshiro256, low: i32, span: u64) -> f64 {
    let cents = i32::try_from(rng.below(span)).expect("the cent value fits in i32") + low;
    f64::from(cents) / 100.0
}

/// A word from the comment vocabulary.
fn draw_comment_word(rng: &mut Xoshiro256) -> &'static str {
    let index =
        usize::try_from(rng.below(len_u64(&COMMENT_WORDS))).expect("the index fits in a usize");
    COMMENT_WORDS.get(index).copied().expect("the index is below the word count")
}

/// Builds a dictionary-encoded Arrow array from its keys and its values.
fn dictionary_column(keys: Vec<i32>, values: &Arc<StringArray>) -> Result<ArrayRef, DatasetError> {
    let owned: Arc<StringArray> = Arc::clone(values);
    let values: ArrayRef = owned;
    let array = DictionaryArray::try_new(Int32Array::from(keys), values)
        .map_err(|error| DatasetError::Parquet(format!("arrow dictionary array: {error}")))?;
    Ok(Arc::new(array))
}

/// The seed of the `orders` PRNG stream, derived from the dataset seed.
///
/// The two tables must not share a stream. If they did, generating the
/// `orders` table would move every `lineitem` row drawn after it, and the
/// dataset would stop being a pure function of `(scale_factor, seed,
/// null_rate)`.
///
/// So the `orders` stream is seeded from a *different* number: the dataset
/// seed mixed with a fixed domain constant. `splitmix64` is a bijection, so
/// two different seeds give two different streams, and the mix is followed
/// by a check that the derived seed is never the dataset seed itself -- so
/// for no seed at all can the two streams be the same stream.
#[must_use]
pub fn orders_seed(seed: u64) -> u64 {
    let mut mixer = seed ^ ORDERS_SEED_DOMAIN;
    let derived = splitmix64(&mut mixer);
    if derived == seed {
        derived.wrapping_add(1)
    } else {
        derived
    }
}

/// The order keys that have already been drawn.
///
/// A bitset over the key space: the space is a few million keys at SF-1, so
/// the set is a few hundred kilobytes and taking a key is a constant-time
/// bit test. A hash set would do the same job with an order of magnitude
/// more memory and a slower path.
#[derive(Debug)]
struct TakenKeys {
    words: Vec<u64>,
}

impl TakenKeys {
    /// A set over a key space of `space` keys.
    fn new(space: u64) -> Self {
        let words = usize::try_from(space.div_ceil(64)).unwrap_or(0);
        Self { words: vec![0; words] }
    }

    /// Takes a key, and reports whether it was free.
    fn take(&mut self, key: u64) -> bool {
        let bit = key.saturating_sub(1);
        let word = usize::try_from(bit / 64).unwrap_or(usize::MAX);
        let Some(cell) = self.words.get_mut(word) else {
            return false;
        };
        let mask = 1u64 << (bit % 64);
        if *cell & mask != 0 {
            return false;
        }
        *cell |= mask;
        true
    }
}

/// The `orders` dictionary value arrays.
///
/// The date dictionary is the same table the `lineitem` dates are drawn
/// from, so the two tables' dates are the same dates.
#[derive(Debug)]
struct OrdersDictionaries {
    status: Arc<StringArray>,
    orderdate: Arc<StringArray>,
    priority: Arc<StringArray>,
}

impl OrdersDictionaries {
    fn new() -> Self {
        let dates = date_table();
        let orderdate_len = usize::try_from(ship_date_span_days() + 1)
            .expect("the order date count fits in a usize");
        let orderdate = StringArray::from(
            dates
                .get(..orderdate_len)
                .expect("the order date slice is inside the date table")
                .to_vec(),
        );

        Self {
            status: Arc::new(StringArray::from_iter_values(ORDER_STATUSES)),
            orderdate: Arc::new(orderdate),
            priority: Arc::new(StringArray::from_iter_values(ORDER_PRIORITIES)),
        }
    }
}

/// The smallest `o_totalprice` value, in cents.
///
/// The same range as `l_extendedprice`: TPC-H's order totals and its
/// extended prices share it.
const TOTAL_PRICE_LOW_CENTS: i32 = EXTENDED_PRICE_LOW_CENTS;
/// The number of distinct `o_totalprice` values, in cents.
const TOTAL_PRICE_SPAN_CENTS: u64 = EXTENDED_PRICE_SPAN_CENTS;

/// Generates the rows of the `orders` table.
///
/// The table is the build side of the join: it is the smaller of the two,
/// and the engine materialises it before it probes `lineitem`.
///
/// The join key, `o_orderkey`, is **distinct** and is drawn from the same
/// key space as `l_orderkey`: `[1, lineitem_rows]`. Distinctness is the
/// table's primary key, and the join's match rate depends on it -- with
/// 1,500,000 distinct keys over a 6,000,000 key space, a quarter of
/// `lineitem`'s rows find a partner.
#[derive(Debug)]
pub struct OrdersGenerator {
    rng: Xoshiro256,
    total_rows: u64,
    emitted: u64,
    null_rate: f64,
    key_space: u64,
    custkey_bound: u64,
    clerk_bound: u64,
    order_date_count: u64,
    taken: TakenKeys,
    dictionaries: OrdersDictionaries,
}

impl OrdersGenerator {
    /// Builds a generator that will produce `rows` rows.
    ///
    /// The keys are drawn from `[1, key_space]`, the key space `lineitem`'s
    /// `l_orderkey` is drawn from, and they are distinct.
    ///
    /// # Errors
    ///
    /// Returns [`DatasetError::InvalidSpec`] if `null_rate` is outside
    /// `[0.0, 1.0]`, or if the table would need more distinct keys than the
    /// key space holds.
    pub fn new(spec: &DatasetSpec, rows: u64, key_space: u64) -> Result<Self, DatasetError> {
        if !(0.0..=1.0).contains(&spec.null_rate) {
            return Err(DatasetError::InvalidSpec(format!(
                "null_rate {} is not in [0.0, 1.0]",
                spec.null_rate
            )));
        }
        if rows > key_space {
            return Err(DatasetError::InvalidSpec(format!(
                "{rows} distinct order keys do not fit in a key space of {key_space} keys"
            )));
        }

        Ok(Self {
            rng: Xoshiro256::seed_from_u64(orders_seed(spec.seed)),
            total_rows: rows,
            emitted: 0,
            null_rate: spec.null_rate,
            key_space,
            custkey_bound: CUSTOMERS_PER_SCALE_FACTOR
                .saturating_mul(u64::from(spec.scale_factor))
                .max(1),
            clerk_bound: CLERKS_PER_SCALE_FACTOR
                .saturating_mul(u64::from(spec.scale_factor))
                .max(1),
            order_date_count: ship_date_span_days() + 1,
            taken: TakenKeys::new(key_space),
            dictionaries: OrdersDictionaries::new(),
        })
    }

    /// The number of rows generated so far.
    #[must_use]
    pub fn rows_emitted(&self) -> u64 {
        self.emitted
    }

    /// The number of rows this generator will produce in total.
    #[must_use]
    pub fn total_rows(&self) -> u64 {
        self.total_rows
    }

    /// The size of the key space the order keys are drawn from.
    #[must_use]
    pub fn key_space(&self) -> u64 {
        self.key_space
    }

    /// Generates the next batch of at most `len` rows.
    ///
    /// The batch holds the columns in schema order and uses the schema from
    /// [`crate::schema::orders_arrow_schema`].
    ///
    /// # Errors
    ///
    /// Returns [`DatasetError::Parquet`] if the Arrow arrays cannot be
    /// assembled into a record batch.
    pub fn next_batch(&mut self, len: usize) -> Result<RecordBatch, DatasetError> {
        let remaining = self.total_rows.saturating_sub(self.emitted);
        let requested = u64::try_from(len).unwrap_or(u64::MAX);
        let rows = usize::try_from(remaining.min(requested)).unwrap_or(len);

        let mut orderkey: Vec<i64> = Vec::with_capacity(rows);
        let mut custkey: Vec<i64> = Vec::with_capacity(rows);
        let mut status: Vec<i32> = Vec::with_capacity(rows);
        let mut totalprice = Float64Builder::with_capacity(rows);
        let mut orderdate: Vec<i32> = Vec::with_capacity(rows);
        let mut priority: Vec<i32> = Vec::with_capacity(rows);
        let mut clerk: Vec<i64> = Vec::with_capacity(rows);
        let mut shippriority: Vec<i64> = Vec::with_capacity(rows);
        let mut comment = StringBuilder::with_capacity(rows, rows * COMMENT_MAX_BYTES);

        let key_space = self.key_space;
        let custkey_bound = self.custkey_bound;
        let clerk_bound = self.clerk_bound;
        let order_date_count = self.order_date_count;

        for _ in 0..rows {
            orderkey.push(self.next_orderkey(key_space));
            custkey.push(self.below_i64(custkey_bound) + 1);
            status.push(self.below_i32(len_u64(&ORDER_STATUSES)));
            totalprice
                .append_value(self.below_cents(TOTAL_PRICE_LOW_CENTS, TOTAL_PRICE_SPAN_CENTS));
            orderdate.push(self.below_i32(order_date_count));
            priority.push(self.below_i32(len_u64(&ORDER_PRIORITIES)));
            clerk.push(self.below_i64(clerk_bound) + 1);
            shippriority.push(self.below_i64(SHIP_PRIORITY_COUNT));

            // The null check is always drawn, so that the random stream does
            // not depend on the null rate.
            let comment_is_null = self.rng.next_f64() < self.null_rate;
            let number = self.below_i64(COMMENT_NUMBER_RANGE);
            let first = self.comment_word();
            let second = self.comment_word();
            if comment_is_null {
                comment.append_null();
            } else {
                comment.append_value(format!("furiously {number} {first} {second}"));
            }
        }

        let columns: Vec<ArrayRef> = vec![
            Arc::new(Int64Array::from(orderkey)),
            Arc::new(Int64Array::from(custkey)),
            dictionary_column(status, &self.dictionaries.status)?,
            Arc::new(totalprice.finish()),
            dictionary_column(orderdate, &self.dictionaries.orderdate)?,
            dictionary_column(priority, &self.dictionaries.priority)?,
            Arc::new(Int64Array::from(clerk)),
            Arc::new(Int64Array::from(shippriority)),
            Arc::new(comment.finish()),
        ];

        self.emitted += u64::try_from(rows).expect("the row count fits in a u64");

        RecordBatch::try_new(orders_arrow_schema(), columns)
            .map_err(|error| DatasetError::Parquet(format!("arrow record batch: {error}")))
    }

    /// The next distinct key, drawn uniformly from the key space.
    ///
    /// A key that has been taken is drawn again: the keys are sampled
    /// without replacement. The set of taken keys is a bitset, so a draw
    /// costs a constant number of operations per attempt.
    fn next_orderkey(&mut self, key_space: u64) -> i64 {
        loop {
            let key = self.below_i64(key_space) + 1;
            if self.taken.take(u64::try_from(key).expect("the key fits in a u64")) {
                return key;
            }
        }
    }

    /// A value in `[0, bound)`, as an `i32`.
    fn below_i32(&mut self, bound: u64) -> i32 {
        draw_below_i32(&mut self.rng, bound)
    }

    /// A value in `[0, bound)`, as an `i64`.
    fn below_i64(&mut self, bound: u64) -> i64 {
        draw_below_i64(&mut self.rng, bound)
    }

    /// A value in `[low, low + span)` cents, as a decimal with two places.
    fn below_cents(&mut self, low: i32, span: u64) -> f64 {
        draw_cents(&mut self.rng, low, span)
    }

    /// A word from the comment vocabulary.
    fn comment_word(&mut self) -> &'static str {
        draw_comment_word(&mut self.rng)
    }
}
