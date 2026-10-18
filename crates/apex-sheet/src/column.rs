//! The cells of one column: blocks of 64 rows, each a bit set of the rows
//! it has and their cells in order, kept in a B-tree by block. A full
//! column costs little more than its cells (a tree node per cell cost
//! about as much again), and a lone cell about the same as in a tree.

use std::collections::BTreeMap;

use crate::sheet::Cell;

const BITS: u32 = 6;
const LOW: u32 = (1 << BITS) - 1;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Column {
    blocks: BTreeMap<u32, Block>,
    len: usize,
}

#[derive(Clone, Debug, PartialEq)]
struct Block {
    /// Bit `i`: row `key * 64 + i` has a cell.
    mask: u64,
    /// The cells, by row.
    cells: Vec<Cell>,
}

/// Rows (0..64) of the set bits of `mask`, in order.
fn bits(mut mask: u64) -> impl Iterator<Item = u32> {
    std::iter::from_fn(move || {
        (mask != 0).then(|| {
            let b = mask.trailing_zeros();
            mask &= mask - 1;
            b
        })
    })
}

impl Block {
    /// Index in `cells` of row bit `bit` (set or not).
    #[inline]
    fn rank(&self, bit: u32) -> usize {
        (self.mask & ((1u64 << bit) - 1)).count_ones() as usize
    }
}

impl Column {
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn get(&self, row: u32) -> Option<&Cell> {
        let b = self.blocks.get(&(row >> BITS))?;
        let bit = row & LOW;
        (b.mask >> bit & 1 == 1).then(|| &b.cells[b.rank(bit)])
    }

    pub fn get_mut(&mut self, row: u32) -> Option<&mut Cell> {
        let b = self.blocks.get_mut(&(row >> BITS))?;
        let bit = row & LOW;
        if b.mask >> bit & 1 == 0 {
            return None;
        }
        let i = b.rank(bit);
        Some(&mut b.cells[i])
    }

    /// Stores a cell; returns the one it replaces.
    pub fn insert(&mut self, row: u32, cell: Cell) -> Option<Cell> {
        let bit = row & LOW;
        // Files fill a column top to bottom: mostly its last block.
        let b = match self.blocks.last_entry() {
            Some(e) if *e.key() == row >> BITS => e.into_mut(),
            _ => self.blocks.entry(row >> BITS).or_insert_with(|| Block { mask: 0, cells: Vec::with_capacity(1) }),
        };
        let i = b.rank(bit);
        if b.mask >> bit & 1 == 1 {
            return Some(std::mem::replace(&mut b.cells[i], cell));
        }
        b.mask |= 1 << bit;
        b.cells.insert(i, cell);
        self.len += 1;
        None
    }

    pub fn remove(&mut self, row: u32) -> Option<Cell> {
        let key = row >> BITS;
        let b = self.blocks.get_mut(&key)?;
        let bit = row & LOW;
        if b.mask >> bit & 1 == 0 {
            return None;
        }
        let cell = b.cells.remove(b.rank(bit));
        b.mask &= !(1 << bit);
        self.len -= 1;
        if b.mask == 0 {
            self.blocks.remove(&key);
        } else if b.cells.capacity() >= 2 * b.cells.len() + 8 {
            b.cells.shrink_to_fit();
        }
        Some(cell)
    }

    /// Row of the last cell.
    pub fn last_row(&self) -> Option<u32> {
        let (k, b) = self.blocks.last_key_value()?;
        Some(k << BITS | (63 - b.mask.leading_zeros()))
    }

    /// Cells of rows `lo..=hi`, top to bottom.
    pub fn range(&self, lo: u32, hi: u32) -> impl Iterator<Item = (u32, &Cell)> {
        let blocks = (lo <= hi).then(|| self.blocks.range(lo >> BITS..=hi >> BITS)).into_iter().flatten();
        blocks.flat_map(|(&k, b)| bits(b.mask).map(move |i| k << BITS | i).zip(&b.cells)).skip_while(move |(r, _)| *r < lo).take_while(move |(r, _)| *r <= hi)
    }

    pub fn range_mut(&mut self, lo: u32, hi: u32) -> impl Iterator<Item = (u32, &mut Cell)> {
        let blocks = (lo <= hi).then(|| self.blocks.range_mut(lo >> BITS..=hi >> BITS)).into_iter().flatten();
        blocks.flat_map(|(&k, b)| bits(b.mask).map(move |i| k << BITS | i).zip(&mut b.cells)).skip_while(move |(r, _)| *r < lo).take_while(move |(r, _)| *r <= hi)
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &Cell)> {
        // All of it: no rows to skip or stop at.
        self.blocks.iter().flat_map(|(&k, b)| bits(b.mask).map(move |i| k << BITS | i).zip(&b.cells))
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut Cell> {
        self.blocks.values_mut().flat_map(|b| b.cells.iter_mut())
    }

    /// Takes out the cells of rows `at..`, top to bottom.
    pub fn split_off(&mut self, at: u32) -> Vec<(u32, Cell)> {
        let mut tail = self.blocks.split_off(&(at >> BITS));
        let mut out = Vec::new();
        // The block holding `at` may have rows above it that stay.
        if let Some(mut first) = tail.remove(&(at >> BITS)) {
            let key = at >> BITS;
            let keep = first.rank(at & LOW);
            let rows: Vec<u32> = bits(first.mask).collect();
            let moved = first.cells.split_off(keep);
            out.extend(rows[keep..].iter().map(|&i| key << BITS | i).zip(moved));
            if keep > 0 {
                first.mask &= (1u64 << (at & LOW)) - 1;
                self.blocks.insert(key, first);
            }
        }
        for (k, b) in tail {
            out.extend(bits(b.mask).map(|i| k << BITS | i).zip(b.cells));
        }
        self.len -= out.len();
        out
    }

    /// Adds the cells of `other`; a block holding rows of both is merged,
    /// the rest of `other` moves as it is.
    pub fn append(&mut self, other: Column) {
        let Column { mut blocks, len } = other;
        self.len += len;
        if let Some(&last) = self.blocks.keys().next_back() {
            let below = blocks.split_off(&(last + 1));
            for (k, b) in std::mem::replace(&mut blocks, below) {
                for (i, cell) in bits(b.mask).zip(b.cells) {
                    // Counted above; `insert` counts it again unless it replaces one.
                    self.len -= 1;
                    self.insert(k << BITS | i, cell);
                }
            }
        }
        self.blocks.append(&mut blocks);
    }

    pub fn into_cells(self) -> impl Iterator<Item = (u32, Cell)> {
        self.blocks.into_iter().flat_map(|(k, b)| bits(b.mask).map(move |i| k << BITS | i).zip(b.cells))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::Content;
    use crate::value::Value;

    fn cell(n: f64) -> Cell {
        Cell::new(Content::Value(Value::Number(n)), 0)
    }

    /// The same edits on a column and on a map give the same cells.
    #[test]
    fn matches_a_map() {
        let mut col = Column::default();
        let mut map: BTreeMap<u32, Cell> = BTreeMap::new();
        let mut x: u64 = 12345;
        for step in 0..20_000u32 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let row = ((x >> 33) % 700) as u32 + if step % 7 == 0 { 1_048_000 } else { 0 };
            if x >> 20 & 3 == 0 {
                assert_eq!(col.remove(row), map.remove(&row));
            } else {
                assert_eq!(col.insert(row, cell(step as f64)), map.insert(row, cell(step as f64)));
            }
            if step % 2000 == 0 {
                let (lo, hi) = ((x >> 10) as u32 % 800, (x >> 40) as u32 % 900);
                let a: Vec<(u32, &Cell)> = col.range(lo, hi).collect();
                let b: Vec<(u32, &Cell)> = if lo <= hi { map.range(lo..=hi).map(|(r, c)| (*r, c)).collect() } else { Vec::new() };
                assert_eq!(a, b, "{lo}..={hi}");
            }
        }
        assert_eq!(col.len(), map.len());
        assert_eq!(col.last_row(), map.last_key_value().map(|(r, _)| *r));
        for r in 0..800 {
            assert_eq!(col.get(r), map.get(&r));
        }
        let at = 333;
        let tail = col.split_off(at);
        let mtail = map.split_off(&at);
        assert_eq!(tail, mtail.into_iter().collect::<Vec<_>>());
        assert_eq!(col.iter().map(|(r, c)| (r, c.clone())).collect::<Vec<_>>(), map.into_iter().collect::<Vec<_>>());
        assert_eq!(col.len(), col.iter().count());
    }

    /// Appending a column gives the same cells as inserting them, also
    /// where both have rows of one block.
    #[test]
    fn append_matches_inserts() {
        for split in [0, 1, 63, 64, 100, 130, 500] {
            let (mut a, mut b, mut whole) = (Column::default(), Column::default(), Column::default());
            for r in (0..600).filter(|r| r % 3 != 1) {
                whole.insert(r, cell(r as f64));
                if r < split { a.insert(r, cell(r as f64)) } else { b.insert(r, cell(r as f64)) };
            }
            a.append(b);
            assert_eq!(a.len(), whole.len());
            assert_eq!(a.iter().collect::<Vec<_>>(), whole.iter().collect::<Vec<_>>());
            assert_eq!(a, whole);
        }
    }
}
