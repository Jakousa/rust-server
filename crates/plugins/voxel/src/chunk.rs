//! Dynamic bit-packed palette-compressed chunk storage.

use serde::{Deserialize, Serialize};

use crate::coords::CHUNK_VOXELS;

#[inline(always)]
const fn entries_per_word(bits: u8) -> usize {
    match bits {
        1 => 64,
        2 => 32,
        4 => 16,
        8 => 8,
        16 => 4,
        _ => 64,
    }
}

#[inline(always)]
const fn words_needed(bits: u8) -> usize {
    match bits {
        0 => 0,
        1 => CHUNK_VOXELS / 64, // 64
        2 => CHUNK_VOXELS / 32, // 128
        4 => CHUNK_VOXELS / 16, // 256
        8 => CHUNK_VOXELS / 8,  // 512
        16 => CHUNK_VOXELS / 4, // 1024
        _ => 1024,
    }
}

#[inline(always)]
const fn next_bits_per_entry(current: u8) -> u8 {
    match current {
        0 => 1,
        1 => 2,
        2 => 4,
        4 => 8,
        8 => 16,
        _ => 16,
    }
}

/// A 16×16×16 chunk (4,096 voxels) stored with dynamic bit-packed palette compression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteChunk {
    pub(crate) palette: Vec<u16>,
    pub(crate) bits_per_entry: u8,
    pub(crate) data: Vec<u64>,
    pub(crate) revision: u64,
    pub(crate) dirty: bool,
}

impl PaletteChunk {
    /// Creates a homogeneous chunk filled entirely with `voxel_type`.
    /// Consumes zero words in `data` buffer (0 bits per entry).
    pub fn new_homogeneous(voxel_type: u16) -> Self {
        Self {
            palette: vec![voxel_type],
            bits_per_entry: 0,
            data: Vec::new(),
            revision: 0,
            dirty: false,
        }
    }

    /// Creates an empty chunk filled with Air (`0`).
    pub fn new_empty() -> Self {
        Self::new_homogeneous(0)
    }

    /// Retrieves the voxel type at flat index `idx` (0..4095).
    #[inline]
    pub fn get(&self, idx: usize) -> u16 {
        assert!(idx < CHUNK_VOXELS, "Voxel index out of bounds");
        if self.bits_per_entry == 0 {
            return self.palette.first().copied().unwrap_or(0);
        }

        let epw = entries_per_word(self.bits_per_entry);
        let word_idx = idx / epw;
        let bit_offset = (idx % epw) * self.bits_per_entry as usize;
        let mask = (1u64 << self.bits_per_entry) - 1;
        let pal_idx = ((self.data[word_idx] >> bit_offset) & mask) as usize;
        self.palette.get(pal_idx).copied().unwrap_or(0)
    }

    /// Sets the voxel type at flat index `idx` (0..4095).
    ///
    /// Returns `true` if the voxel changed, or `false` if it was already `voxel_type`.
    pub fn set(&mut self, idx: usize, voxel_type: u16) -> bool {
        assert!(idx < CHUNK_VOXELS, "Voxel index out of bounds");
        let current = self.get(idx);
        if current == voxel_type {
            return false;
        }

        let pal_idx = match self.palette.iter().position(|&v| v == voxel_type) {
            Some(existing) => existing,
            None => {
                let needed_capacity = self.palette.len() + 1;
                let current_capacity = if self.bits_per_entry == 0 {
                    1
                } else {
                    1usize << self.bits_per_entry
                };

                if needed_capacity > current_capacity {
                    let next_bits = next_bits_per_entry(self.bits_per_entry);
                    self.repack(next_bits);
                }

                let new_idx = self.palette.len();
                self.palette.push(voxel_type);
                new_idx
            }
        };

        if self.bits_per_entry > 0 {
            let epw = entries_per_word(self.bits_per_entry);
            let word_idx = idx / epw;
            let bit_offset = (idx % epw) * self.bits_per_entry as usize;
            let mask = (1u64 << self.bits_per_entry) - 1;
            self.data[word_idx] = (self.data[word_idx] & !(mask << bit_offset))
                | ((pal_idx as u64 & mask) << bit_offset);
        }

        self.revision += 1;
        self.dirty = true;
        true
    }

    /// Repacks the chunk into a larger bit-depth without corrupting existing voxels.
    fn repack(&mut self, next_bits: u8) {
        let mut old_pal_indices = Vec::with_capacity(CHUNK_VOXELS);
        if self.bits_per_entry == 0 {
            old_pal_indices.resize(CHUNK_VOXELS, 0u16);
        } else {
            let old_epw = entries_per_word(self.bits_per_entry);
            let old_mask = (1u64 << self.bits_per_entry) - 1;
            for i in 0..CHUNK_VOXELS {
                let w_idx = i / old_epw;
                let b_off = (i % old_epw) * self.bits_per_entry as usize;
                let pal_idx = ((self.data[w_idx] >> b_off) & old_mask) as u16;
                old_pal_indices.push(pal_idx);
            }
        }

        self.bits_per_entry = next_bits;
        let word_count = words_needed(next_bits);
        let mut new_data = vec![0u64; word_count];
        let new_epw = entries_per_word(next_bits);

        for (i, &pal_idx) in old_pal_indices.iter().enumerate() {
            let w_idx = i / new_epw;
            let b_off = (i % new_epw) * next_bits as usize;
            new_data[w_idx] |= (pal_idx as u64) << b_off;
        }

        self.data = new_data;
    }

    /// Unpacks all 4,096 voxels into a flat array.
    pub fn unpack_all(&self) -> Box<[u16; CHUNK_VOXELS]> {
        let mut result = Box::new([0u16; CHUNK_VOXELS]);
        if self.bits_per_entry == 0 {
            let val = self.palette.first().copied().unwrap_or(0);
            result.fill(val);
            return result;
        }

        let epw = entries_per_word(self.bits_per_entry);
        let mask = (1u64 << self.bits_per_entry) - 1;
        for i in 0..CHUNK_VOXELS {
            let word_idx = i / epw;
            let bit_offset = (i % epw) * self.bits_per_entry as usize;
            let pal_idx = ((self.data[word_idx] >> bit_offset) & mask) as usize;
            result[i] = self.palette.get(pal_idx).copied().unwrap_or(0);
        }
        result
    }

    /// Constructs an optimally-compressed `PaletteChunk` from a raw 4,096 voxel array.
    pub fn from_raw_voxels(voxels: &[u16; CHUNK_VOXELS]) -> Self {
        let mut unique = Vec::new();
        for &v in voxels {
            if !unique.contains(&v) {
                unique.push(v);
            }
        }

        if unique.len() <= 1 {
            let val = unique.first().copied().unwrap_or(0);
            return Self::new_homogeneous(val);
        }

        let mut bits = 1;
        while (1usize << bits) < unique.len() && bits < 16 {
            bits = next_bits_per_entry(bits);
        }

        let epw = entries_per_word(bits);
        let word_count = words_needed(bits);
        let mut data = vec![0u64; word_count];

        for (i, &v) in voxels.iter().enumerate() {
            let pal_idx = unique.iter().position(|&u| u == v).unwrap_or(0);
            let w_idx = i / epw;
            let b_off = (i % epw) * bits as usize;
            data[w_idx] |= (pal_idx as u64) << b_off;
        }

        Self {
            palette: unique,
            bits_per_entry: bits,
            data,
            revision: 0,
            dirty: false,
        }
    }

    /// Returns `true` if the chunk is homogeneous (all voxels identical).
    pub fn is_homogeneous(&self) -> bool {
        self.bits_per_entry == 0 || self.palette.len() <= 1
    }

    /// Marks the chunk as clean after persistence or sync.
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    /// Returns whether the chunk has pending unpersisted modifications.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Monotonically increasing revision number.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Number of palette entries.
    pub fn palette_len(&self) -> usize {
        self.palette.len()
    }

    /// Current bit depth per voxel (0, 1, 2, 4, 8, or 16).
    pub fn bits_per_entry(&self) -> u8 {
        self.bits_per_entry
    }

    /// Estimated memory usage in bytes of this chunk.
    pub fn memory_usage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.palette.capacity() * std::mem::size_of::<u16>()
            + self.data.capacity() * std::mem::size_of::<u64>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_homogeneous_chunk_memory_and_get_set() {
        let mut chunk = PaletteChunk::new_homogeneous(1);
        assert_eq!(chunk.bits_per_entry(), 0);
        assert!(chunk.is_homogeneous());
        assert_eq!(chunk.get(0), 1);
        assert_eq!(chunk.get(4095), 1);
        assert!(!chunk.set(0, 1)); // no-op
        assert_eq!(chunk.revision(), 0);

        // Modifying 1 block upgrades to 1 bit
        assert!(chunk.set(0, 2));
        assert_eq!(chunk.revision(), 1);
        assert_eq!(chunk.bits_per_entry(), 1);
        assert_eq!(chunk.get(0), 2);
        assert_eq!(chunk.get(1), 1);
        assert_eq!(chunk.get(4095), 1);
    }

    #[test]
    fn test_bit_depth_resizing_and_integrity() {
        let mut chunk = PaletteChunk::new_empty();
        assert_eq!(chunk.bits_per_entry(), 0);

        // Add 2nd type -> 1 bit (capacity 2: types 0, 10)
        assert!(chunk.set(10, 10));
        assert_eq!(chunk.bits_per_entry(), 1);
        assert_eq!(chunk.get(10), 10);
        assert_eq!(chunk.get(9), 0);

        // Add 3rd type -> 2 bits (capacity 4: types 0, 10, 20)
        assert!(chunk.set(20, 20));
        assert_eq!(chunk.bits_per_entry(), 2);
        assert_eq!(chunk.get(10), 10);
        assert_eq!(chunk.get(20), 20);
        assert_eq!(chunk.get(0), 0);

        // Add 4th type (still 2 bits)
        assert!(chunk.set(30, 30));
        assert_eq!(chunk.bits_per_entry(), 2);

        // Add 5th type -> 4 bits (capacity 16)
        assert!(chunk.set(40, 40));
        assert_eq!(chunk.bits_per_entry(), 4);
        assert_eq!(chunk.get(10), 10);
        assert_eq!(chunk.get(20), 20);
        assert_eq!(chunk.get(30), 30);
        assert_eq!(chunk.get(40), 40);

        // Fill up to 17 unique types -> 8 bits
        for i in 5..=17 {
            chunk.set(i + 50, (i * 10) as u16);
        }
        assert_eq!(chunk.bits_per_entry(), 8);
        assert_eq!(chunk.get(10), 10);
        assert_eq!(chunk.get(20), 20);
        assert_eq!(chunk.get(40), 40);
        assert_eq!(chunk.get(60), 100);
    }

    #[test]
    fn test_from_raw_voxels_roundtrip() {
        let mut raw = [0u16; CHUNK_VOXELS];
        for (i, item) in raw.iter_mut().enumerate() {
            *item = (i % 7) as u16;
        }

        let chunk = PaletteChunk::from_raw_voxels(&raw);
        assert_eq!(chunk.bits_per_entry(), 4); // 7 unique types fits in 4 bits
        let unpacked = chunk.unpack_all();
        assert_eq!(*unpacked, raw);
    }
}
