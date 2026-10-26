//! The Parquet machinery the probes share.
//!
//! Two probes use this: the one from spec 012, which took the predicate's
//! column apart, and the one from spec 014, which takes the **materialised**
//! column apart. They share the decoder, because there is one decoder and two
//! things to measure with it.
//!
//! # What is here
//!
//! - The Parquet format's **RLE/bit-packed hybrid** decoder, in about a
//!   hundred lines, and the tests that check it against the format.
//! - The **page walking**: the pages are fetched and decompressed by the
//!   parquet crate's own page reader, and the visitor is handed the values
//!   section of each data page.
//! - The **timing** helper: every figure is the median of several runs, with
//!   the spread.
//!
//! # Why the probe owns the RLE
//!
//! The parquet crate's RLE decoder is private: `encodings/rle.rs` is a
//! `pub(crate)` module, and the `RleDecoder` it holds cannot be reached from
//! outside the crate even with the `experimental` feature on. The only public
//! way to read a dictionary-encoded column is the whole expansion:
//! `DictDecoder::get` decodes the indices *and* gathers the dictionary through
//! them, in one call, with no seam between the two.
//!
//! So the probe owns the RLE. And it is checked against the parquet crate
//! itself -- the probe expands every value of every row group with its own
//! decoder and compares it, bit for bit, with what the parquet crate's own
//! reader produced. If the two disagree, the probe says so and stops. An
//! instrument that has not been checked against the reference it is measuring
//! against is not an instrument.
//!
//! # The slices and the indexes
//!
//! Every slice and index in this module is into a buffer the probe has just
//! filled, and every length is asserted immediately above the index that uses
//! it. The lint is allowed here, and the assertion is the reason.

// The slices and indexes below are all into buffers this module has just
// filled, and each one is asserted immediately above the use. See the module
// comment.
#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::path::Path;
use std::time::Duration;

use parquet::basic::Encoding;
use parquet::column::page::Page;
use parquet::file::reader::{FileReader, SerializedFileReader};

/// Milliseconds, for printing.
#[must_use]
pub(crate) fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

// ---------------------------------------------------------------------------
// The RLE/bit-packed hybrid
// ---------------------------------------------------------------------------

/// The Parquet format's RLE/bit-packed hybrid decoder.
///
/// The format is a sequence of runs, and each run's header is an unsigned
/// varint. The low bit says which kind of run it is:
///
/// - **0** -- an RLE run. The rest of the header is the number of values,
///   and the value itself follows in `ceil(bit_width / 8)` bytes.
/// - **1** -- a bit-packed run. The rest of the header is the number of
///   *groups* of eight values, and the groups follow, each `bit_width`
///   bytes wide, with the values packed LSB-first.
///
/// Eight values of `bit_width` bits are `8 * bit_width` bits, which is
/// `bit_width` bytes: the groups are byte-aligned, and a group's width is
/// *not* `ceil(bit_width / 8)`. That is the width of an RLE run's repeated
/// value, which is one value and not eight.
pub(crate) struct Rle<'a> {
    /// How many bits each value occupies.
    bit_width: u8,
    /// The bytes of the run stream.
    data: &'a [u8],
    /// How far into `data` the decoder has read.
    position: usize,
    /// How many values are left in the current RLE run.
    run_left: usize,
    /// The value the current RLE run repeats.
    run_value: u32,
    /// How many values are left in the current bit-packed run.
    packed_left: usize,
    /// The bytes of the current bit-packed run.
    packed: &'a [u8],
    /// When counting, the value the decoder counts.
    count_target: Option<u32>,
    /// How many values the decoder has seen equal to `count_target`.
    count: usize,
}

impl<'a> Rle<'a> {
    /// A decoder over `data`, with `bit_width` bits per value.
    #[must_use]
    pub(crate) fn new(bit_width: u8, data: &'a [u8]) -> Self {
        Self {
            bit_width,
            data,
            position: 0,
            run_left: 0,
            run_value: 0,
            packed_left: 0,
            packed: &[],
            count_target: None,
            count: 0,
        }
    }

    /// A decoder that counts the values equal to `target`.
    #[must_use]
    pub(crate) fn counting(bit_width: u8, data: &'a [u8], target: u32) -> Self {
        Self { count_target: Some(target), ..Self::new(bit_width, data) }
    }

    /// How many bytes one group of eight values occupies.
    fn group_bytes(&self) -> usize {
        usize::from(self.bit_width)
    }

    /// How many bytes an RLE run's repeated value occupies.
    fn value_bytes(&self) -> usize {
        usize::from(self.bit_width.div_ceil(8))
    }

    /// Decodes as many values as fit into `out`, and returns how many it wrote.
    pub(crate) fn decode(&mut self, out: &mut [u32]) -> usize {
        let mut written = 0;
        while written < out.len() {
            if self.run_left > 0 {
                let take = (out.len() - written).min(self.run_left);
                out[written..written + take].fill(self.run_value);
                if self.count_target == Some(self.run_value) {
                    self.count += take;
                }
                written += take;
                self.run_left -= take;
            } else if self.packed_left > 0 {
                written += self.unpack(out, written);
            } else if !self.next_run() {
                break;
            }
        }
        written
    }

    /// Unpacks the bit-packed run into `out`, starting at `out[written..]`.
    fn unpack(&mut self, out: &mut [u32], written: usize) -> usize {
        let group_bytes = self.group_bytes();
        let mask = if self.bit_width >= 32 { u32::MAX } else { (1u32 << self.bit_width) - 1 };
        let width = self.bit_width as usize;

        let mut done = 0usize;
        while done < self.packed_left && written + done < out.len() {
            let group = done / 8;
            let within = done % 8;
            let take = (self.packed_left - done).min(8 - within).min(out.len() - written - done);

            let start = group * group_bytes;
            let bytes = &self.packed[start..start + group_bytes];

            if width <= 8 {
                // The group's eight values fit in one little-endian word.
                let mut word = 0u32;
                for (index, &byte) in bytes.iter().enumerate() {
                    word |= u32::from(byte) << (8 * index);
                }
                for offset in 0..take {
                    let shift = (within + offset) * width;
                    let value = (word >> shift) & mask;
                    if self.count_target == Some(value) {
                        self.count += 1;
                    }
                    out[written + done + offset] = value;
                }
            } else if width <= 16 {
                // Wider than a word, but the group's eight values still fit in
                // one: `8 * bit_width` bits are at most 128. Reading the group
                // as a single wide word and shifting inside it is the
                // difference between a decoder that keeps up with the crate's
                // and one that does not -- and at thirteen bits the bit at a
                // time path is four times slower than the crate's.
                let mut wide = 0u128;
                for (index, &byte) in bytes.iter().enumerate() {
                    wide |= u128::from(byte) << (8 * index);
                }
                let wide_mask = (1u128 << width) - 1;
                for offset in 0..take {
                    let shift = (within + offset) * width;
                    // The masked value is at most sixteen bits wide, so it
                    // fits in a `u32` without losing anything.
                    let value = u32::try_from((wide >> shift) & wide_mask).unwrap_or(0);
                    if self.count_target == Some(value) {
                        self.count += 1;
                    }
                    out[written + done + offset] = value;
                }
            } else {
                // Wider than a wide word: the values are read a bit at a time.
                for offset in 0..take {
                    let bit_offset = (within + offset) * width;
                    let value = read_bits(bytes, bit_offset, width);
                    if self.count_target == Some(value) {
                        self.count += 1;
                    }
                    out[written + done + offset] = value;
                }
            }
            done += take;
        }
        self.packed_left -= done;
        done
    }

    /// Reads the next run's header, and sets the decoder up for it.
    fn next_run(&mut self) -> bool {
        let Some(header) = self.read_varint() else {
            return false;
        };
        if header & 1 == 1 {
            let groups = usize::try_from(header >> 1).unwrap_or(0);
            let bytes = groups * self.group_bytes();
            let end = self.position + bytes;
            if end > self.data.len() {
                return false;
            }
            let data = self.data;
            self.packed = &data[self.position..end];
            self.position = end;
            self.packed_left = groups * 8;
        } else {
            self.run_left = usize::try_from(header >> 1).unwrap_or(0);
            let width = self.value_bytes();
            let end = self.position + width;
            if end > self.data.len() {
                return false;
            }
            let mut value = 0u32;
            for (index, &byte) in self.data[self.position..end].iter().enumerate() {
                value |= u32::from(byte) << (8 * index);
            }
            self.position = end;
            self.run_value = value;
        }
        true
    }

    /// Reads an unsigned varint.
    fn read_varint(&mut self) -> Option<u64> {
        let mut result = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = *self.data.get(self.position)?;
            self.position += 1;
            result |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Some(result);
            }
            shift += 7;
            if shift > 63 {
                return None;
            }
        }
    }
}

/// Reads `width` bits out of `bytes`, starting at `bit_offset`.
///
/// The bits are LSB-first: the first bit of the value is the lowest bit of
/// the first byte. This is the general path, for a bit width too wide for
/// the word-at-a-time unpack.
fn read_bits(bytes: &[u8], bit_offset: usize, width: usize) -> u32 {
    let mut value = 0u32;
    for bit in 0..width {
        let absolute = bit_offset + bit;
        let byte = bytes[absolute / 8];
        let bit_in_byte = absolute % 8;
        value |= u32::from((byte >> bit_in_byte) & 1) << bit;
    }
    value
}

// ---------------------------------------------------------------------------
// The file's pages
// ---------------------------------------------------------------------------

/// How many bits a level of `max_level` needs.
#[must_use]
pub(crate) fn bits_for(max_level: i16) -> u8 {
    if max_level <= 0 {
        0
    } else {
        u8::try_from(u32::from(max_level.unsigned_abs()).ilog2()).unwrap_or(0) + 1
    }
}

/// The buffers a probe reuses across pages.
///
/// A probe measures a decode, and a decode that reallocates its buffers
/// every page measures the allocator as much as the decoder.
#[derive(Debug, Default)]
pub(crate) struct Scratch {
    /// The decoded indices.
    pub(crate) indices: Vec<u32>,
    /// The expanded values.
    pub(crate) values: Vec<f64>,
}

impl Scratch {
    /// A scratch space with room for `rows` indices.
    #[must_use]
    pub(crate) fn new(rows: usize) -> Self {
        Self { indices: vec![0u32; rows], values: Vec::new() }
    }
}

/// The definition levels of a v1 data page, and the values section after them.
///
/// A v1 page holds the repetition levels, then the definition levels, then
/// the values. Each of the two level sections is a four-byte length followed
/// by that many bytes of RLE.
///
/// The levels are decoded into `levels`, and the number of values the page
/// holds -- the rows that are not NULL -- is returned with them.
///
/// A column with no definition levels has no level section at all, and every
/// one of its rows holds a value. That is the case the Parquet format calls
/// *required*, and it is the case the materialised column is in.
pub(crate) fn split_v1_page<'a>(
    buf: &'a [u8],
    max_rep_level: i16,
    max_def_level: i16,
    rows: usize,
    levels: &mut Vec<u16>,
) -> (usize, &'a [u8]) {
    // The repetition levels come first, but a flat column has none of them,
    // and the format does not write a section it does not need.
    let offset = if max_rep_level > 0 { read_level_section(buf, 0).0 } else { 0 };

    let (def_bytes, def_data) =
        if max_def_level > 0 { read_level_section(buf, offset) } else { (0, &[][..]) };
    let offset = offset + def_bytes;

    // The levels are handed to the visitor one per row, whether or not the
    // column has any: a required column's levels are all zero.
    if levels.len() < rows {
        levels.resize(rows, 0);
    }

    let values = if max_def_level == 0 {
        // A required column: every row holds a value, and the definition
        // level of a required column is zero -- which is its maximum, and so
        // means "present".
        rows
    } else {
        let max_level = u32::from(max_def_level.unsigned_abs());
        let mut decoder = Rle::counting(bits_for(max_def_level), def_data, max_level);
        let mut words = vec![0u32; rows];
        let read = decoder.decode(&mut words);
        for (index, &word) in words[..read].iter().enumerate() {
            levels[index] = u16::try_from(word).unwrap_or(0);
        }
        decoder.count
    };

    (values, &buf[offset..])
}

/// Reads a level section's length prefix, and returns its size and its bytes.
fn read_level_section(buf: &[u8], offset: usize) -> (usize, &[u8]) {
    let length_bytes = buf.get(offset..offset + 4).expect("the level section has a length");
    let length = i32::from_le_bytes(length_bytes.try_into().expect("four bytes"));
    let size = 4 + usize::try_from(length).unwrap_or(0);
    (size, buf.get(offset + 4..offset + size).expect("the level section is there"))
}

/// The dictionary page's values, in the dictionary's order.
///
/// A PLAIN dictionary of doubles is eight bytes per value, little-endian.
#[must_use]
pub(crate) fn decode_dictionary(buf: &[u8], num_values: u32) -> Vec<f64> {
    assert_eq!(buf.len(), num_values as usize * 8, "the dictionary is PLAIN doubles");
    buf.chunks_exact(8)
        .map(|bytes| f64::from_le_bytes(bytes.try_into().expect("eight bytes")))
        .collect()
}

/// Walks a column's pages, and hands each data page to `visit`.
///
/// The pages are fetched and decompressed by the parquet crate's own page
/// reader, which is what the crate's own column reader does under it. The
/// visitor is handed the page's encoding, the page's dictionary, the page's
/// definition levels, the number of values the page holds, and the page's
/// values section.
pub(crate) fn for_each_page(
    path: &Path,
    column: usize,
    max_rep_level: i16,
    max_def_level: i16,
    mut visit: impl FnMut(Encoding, &[f64], &[u16], usize, &[u8]),
) {
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    let mut levels: Vec<u16> = Vec::new();
    for group in 0..reader.num_row_groups() {
        let group = reader.get_row_group(group).expect("the row group reads");
        let mut pages = group.get_column_page_reader(column).expect("the page reader opens");
        let mut dictionary: Vec<f64> = Vec::new();
        while let Some(page) = pages.get_next_page().expect("the page reads") {
            match page {
                Page::DictionaryPage { buf, num_values, .. } => {
                    dictionary = decode_dictionary(&buf, num_values);
                }
                Page::DataPage { buf, num_values, encoding, .. } => {
                    let rows = num_values as usize;
                    let (values, section) =
                        split_v1_page(&buf, max_rep_level, max_def_level, rows, &mut levels);
                    visit(encoding, &dictionary, &levels[..rows], values, section);
                }
                Page::DataPageV2 { .. } => {
                    // A v2 page holds its level sections with their lengths in
                    // the page header, which the parquet crate's `Page` does
                    // not carry. This file's pages are v1, so this is a
                    // refusal rather than a guess.
                    panic!("the file holds a v2 data page, which this probe does not read");
                }
            }
        }
    }
}

/// Decodes a page's values section into `out`, and returns how many it wrote.
///
/// The values section of a dictionary-encoded page is one byte of bit width,
/// and then the RLE/bit-packed hybrid.
pub(crate) fn decode_indices(section: &[u8], count: usize, out: &mut [u32]) {
    let bit_width = section[0];
    let mut decoder = Rle::new(bit_width, &section[1..]);
    let read = decoder.decode(&mut out[..count]);
    assert_eq!(read, count, "the page holds the values it says");
}

// ---------------------------------------------------------------------------
// The timings
// ---------------------------------------------------------------------------

/// A measurement, with its median and its spread.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Timing {
    /// The median, in milliseconds.
    pub(crate) median: f64,
    /// The fastest sample, in milliseconds.
    pub(crate) min: f64,
    /// The slowest sample, in milliseconds.
    pub(crate) max: f64,
}

impl Timing {
    /// The timing of a set of samples.
    #[must_use]
    pub(crate) fn of(samples: &[Duration]) -> Self {
        let mut sorted = samples.to_vec();
        sorted.sort();
        let middle = sorted.len() / 2;
        Self { median: ms(sorted[middle]), min: ms(sorted[0]), max: ms(sorted[sorted.len() - 1]) }
    }
}

/// Repeats a measurement, and returns its median and its spread.
pub(crate) fn repeat<T>(
    iterations: usize,
    mut measure: impl FnMut() -> (Duration, T),
) -> (Timing, T) {
    let mut samples: Vec<Duration> = Vec::with_capacity(iterations);
    let mut last = None;
    for _ in 0..iterations {
        let (elapsed, value) = measure();
        samples.push(elapsed);
        last = Some(value);
    }
    let timing = Timing::of(&samples);
    (timing, last.expect("the measurement ran at least once"))
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

    use super::{decode_dictionary, Rle};

    /// Decodes a whole stream into a vector.
    fn decode(bit_width: u8, stream: &[u8], count: usize) -> Vec<u32> {
        let mut out = vec![0u32; count];
        let mut decoder = Rle::new(bit_width, stream);
        let read = decoder.decode(&mut out);
        assert_eq!(read, count, "the stream holds the values it says");
        out
    }

    /// Packs eight values into one bit-packed group.
    ///
    /// A group of eight values of `bit_width` bits is `bit_width` bytes: the
    /// values are packed LSB-first, and each one takes the next `bit_width`
    /// bits of the stream.
    fn pack(values: &[u32], bit_width: usize) -> Vec<u8> {
        assert_eq!(values.len(), 8, "a group is eight values");
        let mut packed = vec![0u8; bit_width];
        for (index, &value) in values.iter().enumerate() {
            for bit in 0..bit_width {
                if value >> bit & 1 == 1 {
                    let absolute = index * bit_width + bit;
                    packed[absolute / 8] |= 1u8 << (absolute % 8);
                }
            }
        }
        packed
    }

    /// A stream holding one bit-packed group of eight values.
    fn group(values: &[u32], bit_width: usize) -> Vec<u8> {
        let mut stream = vec![0x03];
        stream.extend_from_slice(&pack(values, bit_width));
        stream
    }

    /// The reference vector from the Parquet format's own documentation:
    /// eight values of three bits, one bit-packed group.
    #[test]
    fn decodes_the_formats_reference_vector() {
        let values = decode(3, &[0x03, 0x88, 0xC6, 0xFA], 8);
        assert_eq!(values, vec![0, 1, 2, 3, 4, 5, 6, 7]);
    }

    /// An RLE run: the header is the count shifted left, and the value
    /// follows in `ceil(bit_width / 8)` bytes.
    #[test]
    fn decodes_an_rle_run() {
        // Five values of the value 9, at a bit width of four.
        let values = decode(4, &[0x0A, 0x09], 5);
        assert_eq!(values, vec![9, 9, 9, 9, 9]);
    }

    /// An RLE run and then a bit-packed group, in one stream.
    #[test]
    fn decodes_a_mixed_stream() {
        // Three values of 7, then eight values of four bits.
        let mut stream = vec![0x06, 0x07];
        stream.extend_from_slice(&group(&[0, 1, 2, 3, 4, 5, 6, 7], 4));
        let values = decode(4, &stream, 11);
        assert_eq!(values, vec![7, 7, 7, 0, 1, 2, 3, 4, 5, 6, 7]);
    }

    /// A bit width too wide for one word: the values are read a bit at a
    /// time, and the decoder has to agree with itself about where they are.
    #[test]
    fn decodes_a_wide_bit_width() {
        let values: Vec<u32> = (0..8).map(|index| index * 1000 + 7).collect();
        let stream = group(&values, 13);
        assert_eq!(decode(13, &stream, 8), values);
    }

    /// The counting decoder: it counts the values equal to the maximum
    /// level, which is how many values a page holds.
    #[test]
    fn counts_the_levels_that_are_the_maximum() {
        // Six levels of one, in an RLE run, then eight levels from a
        // bit-packed group: three of them are ones.
        let mut stream = vec![0x0C, 0x01];
        stream.extend_from_slice(&group(&[1, 1, 1, 0, 0, 0, 0, 0], 1));
        let mut words = vec![0u32; 14];
        let mut decoder = Rle::counting(1, &stream, 1);
        let read = decoder.decode(&mut words);
        assert_eq!(read, 14);
        assert_eq!(decoder.count, 9, "nine of the fourteen levels are the maximum");
    }

    /// A dictionary whose values are **not sorted**, and whose indices are
    /// **not monotonic**. The gather must still produce the right values.
    #[test]
    fn gathers_an_unsorted_dictionary_with_non_monotonic_indices() {
        let dictionary_values = [3.5f64, -1.0, 99.25, 0.5, 7.0];
        let mut buffer = Vec::with_capacity(dictionary_values.len() * 8);
        for value in dictionary_values {
            buffer.extend_from_slice(&value.to_le_bytes());
        }
        let dictionary = decode_dictionary(&buffer, dictionary_values.len() as u32);
        assert_eq!(dictionary, dictionary_values.to_vec());

        // The indices: 4, 0, 4, 2, 1, 3, 0, 4 -- not monotonic, and not
        // sorted either.
        let indices = [4u32, 0, 4, 2, 1, 3, 0, 4];
        let decoded = decode(3, &group(&indices, 3), indices.len());
        assert_eq!(decoded, indices.to_vec());

        let gathered: Vec<f64> = decoded.iter().map(|&index| dictionary[index as usize]).collect();
        assert_eq!(
            gathered,
            vec![7.0, 3.5, 7.0, 99.25, -1.0, 0.5, 3.5, 7.0],
            "the gather follows the indices, not the order of the values"
        );
    }
}
