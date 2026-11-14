//! Bitwise manipulation helpers for instruction decoding and systems arithmetic.

/// Extracts a contiguous range of bits `[msb:lsb]` inclusive from a 32-bit unsigned integer.
#[inline(always)]
pub const fn extract_bits_u32(val: u32, lsb: u32, msb: u32) -> u32 {
    debug_assert!(msb >= lsb && msb < 32, "Invalid bit range");
    let mask = (1u64 << (msb - lsb + 1)) - 1;
    ((val >> lsb) as u64 & mask) as u32
}

/// Extracts a contiguous range of bits `[msb:lsb]` inclusive from a 64-bit unsigned integer.
#[inline(always)]
pub const fn extract_bits_u64(val: u64, lsb: u32, msb: u32) -> u64 {
    debug_assert!(msb >= lsb && msb < 64, "Invalid bit range");
    let bit_count = msb - lsb + 1;
    let mask = if bit_count == 64 {
        u64::MAX
    } else {
        (1u64 << bit_count) - 1
    };
    (val >> lsb) & mask
}

/// Tests whether the bit at `bit_idx` is set.
#[inline(always)]
pub const fn test_bit_u32(val: u32, bit_idx: u32) -> bool {
    (val & (1u32 << bit_idx)) != 0
}

/// Tests whether the bit at `bit_idx` is set.
#[inline(always)]
pub const fn test_bit_u64(val: u64, bit_idx: u32) -> bool {
    (val & (1u64 << bit_idx)) != 0
}

/// Sign-extends an arbitrary `bit_count`-wide value in `val` to a signed 64-bit integer.
#[inline(always)]
pub const fn sign_extend_64(val: u64, bit_count: u32) -> i64 {
    debug_assert!(bit_count > 0 && bit_count <= 64, "Invalid bit_count");
    let shift = 64 - bit_count;
    ((val as i64) << shift) >> shift
}

/// Sign-extends an arbitrary `bit_count`-wide value in `val` to a signed 32-bit integer.
#[inline(always)]
pub const fn sign_extend_32(val: u32, bit_count: u32) -> i32 {
    debug_assert!(bit_count > 0 && bit_count <= 32, "Invalid bit_count");
    let shift = 32 - bit_count;
    ((val as i32) << shift) >> shift
}

/// Parallel bit deposit (PDEP): deposits contiguous low-order bits from `value`
/// into positions specified by the set bits of `mask`.
#[inline(always)]
pub const fn pdep_u32(value: u32, mask: u32) -> u32 {
    let mut res = 0u32;
    let mut m = mask;
    let mut bit = 1u32;
    while m != 0 {
        let lowest = m & (!m).wrapping_add(1);
        if value & bit != 0 {
            res |= lowest;
        }
        m &= m.wrapping_sub(1);
        bit = bit.wrapping_add(bit);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_bits() {
        let val: u32 = 0b1011_0100;
        assert_eq!(extract_bits_u32(val, 2, 5), 0b1101);
        assert_eq!(extract_bits_u32(val, 0, 0), 0);
        assert_eq!(extract_bits_u32(val, 7, 7), 1);
        assert_eq!(extract_bits_u32(0xFFFF_FFFF, 0, 31), 0xFFFF_FFFF);
    }

    #[test]
    fn test_test_bit() {
        let val: u32 = 0x8000_0001;
        assert!(test_bit_u32(val, 0));
        assert!(!test_bit_u32(val, 1));
        assert!(test_bit_u32(val, 31));
    }

    #[test]
    fn test_sign_extend() {
        // 9-bit negative value: 0x1F0 (-16)
        let raw_9bit = 0x1F0u64;
        assert_eq!(sign_extend_64(raw_9bit, 9), -16);

        // 12-bit positive value: 0x07F (+127)
        let raw_12bit = 0x07Fu64;
        assert_eq!(sign_extend_64(raw_12bit, 12), 127);

        // 16-bit negative: 0xFFFF (-1)
        assert_eq!(sign_extend_64(0xFFFF, 16), -1);
    }

    #[test]
    fn test_pdep_u32() {
        assert_eq!(pdep_u32(0b1011, 0b10101010), 0b10001010);
        assert_eq!(pdep_u32(0b11, 0b0101), 0b0101);
        assert_eq!(pdep_u32(0, 0xFFFF), 0);
        assert_eq!(pdep_u32(0xFFFF, 0), 0);
    }
}
