//! Native sample types shared by acquisition adapters and synchronous consumers.
use crate::{fail, Result};
#[derive(Debug)]
pub enum Pixels {
    UInt16(Vec<u16>),
    Float32(Vec<f32>),
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
            Self::UInt16(v) => v.len(),
            Self::Float32(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn byte_len(&self) -> usize {
        self.len()
            * match self {
                Self::UInt16(_) => 2,
                Self::Float32(_) => 4,
            }
    }
    pub fn as_u16(&self) -> Option<&[u16]> {
        if let Self::UInt16(v) = self {
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
    pub fn into_u16(self) -> Result<Vec<u16>> {
        if let Self::UInt16(v) = self {
            Ok(v)
        } else {
            fail("UInt16 output adapter cannot accept Float32")
        }
    }
    pub(crate) fn value(&self, i: usize) -> f64 {
        match self {
            Self::UInt16(v) => v[i] as f64,
            Self::Float32(v) => v[i] as f64,
        }
    }
    pub(crate) fn valid(&self, i: usize, nodata: Option<f64>) -> bool {
        let v = self.value(i);
        !v.is_nan() && nodata.is_none_or(|n| v != n)
    }
    pub(crate) fn copy(&mut self, dst: usize, other: &Self, src: usize, len: usize) -> Result<()> {
        match (self, other) {
            (Self::UInt16(a), Self::UInt16(b)) => {
                a[dst..dst + len].copy_from_slice(&b[src..src + len])
            }
            (Self::Float32(a), Self::Float32(b)) => {
                a[dst..dst + len].copy_from_slice(&b[src..src + len])
            }
            _ => return fail("native sample type mismatch"),
        };
        Ok(())
    }
}
