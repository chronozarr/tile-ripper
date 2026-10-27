//! Native sample types shared by acquisition adapters and synchronous consumers.
use crate::{fail, Result};
#[derive(Debug)]
pub enum Pixels {
    UInt8(Vec<u8>),
    Int8(Vec<i8>),
    UInt16(Vec<u16>),
    Int16(Vec<i16>),
    UInt32(Vec<u32>),
    Float32(Vec<f32>),
    Float64(Vec<f64>),
}

impl Default for Pixels {
    fn default() -> Self {
        Self::UInt16(Vec::new())
    }
}
impl From<Vec<u16>> for Pixels {
    fn from(v: Vec<u16>) -> Self {
        Self::UInt16(v)
    }
}
impl Pixels {
    pub fn len(&self) -> usize {
        match self {
            Self::UInt8(v) => v.len(),
            Self::Int8(v) => v.len(),
            Self::UInt16(v) => v.len(),
            Self::Int16(v) => v.len(),
            Self::UInt32(v) => v.len(),
            Self::Float32(v) => v.len(),
            Self::Float64(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn byte_len(&self) -> usize {
        self.len()
            * match self {
                Self::UInt8(_) | Self::Int8(_) => 1,
                Self::UInt16(_) => 2,
                Self::Int16(_) => 2,
                Self::UInt32(_) => 4,
                Self::Float32(_) => 4,
                Self::Float64(_) => 8,
            }
    }
    pub fn as_u8(&self) -> Option<&[u8]> {
        if let Self::UInt8(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_i8(&self) -> Option<&[i8]> {
        if let Self::Int8(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_u32(&self) -> Option<&[u32]> {
        if let Self::UInt32(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_u16(&self) -> Option<&[u16]> {
        if let Self::UInt16(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_i16(&self) -> Option<&[i16]> {
        if let Self::Int16(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_f32(&self) -> Option<&[f32]> {
        if let Self::Float32(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_f64(&self) -> Option<&[f64]> {
        if let Self::Float64(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn into_u16(self) -> Result<Vec<u16>> {
        if let Self::UInt16(v) = self {
            Ok(v)
        } else {
            fail("UInt16 output adapter requires UInt16 stored values")
        }
    }
    pub(crate) fn value(&self, i: usize) -> f64 {
        match self {
            Self::UInt8(v) => v[i] as f64,
            Self::Int8(v) => v[i] as f64,
            Self::UInt16(v) => v[i] as f64,
            Self::Int16(v) => v[i] as f64,
            Self::UInt32(v) => v[i] as f64,
            Self::Float32(v) => v[i] as f64,
            Self::Float64(v) => v[i],
        }
    }
    pub(crate) fn valid(&self, i: usize, nodata: Option<f64>) -> bool {
        let v = self.value(i);
        !v.is_nan() && nodata.is_none_or(|n| v != n)
    }
    pub(crate) fn copy(&mut self, dst: usize, other: &Self, src: usize, len: usize) -> Result<()> {
        match (self, other) {
            (Self::UInt8(a), Self::UInt8(b)) => {
                a[dst..dst + len].copy_from_slice(&b[src..src + len])
            }
            (Self::Int8(a), Self::Int8(b)) => a[dst..dst + len].copy_from_slice(&b[src..src + len]),
            (Self::UInt32(a), Self::UInt32(b)) => {
                a[dst..dst + len].copy_from_slice(&b[src..src + len])
            }
            (Self::UInt16(a), Self::UInt16(b)) => {
                a[dst..dst + len].copy_from_slice(&b[src..src + len])
            }
            (Self::Int16(a), Self::Int16(b)) => {
                a[dst..dst + len].copy_from_slice(&b[src..src + len])
            }
            (Self::Float32(a), Self::Float32(b)) => {
                a[dst..dst + len].copy_from_slice(&b[src..src + len])
            }
            (Self::Float64(a), Self::Float64(b)) => {
                a[dst..dst + len].copy_from_slice(&b[src..src + len])
            }
            _ => return fail("native sample type mismatch"),
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Pixels;

    #[test]
    fn float64_values_and_validity_keep_native_precision() {
        let precise = 1.0 + f64::EPSILON;
        let nan = f64::from_bits(0x7ff8_0000_0000_0042);
        let source = Pixels::Float64(vec![precise, nan, -9999., -0.]);
        assert_eq!(source.byte_len(), 32);
        assert_eq!(source.as_f64().unwrap()[0].to_bits(), precise.to_bits());
        assert!(source.as_f32().is_none());
        assert!(source.valid(0, Some(-9999.)));
        assert!(!source.valid(1, None));
        assert!(!source.valid(2, Some(-9999.)));
        assert!(source.valid(3, None));
        let mut copied = Pixels::Float64(vec![0.; 4]);
        copied.copy(0, &source, 0, 4).unwrap();
        assert_eq!(
            copied
                .as_f64()
                .unwrap()
                .iter()
                .map(|v| v.to_bits())
                .collect::<Vec<_>>(),
            source
                .as_f64()
                .unwrap()
                .iter()
                .map(|v| v.to_bits())
                .collect::<Vec<_>>()
        );
        assert!(copied.copy(0, &Pixels::Float32(vec![1.]), 0, 1).is_err());
    }
}
