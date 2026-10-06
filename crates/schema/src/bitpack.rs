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

    /// Returns the capacity of the underlying byte buffer in bytes.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.buffer.capacity()
    }

    /// Writes a single boolean as 1 bit.
    #[inline]
    pub fn write_bool(&mut self, val: bool) {
        self.write_bits(if val { 1 } else { 0 }, 1);
    }

    /// Writes `num_bits` (0..=64) from `val` into the bitstream (least significant bits first).
    pub fn write_bits(&mut self, mut val: u64, mut num_bits: usize) {
        if num_bits == 0 {
            return;
        }
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

    /// Writes an unsigned 64-bit varint, automatically ensuring byte alignment first.
    pub fn put_varint_u64(&mut self, val: u64) {
        self.flush_byte_align();
        put_varint_u64(&mut self.buffer, val);
    }

    /// Writes an unsigned 32-bit varint, automatically ensuring byte alignment first.
    pub fn put_varint_u32(&mut self, val: u32) {
        self.flush_byte_align();
        put_varint_u32(&mut self.buffer, val);
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
        if num_bits == 0 {
            return Ok(0);
        }
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

    /// Reads an unsigned 64-bit integer using LEB128 varint encoding, automatically aligning to byte boundary first.
    pub fn get_varint_u64(&mut self) -> Result<u64, ClientCodecError> {
        self.align_byte();
        let mut slice = &self.data[self.byte_pos..];
        let original_len = slice.len();
        let val = get_varint_u64(&mut slice)?;
        let consumed = original_len - slice.len();
        self.byte_pos += consumed;
        Ok(val)
    }

    /// Reads an unsigned 32-bit integer using LEB128 varint encoding, automatically aligning to byte boundary first.
    pub fn get_varint_u32(&mut self) -> Result<u32, ClientCodecError> {
        self.align_byte();
        let mut slice = &self.data[self.byte_pos..];
        let original_len = slice.len();
        let val = get_varint_u32(&mut slice)?;
        let consumed = original_len - slice.len();
        self.byte_pos += consumed;
        Ok(val)
    }
}

#[macro_export]
#[doc(hidden)]
macro_rules! encode_bitpack_field {
    ($writer:expr, $val:expr, bool) => {
        $writer.write_bool(*$val);
    };
    ($writer:expr, $val:expr, bits($n:expr)) => {
        $writer.write_bits(*$val as u64, $n);
    };
    ($writer:expr, $val:expr, varint) => {
        $writer.put_varint_u32(*$val);
    };
    ($writer:expr, $val:expr, varint64) => {
        $writer.put_varint_u64(*$val);
    };
    ($writer:expr, $val:expr, delta_pos) => {
        $writer.write_bits(
            $crate::quantize::quantize_delta_i16(*$val) as u16 as u64,
            16,
        );
    };
    ($writer:expr, $val:expr, yaw_u8) => {
        $writer.write_bits($crate::quantize::quantize_yaw_u8(*$val) as u64, 8);
    };
    ($writer:expr, $val:expr, pitch_i8) => {
        $writer.write_bits($crate::quantize::quantize_pitch_i8(*$val) as u8 as u64, 8);
    };
    ($writer:expr, $val:expr, vel_i16) => {
        $writer.write_bits(
            $crate::quantize::quantize_velocity_i16(*$val) as u16 as u64,
            16,
        );
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! decode_bitpack_field {
    ($reader:expr, $type:ty, bool) => {
        $reader.read_bool()?
    };
    ($reader:expr, $type:ty, bits($n:expr)) => {
        $reader.read_bits($n)? as $type
    };
    ($reader:expr, $type:ty, varint) => {
        $reader.get_varint_u32()?
    };
    ($reader:expr, $type:ty, varint64) => {
        $reader.get_varint_u64()?
    };
    ($reader:expr, $type:ty, delta_pos) => {
        $crate::quantize::dequantize_delta_i16($reader.read_bits(16)? as u16 as i16)
    };
    ($reader:expr, $type:ty, yaw_u8) => {
        $crate::quantize::dequantize_yaw_u8($reader.read_bits(8)? as u8)
    };
    ($reader:expr, $type:ty, pitch_i8) => {
        $crate::quantize::dequantize_pitch_i8($reader.read_bits(8)? as u8 as i8)
    };
    ($reader:expr, $type:ty, vel_i16) => {
        $crate::quantize::dequantize_velocity_i16($reader.read_bits(16)? as u16 as i16)
    };
}

/// Macro for defining compact bit-packed delta structures.
///
/// Automatically generates:
/// - Associated field mask constants (`MASK_<FIELD> = 1 << bit_index`)
/// - A struct where optional fields are wrapped in `Option<T>`
/// - `compute_field_mask(&self) -> u16`
/// - `encode(&self, writer: &mut BitWriter)`
/// - `decode(reader: &mut BitReader) -> Result<Self, ClientCodecError>`
#[macro_export]
macro_rules! bitpack_delta {
    (
        $(#[$struct_meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                $field:ident $( ( $mask_const:ident ) )? : $type:ty => $directive:ident $( ( $opt:expr ) )? = $mask_bit:expr
            ),+ $(,)?
        }
    ) => {
        $(#[$struct_meta])*
        $vis struct $name {
            $(
                $(#[$field_meta])*
                pub $field: Option<$type>,
            )+
        }

        impl $name {
            $(
                $(
                    #[doc = concat!("Presence bitmask constant for `", stringify!($field), "`")]
                    pub const $mask_const: u16 = 1u16 << ($mask_bit);
                )?
            )+

            /// Computes the bitmask of all currently populated fields.
            #[inline]
            pub fn compute_field_mask(&self) -> u16 {
                let mut mask = 0u16;
                $(
                    if self.$field.is_some() {
                        mask |= 1u16 << ($mask_bit);
                    }
                )+
                mask
            }

            /// Encodes this delta into a [`$crate::bitpack::BitWriter`].
            pub fn encode(&self, writer: &mut $crate::bitpack::BitWriter) {
                let mask = self.compute_field_mask();
                writer.write_bits(mask as u64, 16);

                $(
                    if let Some(ref val) = self.$field {
                        $crate::encode_bitpack_field!(writer, val, $directive $( ( $opt ) )?);
                    }
                )+
            }

            /// Decodes this delta from a [`$crate::bitpack::BitReader`].
            pub fn decode(reader: &mut $crate::bitpack::BitReader<'_>) -> Result<Self, $crate::client::ClientCodecError> {
                let mask = reader.read_bits(16)? as u16;

                Ok(Self {
                    $(
                        $field: if mask & (1u16 << ($mask_bit)) != 0 {
                            Some($crate::decode_bitpack_field!(reader, $type, $directive $( ( $opt ) )?))
                        } else {
                            None
                        },
                    )+
                })
            }
        }
    };
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
        // 1. Buffer ending after 1st byte with MSB set (requires 2nd byte) -> expected: 2, got: 1
        let mut buf = BytesMut::from(&[0x80u8][..]);
        assert_eq!(
            get_varint_u64(&mut buf),
            Err(ClientCodecError::BufferTooShort {
                expected: 2,
                got: 1,
            })
        );

        // 2. Empty buffer -> expected: 1, got: 0
        let mut empty = BytesMut::new();
        assert_eq!(
            get_varint_u64(&mut empty),
            Err(ClientCodecError::BufferTooShort {
                expected: 1,
                got: 0,
            })
        );

        // 3. get_varint_u32 with buffer too short
        let mut buf_u32 = BytesMut::from(&[0x80u8][..]);
        assert_eq!(
            get_varint_u32(&mut buf_u32),
            Err(ClientCodecError::BufferTooShort {
                expected: 2,
                got: 1,
            })
        );

        let mut empty_u32 = BytesMut::new();
        assert_eq!(
            get_varint_u32(&mut empty_u32),
            Err(ClientCodecError::BufferTooShort {
                expected: 1,
                got: 0,
            })
        );
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

    #[test]
    fn test_bitwriter_capacity() {
        let writer = BitWriter::with_capacity(64);
        assert!(writer.capacity() >= 64);
    }

    #[test]
    fn test_bitwriter_write_bits_edge_cases() {
        // num_bits == 0: nothing written, byte_len remains 0
        let mut writer0 = BitWriter::with_capacity(8);
        writer0.write_bits(0xFFFF_FFFF_FFFF_FFFF, 0);
        assert_eq!(writer0.byte_len(), 0);
        assert_eq!(writer0.finish().len(), 0);

        // num_bits == 64: all 64 bits written
        let mut writer64 = BitWriter::with_capacity(8);
        let val64 = 0x0123_4567_89AB_CDEF_u64;
        writer64.write_bits(val64, 64);
        assert_eq!(writer64.byte_len(), 8);
        let bytes64 = writer64.finish();
        assert_eq!(bytes64.len(), 8);
        let mut reader64 = BitReader::new(&bytes64);
        assert_eq!(reader64.read_bits(64).unwrap(), val64);

        // num_bits > 64 (e.g. 70): clamped to 64 bits written
        let mut writer70 = BitWriter::with_capacity(16);
        writer70.write_bits(val64, 70);
        assert_eq!(writer70.byte_len(), 8);
        let bytes70 = writer70.finish();
        assert_eq!(bytes70.len(), 8);
        let mut reader70 = BitReader::new(&bytes70);
        assert_eq!(reader70.read_bits(64).unwrap(), val64);
        assert_eq!(reader70.remaining_bits(), 0);
    }

    #[test]
    fn test_bitwriter_flush_byte_align_zero_offset() {
        // Calling flush_byte_align when bit_offset == 0 must NOT append an extra zero byte
        let mut writer = BitWriter::with_capacity(8);
        writer.flush_byte_align();
        assert_eq!(writer.byte_len(), 0);
        assert_eq!(writer.finish().len(), 0);

        // Calling flush_byte_align when aligned after writing a full byte (8 bits)
        let mut writer2 = BitWriter::with_capacity(8);
        writer2.write_bits(0xAA, 8);
        assert_eq!(writer2.byte_len(), 1);
        writer2.flush_byte_align();
        assert_eq!(writer2.byte_len(), 1);
        let bytes2 = writer2.finish();
        assert_eq!(bytes2.len(), 1);
        assert_eq!(bytes2[0], 0xAA);
    }

    #[test]
    fn test_bitwriter_byte_len_boundaries() {
        let mut writer = BitWriter::with_capacity(8);
        assert_eq!(writer.byte_len(), 0); // 0 bits -> 0 bytes

        writer.write_bits(1, 1);
        assert_eq!(writer.byte_len(), 1); // 1 bit -> 1 byte

        writer.write_bits(0, 6);
        assert_eq!(writer.byte_len(), 1); // 7 bits total -> 1 byte

        writer.write_bits(1, 1);
        assert_eq!(writer.byte_len(), 1); // 8 bits total -> 1 byte

        writer.write_bits(1, 1);
        assert_eq!(writer.byte_len(), 2); // 9 bits total -> 2 bytes

        writer.write_bits(0, 7);
        assert_eq!(writer.byte_len(), 2); // 16 bits total -> 2 bytes

        writer.write_bits(1, 1);
        assert_eq!(writer.byte_len(), 3); // 17 bits total -> 3 bytes
    }

    #[test]
    fn test_bitreader_read_bits_boundary_and_limits() {
        let bytes = [0x55; 9];
        let mut reader = BitReader::new(&bytes);

        // num_bits == 0 succeeds immediately returning 0
        assert_eq!(reader.read_bits(0), Ok(0));

        // num_bits == 64 succeeds (kills > replaced with >= or ==)
        assert!(reader.read_bits(64).is_ok());

        // num_bits == 65 fails with InvalidData
        let mut reader2 = BitReader::new(&bytes);
        assert!(matches!(
            reader2.read_bits(65),
            Err(ClientCodecError::InvalidData(_))
        ));

        // Exact remaining bits (remaining_bits() == num_bits) succeeds (kills < replaced with <=)
        let bytes1 = [0xAA];
        let mut reader3 = BitReader::new(&bytes1);
        assert_eq!(reader3.remaining_bits(), 8);
        assert_eq!(reader3.read_bits(8), Ok(0xAA));
        assert_eq!(reader3.remaining_bits(), 0);
    }

    #[test]
    fn test_bitreader_align_byte_when_already_aligned() {
        let bytes = [0x11, 0x22, 0x33];
        let mut reader = BitReader::new(&bytes);

        // Initially aligned at byte 0, bit_offset 0
        reader.align_byte();
        assert_eq!(reader.byte_pos, 0);
        assert_eq!(reader.remaining_bits(), 24);

        // Read 8 bits -> byte_pos becomes 1, bit_offset is 0
        assert_eq!(reader.read_bits(8).unwrap(), 0x11);
        assert_eq!(reader.byte_pos, 1);
        assert_eq!(reader.bit_offset, 0);

        // Calling align_byte() when bit_offset == 0 must NOT advance byte_pos (kills > replaced with >=)
        reader.align_byte();
        assert_eq!(reader.byte_pos, 1);
        assert_eq!(reader.remaining_bits(), 16);

        // Read next byte to verify stream position and data integrity
        assert_eq!(reader.read_bits(8).unwrap(), 0x22);
    }

    #[test]
    fn test_bitreader_read_bytes_boundaries() {
        let bytes = [1, 2, 3, 4, 5];
        let mut reader = BitReader::new(&bytes);

        // len < remaining
        let slice1 = reader.read_bytes(2).unwrap();
        assert_eq!(slice1, &[1, 2]);

        // len == remaining (kills < replaced with <=)
        let slice2 = reader.read_bytes(3).unwrap();
        assert_eq!(slice2, &[3, 4, 5]);

        // len > remaining (expected: 1, got: 0)
        assert_eq!(
            reader.read_bytes(1),
            Err(ClientCodecError::BufferTooShort {
                expected: 1,
                got: 0,
            })
        );

        // Fresh reader: len > remaining (expected: 6, got: 5)
        let mut reader2 = BitReader::new(&bytes);
        assert_eq!(
            reader2.read_bytes(6),
            Err(ClientCodecError::BufferTooShort {
                expected: 6,
                got: 5,
            })
        );
    }

    #[test]
    fn test_bitwriter_and_reader_varints_roundtrip() {
        let mut writer = BitWriter::with_capacity(32);
        writer.write_bits(0b101, 3);
        writer.put_varint_u32(123456);
        writer.put_varint_u64(9876543210123);
        writer.write_bool(true);

        let bytes = writer.finish();
        let mut reader = BitReader::new(&bytes);

        assert_eq!(reader.read_bits(3).unwrap(), 0b101);
        assert_eq!(reader.get_varint_u32().unwrap(), 123456);
        assert_eq!(reader.get_varint_u64().unwrap(), 9876543210123);
        assert!(reader.read_bool().unwrap());
    }

    bitpack_delta! {
        #[derive(Debug, Clone, PartialEq)]
        pub struct TestKinematicDelta {
            dx (MASK_DX): f32 => delta_pos = 0,
            dy (MASK_DY): f32 => delta_pos = 1,
            dz (MASK_DZ): f32 => delta_pos = 2,
            yaw (MASK_YAW): f32 => yaw_u8 = 3,
            pitch (MASK_PITCH): f32 => pitch_i8 = 4,
            vel_x (MASK_VEL_X): f32 => vel_i16 = 5,
            crouching (MASK_CROUCHING): bool => bool = 6,
            action_id (MASK_ACTION): u32 => varint = 7,
        }
    }

    #[test]
    fn test_bitpack_delta_macro_roundtrip_full() {
        assert_eq!(TestKinematicDelta::MASK_DX, 1 << 0);
        assert_eq!(TestKinematicDelta::MASK_DY, 1 << 1);
        assert_eq!(TestKinematicDelta::MASK_DZ, 1 << 2);
        assert_eq!(TestKinematicDelta::MASK_YAW, 1 << 3);
        assert_eq!(TestKinematicDelta::MASK_PITCH, 1 << 4);
        assert_eq!(TestKinematicDelta::MASK_VEL_X, 1 << 5);
        assert_eq!(TestKinematicDelta::MASK_CROUCHING, 1 << 6);
        assert_eq!(TestKinematicDelta::MASK_ACTION, 1 << 7);

        let delta = TestKinematicDelta {
            dx: Some(1.25),
            dy: Some(-0.5),
            dz: Some(10.0),
            yaw: Some(1.57),
            pitch: Some(-0.25),
            vel_x: Some(3.5),
            crouching: Some(true),
            action_id: Some(42),
        };

        let mut writer = BitWriter::with_capacity(32);
        delta.encode(&mut writer);
        let bytes = writer.finish();

        let mut reader = BitReader::new(&bytes);
        let decoded = TestKinematicDelta::decode(&mut reader).expect("decode failed");

        assert!((decoded.dx.unwrap() - 1.25).abs() < 0.01);
        assert!((decoded.dy.unwrap() - (-0.5)).abs() < 0.01);
        assert!((decoded.dz.unwrap() - 10.0).abs() < 0.01);
        assert!((decoded.yaw.unwrap() - 1.57).abs() < 0.03);
        assert!((decoded.pitch.unwrap() - (-0.25)).abs() < 0.03);
        assert!((decoded.vel_x.unwrap() - 3.5).abs() < 0.02);
        assert_eq!(decoded.crouching, Some(true));
        assert_eq!(decoded.action_id, Some(42));
    }

    #[test]
    fn test_bitpack_delta_macro_roundtrip_sparse() {
        let sparse = TestKinematicDelta {
            dx: Some(0.125),
            dy: None,
            dz: None,
            yaw: Some(std::f32::consts::PI),
            pitch: None,
            vel_x: None,
            crouching: None,
            action_id: None,
        };

        let mut writer = BitWriter::with_capacity(16);
        sparse.encode(&mut writer);
        let bytes = writer.finish();

        // 16 bits mask (2 bytes) + 16 bits dx (2 bytes) + 8 bits yaw (1 byte) = 5 bytes total!
        assert_eq!(bytes.len(), 5);

        let mut reader = BitReader::new(&bytes);
        let decoded = TestKinematicDelta::decode(&mut reader).expect("sparse decode failed");

        assert!((decoded.dx.unwrap() - 0.125).abs() < 0.01);
        assert!(decoded.dy.is_none());
        assert!(decoded.dz.is_none());
        assert!((decoded.yaw.unwrap() - std::f32::consts::PI).abs() < 0.03);
        assert!(decoded.pitch.is_none());
        assert!(decoded.vel_x.is_none());
        assert!(decoded.crouching.is_none());
        assert!(decoded.action_id.is_none());
    }
}
