//! Bit-level serialization utilities and variable-length integer codecs.
//!
//! Provides zero-allocation streaming bit reading and writing for compact
//! delta state replication, and unsigned LEB128 variable-length integer encoding.

use bytes::{Buf, BufMut, BytesMut};

use crate::client::ClientCodecError;

/// Maximum number of bytes in an LEB128-encoded 64-bit integer.
pub const MAX_VARINT_U64_BYTES: usize = 10;

/// Maximum number of bytes in an LEB128-encoded 32-bit integer.
pub const MAX_VARINT_U32_BYTES: usize = 5;

/// Serializes an unsigned 64-bit integer into a buffer using LEB128 varint encoding.
#[inline]
pub fn put_varint_u64<B: BufMut>(dst: &mut B, mut val: u64) {
    while val >= 0x80 {
        dst.put_u8((val as u8 & 0x7F) | 0x80);
        val >>= 7;
    }
    dst.put_u8(val as u8);
}

/// Deserializes an unsigned 64-bit integer from a buffer using LEB128 varint encoding.
///
/// Returns `ClientCodecError::BufferTooShort` if the buffer ends prematurely,
/// or `ClientCodecError::InvalidData` if the varint exceeds 10 bytes.
#[inline]
pub fn get_varint_u64<B: Buf>(src: &mut B) -> Result<u64, ClientCodecError> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;

    for byte_count in 0..MAX_VARINT_U64_BYTES {
        if !src.has_remaining() {
            return Err(ClientCodecError::BufferTooShort {
                expected: byte_count + 1,
                got: byte_count,
            });
        }
        let byte = src.get_u8();
        result |= ((byte & 0x7F) as u64) << shift;

        if (byte & 0x80) == 0 {
            return Ok(result);
        }
        shift += 7;
    }

    Err(ClientCodecError::InvalidData(
        "Varint u64 exceeds maximum allowed 10 bytes".to_string(),
    ))
}

/// Serializes an unsigned 32-bit integer into a buffer using LEB128 varint encoding.
#[inline]
pub fn put_varint_u32<B: BufMut>(dst: &mut B, mut val: u32) {
    while val >= 0x80 {
        dst.put_u8((val as u8 & 0x7F) | 0x80);
        val >>= 7;
    }
    dst.put_u8(val as u8);
}

/// Deserializes an unsigned 32-bit integer from a buffer using LEB128 varint encoding.
#[inline]
pub fn get_varint_u32<B: Buf>(src: &mut B) -> Result<u32, ClientCodecError> {
    let val = get_varint_u64(src)?;
    if val > u32::MAX as u64 {
        return Err(ClientCodecError::InvalidData(
            "Varint value exceeds u32::MAX".to_string(),
        ));
    }
    Ok(val as u32)
}

/// Bit-level stream writer for compact arbitrary bit-width serialization.
#[derive(Debug, Default, Clone)]
pub struct BitWriter {
    buffer: BytesMut,
    current_byte: u8,
    bit_offset: u8, // 0..7: next bit index to write into current_byte (LSB first)
}

impl BitWriter {
    /// Creates a new `BitWriter` with pre-allocated capacity in bytes.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer: BytesMut::with_capacity(capacity),
            current_byte: 0,
            bit_offset: 0,
        }
    }

    /// Writes a single boolean as 1 bit.
    #[inline]
    pub fn write_bool(&mut self, val: bool) {
        self.write_bits(if val { 1 } else { 0 }, 1);
    }

    /// Writes `num_bits` (0..=64) from `val` into the bitstream (least significant bits first).
    pub fn write_bits(&mut self, mut val: u64, mut num_bits: usize) {
        if num_bits > 64 {
            num_bits = 64;
        }

        while num_bits > 0 {
            let bits_free_in_byte = (8 - self.bit_offset) as usize;
            let bits_to_write = bits_free_in_byte.min(num_bits);
            let mask = (1u64 << bits_to_write) - 1;
            let chunk = (val & mask) as u8;

            self.current_byte |= chunk << self.bit_offset;
            self.bit_offset += bits_to_write as u8;
            val >>= bits_to_write;
            num_bits -= bits_to_write;

            if self.bit_offset == 8 {
                self.buffer.put_u8(self.current_byte);
                self.current_byte = 0;
                self.bit_offset = 0;
            }
        }
    }

    /// Flushes any pending bits into the byte buffer, zero-padding the remaining bits to a full byte.
    #[inline]
    pub fn flush_byte_align(&mut self) {
        if self.bit_offset > 0 {
            self.buffer.put_u8(self.current_byte);
            self.current_byte = 0;
            self.bit_offset = 0;
        }
    }

    /// Appends a raw byte slice, automatically ensuring byte alignment first.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.flush_byte_align();
        self.buffer.put_slice(bytes);
    }

    /// Returns the number of fully or partially committed bytes currently in the writer.
    pub fn byte_len(&self) -> usize {
        self.buffer.len() + if self.bit_offset > 0 { 1 } else { 0 }
    }

    /// Finalizes the bitstream by aligning to byte boundary and returning the underlying bytes.
    pub fn finish(mut self) -> BytesMut {
        self.flush_byte_align();
        self.buffer
    }
}

/// Bit-level stream reader for decoding arbitrary bit-width packed data.
#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_offset: u8, // 0..7
}

impl<'a> BitReader<'a> {
    /// Creates a new `BitReader` borrowing the given slice.
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_offset: 0,
        }
    }

    /// Returns the total number of unread bits remaining in the buffer.
    pub fn remaining_bits(&self) -> usize {
        let remaining_full_bytes = self.data.len().saturating_sub(self.byte_pos);
        if remaining_full_bytes == 0 {
            0
        } else {
            (remaining_full_bytes * 8).saturating_sub(self.bit_offset as usize)
        }
    }

    /// Reads a single boolean bit.
    #[inline]
    pub fn read_bool(&mut self) -> Result<bool, ClientCodecError> {
        let bits = self.read_bits(1)?;
        Ok(bits == 1)
    }

    /// Reads `num_bits` (0..=64) from the bitstream.
    pub fn read_bits(&mut self, mut num_bits: usize) -> Result<u64, ClientCodecError> {
        if num_bits > 64 {
            return Err(ClientCodecError::InvalidData(
                "Cannot read more than 64 bits in a single read_bits call".to_string(),
            ));
        }

        if self.remaining_bits() < num_bits {
            return Err(ClientCodecError::BufferTooShort {
                expected: num_bits.div_ceil(8),
                got: self.remaining_bits().div_ceil(8),
            });
        }

        let mut result: u64 = 0;
        let mut bits_read: usize = 0;

        while num_bits > 0 {
            let bits_left_in_byte = (8 - self.bit_offset) as usize;
            let bits_to_read = bits_left_in_byte.min(num_bits);
            let mask = ((1u16 << bits_to_read) - 1) as u8;

            let byte_val = self.data[self.byte_pos];
            let chunk = ((byte_val >> self.bit_offset) & mask) as u64;

            result |= chunk << bits_read;
            bits_read += bits_to_read;
            self.bit_offset += bits_to_read as u8;
            num_bits -= bits_to_read;

            if self.bit_offset == 8 {
                self.byte_pos += 1;
                self.bit_offset = 0;
            }
        }

        Ok(result)
    }

    /// Advances the read pointer to the next whole byte boundary if currently misaligned.
    #[inline]
    pub fn align_byte(&mut self) {
        if self.bit_offset > 0 {
            self.byte_pos += 1;
            self.bit_offset = 0;
        }
    }

    /// Reads an exact slice of raw bytes, automatically aligning to the next byte boundary first.
    pub fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], ClientCodecError> {
        self.align_byte();
        if self.data.len() - self.byte_pos < len {
            return Err(ClientCodecError::BufferTooShort {
                expected: len,
                got: self.data.len() - self.byte_pos,
            });
        }
        let slice = &self.data[self.byte_pos..self.byte_pos + len];
        self.byte_pos += len;
        Ok(slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_varint_u64_roundtrip() {
        let test_values = [
            0u64,
            1,
            63,
            127,
            128,
            255,
            300,
            16383,
            16384,
            65535,
            1_000_000,
            u32::MAX as u64,
            (u32::MAX as u64) + 1,
            u64::MAX / 2,
            u64::MAX,
        ];

        let mut buf = BytesMut::new();
        for &val in &test_values {
            put_varint_u64(&mut buf, val);
        }

        for &expected in &test_values {
            let decoded = get_varint_u64(&mut buf).expect("Failed to decode varint");
            assert_eq!(expected, decoded);
        }
        assert!(!buf.has_remaining());
    }

    #[test]
    fn test_varint_u32_roundtrip() {
        let test_values = [0u32, 1, 127, 128, 255, 16384, 1_000_000, u32::MAX];
        let mut buf = BytesMut::new();
        for &val in &test_values {
            put_varint_u32(&mut buf, val);
        }

        for &expected in &test_values {
            let decoded = get_varint_u32(&mut buf).expect("Failed to decode varint u32");
            assert_eq!(expected, decoded);
        }
    }

    #[test]
    fn test_varint_wire_sizes() {
        let mut buf = BytesMut::new();
        put_varint_u64(&mut buf, 0);
        assert_eq!(buf.len(), 1);
        buf.clear();

        put_varint_u64(&mut buf, 127);
        assert_eq!(buf.len(), 1);
        buf.clear();

        put_varint_u64(&mut buf, 128);
        assert_eq!(buf.len(), 2);
        buf.clear();

        put_varint_u64(&mut buf, 16383);
        assert_eq!(buf.len(), 2);
        buf.clear();

        put_varint_u64(&mut buf, 16384);
        assert_eq!(buf.len(), 3);
    }

    #[test]
    fn test_varint_buffer_too_short() {
        let mut buf = BytesMut::from(&[0x80u8][..]); // Needs next byte
        assert!(matches!(
            get_varint_u64(&mut buf),
            Err(ClientCodecError::BufferTooShort { .. })
        ));
    }

    #[test]
    fn test_varint_exceeds_max_bytes() {
        // 11 bytes all with MSB set
        let bad_bytes = [0x80u8; 11];
        let mut buf = BytesMut::from(&bad_bytes[..]);
        assert!(matches!(
            get_varint_u64(&mut buf),
            Err(ClientCodecError::InvalidData(_))
        ));
    }

    #[test]
    fn test_bitwriter_and_bitreader_basic() {
        let mut writer = BitWriter::with_capacity(16);
        writer.write_bool(true);
        writer.write_bool(false);
        writer.write_bits(5, 3); // 101 in binary (3 bits)
        writer.write_bits(42, 6); // 101010 in binary (6 bits)
        writer.write_bits(0xABCD, 16); // 16 bits
        writer.write_bool(true);

        let bytes = writer.finish();

        let mut reader = BitReader::new(&bytes);
        assert!(reader.read_bool().unwrap());
        assert!(!reader.read_bool().unwrap());
        assert_eq!(reader.read_bits(3).unwrap(), 5);
        assert_eq!(reader.read_bits(6).unwrap(), 42);
        assert_eq!(reader.read_bits(16).unwrap(), 0xABCD);
        assert!(reader.read_bool().unwrap());
    }

    #[test]
    fn test_bitwriter_alignment_and_bytes() {
        let mut writer = BitWriter::with_capacity(32);
        writer.write_bits(3, 2); // 2 bits
        writer.write_bytes(&[10, 20, 30]); // Should align to byte boundary first
        writer.write_bits(7, 3); // 3 bits

        let bytes = writer.finish();
        assert_eq!(bytes.len(), 5); // 1 byte (2 bits + pad) + 3 bytes + 1 byte (3 bits + pad)

        let mut reader = BitReader::new(&bytes);
        assert_eq!(reader.read_bits(2).unwrap(), 3);
        let raw = reader.read_bytes(3).unwrap();
        assert_eq!(raw, &[10, 20, 30]);
        assert_eq!(reader.read_bits(3).unwrap(), 7);
    }

    #[test]
    fn test_bitreader_buffer_too_short() {
        let bytes = [0b00000001];
        let mut reader = BitReader::new(&bytes);
        assert!(reader.read_bits(9).is_err());
    }
}
