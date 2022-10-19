//! Support for the [`parity-scale-codec`](https://crates.io/crates/parity-scale-codec) crate.
#![cfg(feature = "parity-scale-codec")]
#![cfg_attr(has_doc_cfg, doc(cfg(feature = "parity-scale-codec")))]

use crate::Uint;
use parity_scale_codec::{Compact, CompactAs, Decode, Encode, Error, Input, MaxEncodedLen, Output};

// Compact encoding is supported only for 0-(2**536-1) values:
// https://docs.substrate.io/reference/scale-codec/#fn-1
pub(crate) const COMPACT_BITS_LIMIT: usize = 536;

impl<const BITS: usize, const LIMBS: usize> Encode for Uint<BITS, LIMBS> {
    /// u32 prefix for compact encoding + bytes needed for LE bytes representation
    fn size_hint(&self) -> usize {
        core::mem::size_of::<u32>() + Self::BYTES
    }

    fn using_encoded<R, F: FnOnce(&[u8]) -> R>(&self, f: F) -> R {
        self.as_le_bytes().using_encoded(f)
    }
}

impl<const BITS: usize, const LIMBS: usize> MaxEncodedLen for Uint<BITS, LIMBS> {
    fn max_encoded_len() -> usize {
        core::mem::size_of::<Self>()
    }
}

impl<const BITS: usize, const LIMBS: usize> Decode for Uint<BITS, LIMBS> {
    fn decode<I: Input>(input: &mut I) -> Result<Self, Error> {
        Decode::decode(input).and_then(|b: Vec<_>| {
            Self::try_from_le_slice(&b).ok_or(Error::from("value is larger than fits the Uint"))
        })
    }
}

// TODO: Use nightly generic const expressions to validate that BITS parameter is less than 536
pub struct CompactUint<const BITS: usize, const LIMBS: usize>(pub Uint<BITS, LIMBS>);

impl<const BITS: usize, const LIMBS: usize> From<Compact<Self>> for CompactUint<BITS, LIMBS> {
    fn from(v: Compact<Self>) -> Self {
        v.0
    }
}

impl<const BITS: usize, const LIMBS: usize> CompactAs for CompactUint<BITS, LIMBS> {
    type As = Uint<BITS, LIMBS>;

    fn encode_as(&self) -> &Self::As {
        &self.0
    }

    fn decode_from(v: Self::As) -> Result<Self, Error> {
        Ok(Self(v))
    }
}

pub struct CompactRefUint<'a, const BITS: usize, const LIMBS: usize>(pub &'a Uint<BITS, LIMBS>);

impl<'a, const BITS: usize, const LIMBS: usize> Encode for CompactRefUint<'a, BITS, LIMBS> {
    fn size_hint(&self) -> usize {
        match self.0.trailing_ones() {
            0..=6 => 1,
            0..=14 => 2,
            0..=30 => 4,
            _ => (32 - self.0.leading_zeros() / 8) as usize + 1,
        }
    }

    fn encode_to<T: Output + ?Sized>(&self, dest: &mut T) {
        assert_compact_supported::<BITS>();

        match self.0.bit_len() {
            // 0..=0b0011_1111
            0..=6 => dest.push_byte((self.0.to::<u8>()) << 2),
            // 0..=0b0011_1111_1111_1111
            0..=14 => ((self.0.to::<u16>() << 2) | 0b01).encode_to(dest),
            // 0..=0b0011_1111_1111_1111_1111_1111_1111_1111
            0..=30 => ((self.0.to::<u32>() << 2) | 0b10).encode_to(dest),
            _ => {
                let bytes_needed = self.0.byte_len();
                assert!(
                    bytes_needed >= 4,
                    "Previous match arm matches anything less than 2^30; qed"
                );
                dest.push_byte(0b11 + ((bytes_needed - 4) << 2) as u8);
                dest.write(&self.0.as_le_bytes_trimmed());
            }
        }
    }
}

/// Prefix another input with a byte.
struct PrefixInput<'a, T> {
    prefix: Option<u8>,
    input: &'a mut T,
}

impl<'a, T: 'a + Input> Input for PrefixInput<'a, T> {
    fn remaining_len(&mut self) -> Result<Option<usize>, Error> {
        let len = if let Some(len) = self.input.remaining_len()? {
            Some(len.saturating_add(self.prefix.iter().count()))
        } else {
            None
        };
        Ok(len)
    }

    fn read(&mut self, buffer: &mut [u8]) -> Result<(), Error> {
        match self.prefix.take() {
            Some(v) if !buffer.is_empty() => {
                buffer[0] = v;
                self.input.read(&mut buffer[1..])
            }
            _ => self.input.read(buffer),
        }
    }
}

const OUT_OF_RANGE: &str = "out of range Uint decoding";

impl<const BITS: usize, const LIMBS: usize> Decode for CompactUint<BITS, LIMBS> {
    fn decode<I: Input>(input: &mut I) -> Result<Self, Error> {
        assert_compact_supported::<BITS>();

        let prefix = input.read_byte()?;
        Ok(Self(match prefix % 4 {
            0 => {
                Uint::<BITS, LIMBS>::try_from(prefix >> 2).map_err(|_| Error::from(OUT_OF_RANGE))?
            } // right shift to remove mode bits
            1 => {
                let x = u16::decode(&mut PrefixInput {
                    prefix: Some(prefix),
                    input,
                })? >> 2; // right shift to remove mode bits
                if (0b0011_1111..=0b0011_1111_1111_1111).contains(&x) {
                    x.try_into().map_err(|_| Error::from(OUT_OF_RANGE))?
                } else {
                    return Err(Error::from(OUT_OF_RANGE));
                }
            }
            2 => {
                let x = u32::decode(&mut PrefixInput {
                    prefix: Some(prefix),
                    input,
                })? >> 2; // right shift to remove mode bits
                if (0b0011_1111_1111_1111..=u32::MAX >> 2).contains(&x) {
                    x.try_into().map_err(|_| Error::from(OUT_OF_RANGE))?
                } else {
                    return Err(OUT_OF_RANGE.into());
                }
            }
            _ => match (prefix >> 2) + 4 {
                4 => {
                    let x = u32::decode(input)?;
                    if x > u32::MAX >> 2 {
                        x.try_into().map_err(|_| Error::from(OUT_OF_RANGE))?
                    } else {
                        return Err(OUT_OF_RANGE.into());
                    }
                }
                8 => {
                    let x = u64::decode(input)?;
                    if x > u64::MAX >> 8 {
                        x.try_into().map_err(|_| Error::from(OUT_OF_RANGE))?
                    } else {
                        return Err(OUT_OF_RANGE.into());
                    }
                }
                16 => {
                    let x = u128::decode(input)?;
                    if x > u128::MAX >> 8 {
                        x.try_into().map_err(|_| Error::from(OUT_OF_RANGE))?
                    } else {
                        return Err(OUT_OF_RANGE.into());
                    }
                }
                bytes => {
                    let le_byte_slice = (0..bytes)
                        .map(|_| input.read_byte())
                        .rev()
                        .collect::<Result<Vec<_>, _>>()?;
                    let x = Uint::<BITS, LIMBS>::try_from_le_slice(&le_byte_slice)
                        .ok_or(Error::from("value is larger than fits the Uint"))?;
                    let bits = bytes as usize * 8;
                    let limbs = (bits + 64 - 1) / 64;

                    let mut new_limbs = vec![u64::MAX; limbs];
                    if bits > 0 {
                        new_limbs[limbs - 1] &= if bits % 64 == 0 {
                            u64::MAX
                        } else {
                            (1 << bits % 64) - 1
                        }
                    }
                    if Uint::<COMPACT_BITS_LIMIT, 9>::from(x)
                        > Uint::from_limbs_slice(&new_limbs) >> ((68 - bytes as usize + 1) * 8)
                    {
                        x
                    } else {
                        return Err(OUT_OF_RANGE.into());
                    }
                }
            },
        }))
    }
}

fn assert_compact_supported<const BITS: usize>() {
    assert!(
        BITS < COMPACT_BITS_LIMIT,
        "compact encoding is supported only for 0-(2**536-1) values"
    );
}

#[cfg(test)]
mod tests {
    use crate::support::scale::{CompactRefUint, CompactUint, COMPACT_BITS_LIMIT};
    use crate::{const_for, nlimbs, Uint};
    use parity_scale_codec::{Decode, Encode};
    use proptest::proptest;

    #[test]
    fn test_scale() {
        const_for!(BITS in SIZES {
            const LIMBS: usize = nlimbs(BITS);
            proptest!(|(value: Uint<BITS, LIMBS>)| {
                let serialized = Encode::encode(&value);
                let deserialized = <Uint::<BITS, LIMBS> as Decode>::decode(&mut serialized.as_slice()).unwrap();
                assert_eq!(value, deserialized);
            });
        });
    }

    #[test]
    fn test_scale_compact() {
        const_for!(BITS in SIZES {
            const LIMBS: usize = nlimbs(BITS);
            proptest!(|(value: Uint<BITS, LIMBS>)| {
                if BITS < COMPACT_BITS_LIMIT {
                    let serialized_compact = CompactRefUint(&value).encode();
                    let deserialized_compact = CompactUint::decode(&mut serialized_compact.as_slice()).unwrap();
                    assert_eq!(value, deserialized_compact.0);

                    if BITS < 30 && value != Uint::ZERO {
                        let serialized_normal = value.encode();
                        assert!(serialized_compact.len() < serialized_normal.len());
                    }
                }
            });
        });
    }
}
