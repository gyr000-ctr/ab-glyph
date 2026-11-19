//! GPOS pair kerning support.
//!
//! Uses the legacy `kern` table when present, preserving those values exactly.
//! Otherwise horizontal pair adjustments from the `kern` feature of the `GPOS`
//! table are used.
//!
//! The GPOS table itself is parsed directly into owned, compact lookup tables
//! once per font. A kerning query then only does constant-time indexing and
//! hashing, with no allocation.

#[cfg(test)]
mod tests;

use alloc::vec::Vec;

/// Maximum number of pair records parsed from a format 1 pair adjustment
/// subtable. Larger subtables are ignored, to bound font load cost for
/// pathological fonts. Such fonts got no GPOS kerning before, so this only
/// affects the size of the improvement.
const MAX_PAIR_RECORDS: usize = 1 << 22;

/// Maximum class matrix size (in entries) parsed from a format 2 pair
/// adjustment subtable. Larger matrices are ignored, as with
/// [`MAX_PAIR_RECORDS`].
const MAX_CLASS_MATRIX: usize = 1 << 22;

/// Class of glyphs not covered by a format 2 pair adjustment.
const UNCOVERED: u16 = u16::MAX;

/// Horizontal pair kerning of a font, in font units.
#[derive(Clone, Default)]
pub(crate) struct PairKerning {
    /// Pair adjustment lookups of `kern` features, in application order.
    lookups: Vec<Lookup>,
}

#[derive(Clone)]
struct Lookup {
    /// Subtables in application order; the first applicable one wins.
    subtables: Vec<Subtable>,
}

#[derive(Clone)]
enum Subtable {
    /// GPOS pair adjustment format 1.
    PairSets {
        /// Glyph id -> covered (0/1).
        covered: Vec<u8>,
        /// `(first << 16 | second) -> adjustment` for every pair record.
        pairs: PairHash,
    },
    /// GPOS pair adjustment format 2.
    ClassMatrix {
        /// Glyph id -> class of the first glyph, or [`UNCOVERED`].
        class1: Vec<u16>,
        /// Glyph id -> class of the second glyph, 0 when undefined.
        class2: Vec<u16>,
        class2_count: u16,
        /// `class1_count * class2_count` adjustments.
        values: Vec<i32>,
    },
}

impl Subtable {
    /// Returns the adjustment if this subtable applies to the glyph pair, else
    /// `None`.
    ///
    /// A format 1 subtable only applies when it has a pair record for
    /// `second`; merely covering `first` does not apply a zero adjustment and
    /// the lookup continues with the next subtable.
    #[inline]
    fn value(&self, first: u16, second: u16) -> Option<i32> {
        match self {
            Subtable::PairSets { covered, pairs } => {
                if covered.get(first as usize).copied().unwrap_or(0) == 0 {
                    return None;
                }
                pairs.get(pair_key(first, second))
            }
            Subtable::ClassMatrix {
                class1,
                class2,
                class2_count,
                values,
            } => {
                let c1 = *class1.get(first as usize)?;
                if c1 == UNCOVERED {
                    return None;
                }
                let c2 = class2.get(second as usize).copied().unwrap_or(0);
                let index = c1 as usize * *class2_count as usize + c2 as usize;
                Some(values.get(index).copied().unwrap_or(0))
            }
        }
    }
}

impl PairKerning {
    /// Parses the horizontal pair kerning of `data`, a complete font file.
    ///
    /// Returns no kerning at all for fonts with a legacy `kern` table, so their
    /// values are left entirely to the legacy implementation.
    pub(crate) fn parse(data: &[u8], index: u32, glyph_count: usize) -> Self {
        let Some(gpos) = raw_table(data, index, b"GPOS") else {
            return Self::default();
        };
        if raw_table(data, index, b"kern").is_some() {
            return Self::default();
        }
        parse_gpos(gpos, glyph_count).unwrap_or_default()
    }

    /// Unscaled kerning adjustment for a glyph pair, as used by
    /// [`Font::kern_unscaled`](crate::Font::kern_unscaled).
    #[inline]
    pub(crate) fn adjust(&self, first: u16, second: u16) -> i32 {
        let mut adjustment = 0;
        for lookup in &self.lookups {
            for subtable in &lookup.subtables {
                if let Some(value) = subtable.value(first, second) {
                    adjustment += value;
                    break;
                }
            }
        }
        adjustment
    }
}

fn pair_key(first: u16, second: u16) -> u32 {
    (u32::from(first) << 16) | u32::from(second)
}

fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
}

/// Resolves `offset`, relative to the start of `table`, into a slice of `data`.
///
/// `table` must be a sub-slice of `data`.
fn table_slice<'a>(data: &'a [u8], table: &[u8], offset: u16) -> Option<&'a [u8]> {
    let start = (table.as_ptr() as usize).checked_sub(data.as_ptr() as usize)?;
    data.get(start.checked_add(usize::from(offset))?..)
}

/// Returns the byte offset of face `index` in the font file.
fn face_offset(data: &[u8], index: u32) -> Option<usize> {
    if data.get(..4)? != b"ttcf" {
        return (index == 0).then_some(0);
    }
    let count = u32::from_be_bytes(data.get(8..12)?.try_into().ok()?);
    if index >= count {
        return None;
    }
    let offset = data.get(12 + 4 * index as usize..16 + 4 * index as usize)?;
    usize::try_from(u32::from_be_bytes(offset.try_into().ok()?)).ok()
}

/// Returns the raw data of a font table.
fn raw_table<'a>(data: &'a [u8], index: u32, tag: &[u8; 4]) -> Option<&'a [u8]> {
    let face = face_offset(data, index)?;
    let num_tables = u16_at(data, face + 4)?;
    for i in 0..usize::from(num_tables) {
        let record = face + 12 + 16 * i;
        if data.get(record..record + 4)? != tag {
            continue;
        }
        let offset = u32::from_be_bytes(data.get(record + 8..record + 12)?.try_into().ok()?);
        let length = u32::from_be_bytes(data.get(record + 12..record + 16)?.try_into().ok()?);
        let start = usize::try_from(offset).ok()?;
        let end = start.checked_add(usize::try_from(length).ok()?)?;
        return data.get(start..end);
    }
    None
}

/// Returns the GPOS `kern` feature pair adjustments.
fn parse_gpos(gpos: &[u8], glyph_count: usize) -> Option<PairKerning> {
    let feature_list = u16_at(gpos, 6)?;
    let lookup_list = u16_at(gpos, 8)?;
    let feature_list = table_slice(gpos, gpos, feature_list)?;
    let lookup_list = table_slice(gpos, gpos, lookup_list)?;

    // collect unique lookup indices of all `kern` features
    let mut lookup_indices = Vec::new();
    let feature_count = u16_at(feature_list, 0)?;
    for i in 0..usize::from(feature_count) {
        let record = 2 + 6 * i;
        if feature_list.get(record..record + 4)? != b"kern" {
            continue;
        }
        let feature = table_slice(gpos, feature_list, u16_at(feature_list, record + 4)?)?;
        let lookup_count = u16_at(feature, 2)?;
        for j in 0..usize::from(lookup_count) {
            lookup_indices.push(u16_at(feature, 4 + 2 * j)?);
        }
    }
    lookup_indices.sort_unstable();
    lookup_indices.dedup();

    let mut lookups = Vec::new();
    for index in lookup_indices {
        let Some(lookup) = parse_lookup(gpos, lookup_list, index, glyph_count) else {
            continue;
        };
        if !lookup.subtables.is_empty() {
            lookups.push(lookup);
        }
    }
    Some(PairKerning { lookups })
}

fn parse_lookup(gpos: &[u8], lookup_list: &[u8], index: u16, glyph_count: usize) -> Option<Lookup> {
    let count = u16_at(lookup_list, 0)?;
    if index >= count {
        return None;
    }
    let lookup = table_slice(
        gpos,
        lookup_list,
        u16_at(lookup_list, 2 + 2 * usize::from(index))?,
    )?;
    let kind = u16_at(lookup, 0)?;
    let subtable_count = u16_at(lookup, 4)?;

    let mut subtables = Vec::new();
    for i in 0..usize::from(subtable_count) {
        let offset = u16_at(lookup, 6 + 2 * i)?;
        let Some(subtable) = table_slice(lookup, lookup, offset) else {
            continue;
        };
        let subtable = match kind {
            2 => parse_pair_adjustment(gpos, subtable, glyph_count),
            // extension positioning wraps another lookup kind
            9 => extension_subtable(gpos, subtable)
                .and_then(|(kind, subtable)| (kind == 2).then_some(subtable))
                .and_then(|subtable| parse_pair_adjustment(gpos, subtable, glyph_count)),
            _ => None,
        };
        if let Some(subtable) = subtable {
            subtables.push(subtable);
        }
    }

    Some(Lookup { subtables })
}

/// Returns the wrapped lookup kind & subtable of an extension positioning
/// subtable.
fn extension_subtable<'a>(gpos: &'a [u8], data: &'a [u8]) -> Option<(u16, &'a [u8])> {
    // format, extensionLookupType, extensionOffset32
    if u16_at(data, 0)? != 1 {
        return None;
    }
    let kind = u16_at(data, 2)?;
    let offset = u32::from_be_bytes(data.get(4..8)?.try_into().ok()?);
    let offset = u16::try_from(offset).ok()?;
    // extension offsets are relative to the start of the extension subtable
    Some((kind, table_slice(gpos, data, offset)?))
}

fn parse_pair_adjustment(gpos: &[u8], data: &[u8], glyph_count: usize) -> Option<Subtable> {
    match u16_at(data, 0)? {
        1 => parse_pair_sets(gpos, data),
        2 => parse_class_matrix(gpos, data, glyph_count),
        _ => None,
    }
}

/// Returns `(x_placement, x_advance)` of a value record at `offset`.
///
/// Only `x_placement` and `x_advance` are relevant to kerning; other fields
/// still take up space and are skipped.
fn read_value(data: &[u8], offset: usize, format: u16) -> Option<(i16, i16)> {
    let mut at = offset;
    let mut x_placement = 0;
    let mut x_advance = 0;
    if format & 0x0001 != 0 {
        x_placement = i16_at(data, at)?;
        at += 2;
    }
    if format & 0x0002 != 0 {
        at += 2;
    }
    if format & 0x0004 != 0 {
        x_advance = i16_at(data, at)?;
        at += 2;
    }
    if format & 0x0008 != 0 {
        at += 2;
    }
    // device table offsets are unused but still take up space
    for bit in [0x0010, 0x0020, 0x0040, 0x0080] {
        if format & bit != 0 {
            at += 2;
            data.get(at)?;
        }
    }
    Some((x_placement, x_advance))
}

fn i16_at(data: &[u8], offset: usize) -> Option<i16> {
    if let [a, b] = *data.get(offset..offset.checked_add(2)?)? {
        Some(i16::from_be_bytes([a, b]))
    } else {
        None
    }
}

/// Horizontal kerning adjustment of a value record pair.
fn value_adjustment(first: (i16, i16), second: (i16, i16)) -> i32 {
    i32::from(first.1) + i32::from(second.0)
}

/// Parses `count` consecutive value record pairs into one `x_advance` +
/// `x_placement` adjustment each.
fn read_value_pairs(
    data: &[u8],
    count: usize,
    format1: u16,
    format2: u16,
    values: &mut Vec<i32>,
) -> Option<()> {
    let len = record_len(format1) + record_len(format2);
    let first_len = record_len(format1);
    let data = data.get(..count.checked_mul(len)?)?;
    values.reserve(count);
    for record in data.chunks_exact(len) {
        let first = read_value(record, 0, format1)?;
        let second = read_value(record, first_len, format2)?;
        values.push(value_adjustment(first, second));
    }
    Some(())
}

fn parse_pair_sets(gpos: &[u8], data: &[u8]) -> Option<Subtable> {
    let coverage = table_slice(gpos, data, u16_at(data, 2)?)?;
    let format1 = u16_at(data, 4)?;
    let format2 = u16_at(data, 6)?;
    let pair_set_count = u16_at(data, 8)?;

    let mut covered = Vec::new();
    let mut firsts = Vec::new();
    for_each_coverage(gpos, coverage, |glyph, index| {
        if index >= pair_set_count {
            return;
        }
        let slot = usize::from(glyph);
        if covered.len() <= slot {
            covered.resize(slot + 1, 0u8);
        }
        covered[slot] = 1;
        firsts.push((glyph, index));
    });
    if firsts.is_empty() {
        return None;
    }

    let mut entries = Vec::new();
    for (glyph, set_index) in firsts {
        let offset = u16_at(data, 10 + 2 * usize::from(set_index))?;
        let set = table_slice(gpos, data, offset)?;
        let record_count = u16_at(set, 0)?;
        if entries.len().saturating_add(usize::from(record_count)) > MAX_PAIR_RECORDS {
            return None;
        }
        let len = 2 + record_len(format1) + record_len(format2);
        for i in 0..usize::from(record_count) {
            let record = 2 + i * len;
            let second = u16_at(set, record)?;
            let value1 = read_value(set, record + 2, format1)?;
            let value2 = read_value(set, record + 2 + record_len(format1), format2)?;
            entries.push((pair_key(glyph, second), value_adjustment(value1, value2)));
        }
    }

    Some(Subtable::PairSets {
        covered,
        pairs: PairHash::from_entries(&entries),
    })
}

/// Size of a value record: every set format bit is a 2 byte offset or value,
/// including the device table offsets.
fn record_len(format: u16) -> usize {
    format.count_ones() as usize * 2
}

fn parse_class_matrix(gpos: &[u8], data: &[u8], glyph_count: usize) -> Option<Subtable> {
    let coverage = table_slice(gpos, data, u16_at(data, 2)?)?;
    let format1 = u16_at(data, 4)?;
    let format2 = u16_at(data, 6)?;
    let class1_defs = table_slice(gpos, data, u16_at(data, 8)?)?;
    let class2_defs = table_slice(gpos, data, u16_at(data, 10)?)?;
    let class1_count = u16_at(data, 12)?;
    let class2_count = u16_at(data, 14)?;
    let matrix = data.get(16..)?;

    if class1_count == 0 || class2_count == 0 {
        return None;
    }
    if usize::from(class1_count) * usize::from(class2_count) > MAX_CLASS_MATRIX {
        return None;
    }
    let value_len = record_len(format1) + record_len(format2);
    if value_len == 0 {
        return None;
    }

    // covered glyphs, mapped to their (verified in range) class 1
    let mut class1 = Vec::new();
    class1.resize(glyph_count, UNCOVERED);
    let mut covered_any = false;
    for_each_coverage(gpos, coverage, |glyph, _| {
        let class = class_definition(gpos, class1_defs, glyph);
        if class >= class1_count {
            return;
        }
        if let Some(slot) = class1.get_mut(usize::from(glyph)) {
            *slot = class;
            covered_any = true;
        }
    });
    if !covered_any {
        return None;
    }

    let mut class2 = Vec::new();
    class2.resize(glyph_count, 0u16);
    // class 2 lookups are sequential while a binary search costs O(log n)
    class2_definition(gpos, class2_defs, &mut class2);

    let count = usize::from(class1_count) * usize::from(class2_count);
    let mut values = Vec::new();
    read_value_pairs(matrix, count, format1, format2, &mut values)?;

    Some(Subtable::ClassMatrix {
        class1,
        class2,
        class2_count,
        values,
    })
}

/// Fills `classes` (indexed by glyph id) from a class definition table.
///
/// Classes are written as-is; callers are responsible for range checking.
fn class2_definition(gpos: &[u8], data: &[u8], classes: &mut [u16]) {
    match u16_at(data, 0) {
        Some(1) => {
            let (Some(start), Some(count)) = (u16_at(data, 2), u16_at(data, 4)) else {
                return;
            };
            let Some(set) = table_slice(gpos, data, 6) else {
                return;
            };
            for index in 0..count {
                let glyph = start.wrapping_add(index);
                let Some(slot) = classes.get_mut(usize::from(glyph)) else {
                    continue;
                };
                *slot = u16_at(set, 2 * usize::from(index)).unwrap_or(0);
            }
        }
        Some(2) => {
            let Some(count) = u16_at(data, 2) else {
                return;
            };
            let Some(records) = table_slice(gpos, data, 4) else {
                return;
            };
            for index in 0..count {
                let Some(record) = table_slice(gpos, records, (6 * usize::from(index)) as u16)
                else {
                    return;
                };
                let (Some(start), Some(end), Some(class)) =
                    (u16_at(record, 0), u16_at(record, 2), u16_at(record, 4))
                else {
                    return;
                };
                for glyph in start..=end {
                    if let Some(slot) = classes.get_mut(usize::from(glyph)) {
                        *slot = class;
                    }
                }
            }
        }
        _ => {}
    }
}

/// Returns the class of `glyph` per a class definition table.
fn class_definition(gpos: &[u8], data: &[u8], glyph: u16) -> u16 {
    match u16_at(data, 0) {
        Some(1) => {
            let (Some(start), Some(count)) = (u16_at(data, 2), u16_at(data, 4)) else {
                return 0;
            };
            let Some(set) = table_slice(gpos, data, 6) else {
                return 0;
            };
            match glyph.checked_sub(start) {
                Some(index) if index < count => u16_at(set, 2 * usize::from(index)).unwrap_or(0),
                _ => 0,
            }
        }
        Some(2) => {
            let Some(count) = u16_at(data, 2) else {
                return 0;
            };
            let Some(records) = table_slice(gpos, data, 4) else {
                return 0;
            };
            // range records are sorted by start glyph id
            let mut low = 0u16;
            let mut high = count;
            while low < high {
                let mid = low + (high - low) / 2;
                let Some(record) = table_slice(gpos, records, (6 * usize::from(mid)) as u16) else {
                    return 0;
                };
                let (Some(start), Some(end), Some(class)) =
                    (u16_at(record, 0), u16_at(record, 2), u16_at(record, 4))
                else {
                    return 0;
                };
                if glyph < start {
                    high = mid;
                } else if glyph > end {
                    low = mid + 1;
                } else {
                    return class;
                }
            }
            0
        }
        _ => 0,
    }
}

/// Iterates `(glyph id, coverage index)` pairs of a coverage table.
fn for_each_coverage(gpos: &[u8], data: &[u8], mut f: impl FnMut(u16, u16)) {
    let Some(format) = u16_at(data, 0) else {
        return;
    };
    let Some(count) = u16_at(data, 2) else {
        return;
    };
    match format {
        1 => {
            for index in 0..count {
                if let Some(glyph) = u16_at(data, 4 + 2 * usize::from(index)) {
                    f(glyph, index);
                }
            }
        }
        2 => {
            for index in 0..count {
                let Some(record) = table_slice(gpos, data, (4 + 6 * usize::from(index)) as u16)
                else {
                    return;
                };
                let (Some(start), Some(end), Some(value)) =
                    (u16_at(record, 0), u16_at(record, 2), u16_at(record, 4))
                else {
                    return;
                };
                for (offset, glyph) in (0u16..).zip(start..=end) {
                    if let Some(coverage_index) = offset.checked_add(value) {
                        f(glyph, coverage_index);
                    }
                }
            }
        }
        _ => {}
    }
}

/// One slot of the [`PairHash`] table.
#[derive(Clone, Copy, Default)]
struct PairSlot {
    /// `(first << 16) | second`.
    key: u32,
    /// Pair adjustment, only meaningful when `used`.
    value: i32,
    used: bool,
}

/// Open addressing hash map `u32 -> i32` used for format 1 pair records.
///
/// Records may have a zero adjustment, and presence still matters, so empty
/// slots are tracked separately from the key/value pair.
#[derive(Clone, Default)]
struct PairHash {
    /// `slots.len() - 1`, 0 when empty.
    mask: usize,
    slots: Vec<PairSlot>,
}

impl PairHash {
    fn from_entries(entries: &[(u32, i32)]) -> Self {
        if entries.is_empty() {
            return Self::default();
        }

        let mut capacity = 2;
        while capacity < entries.len().saturating_mul(2) {
            capacity <<= 1;
        }
        let mut hash = Self {
            mask: capacity - 1,
            slots: Vec::new(),
        };
        hash.slots.resize(capacity, PairSlot::default());
        for &(key, value) in entries {
            hash.insert(key, value);
        }
        hash
    }

    #[inline]
    fn index(&self, key: u32) -> usize {
        let mut x = key;
        x ^= x >> 16;
        x = x.wrapping_mul(0x7feb_352d);
        x ^= x >> 15;
        x = x.wrapping_mul(0x846c_a68b);
        x ^= x >> 16;
        x as usize & self.mask
    }

    fn insert(&mut self, key: u32, value: i32) {
        let mut index = self.index(key);
        loop {
            let slot = &mut self.slots[index];
            if !slot.used || slot.key == key {
                *slot = PairSlot {
                    key,
                    value,
                    used: true,
                };
                return;
            }
            index = (index + 1) & self.mask;
        }
    }

    #[inline]
    fn get(&self, key: u32) -> Option<i32> {
        if self.slots.is_empty() {
            return None;
        }
        let mut index = self.index(key);
        loop {
            let slot = &self.slots[index];
            if !slot.used {
                return None;
            }
            if slot.key == key {
                return Some(slot.value);
            }
            index = (index + 1) & self.mask;
        }
    }
}
