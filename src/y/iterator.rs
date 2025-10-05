use byteorder::BigEndian;
use byteorder::{ReadBytesExt, WriteBytesExt};
use log::info;

use serde::{Deserialize, Serialize};
use std::io::{Cursor, Write};

/// `ValueStruct` represents the value info that can be associated with a key, but also the internal
/// Meta field.
/// |`meta`|`user_meta`|`value_size`|`value`|
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[repr(C)]
pub struct ValueStruct {
    pub(crate) meta: u8,
    pub(crate) user_meta: u8,
    pub(crate) value: Vec<u8>,
    /// This field is not serialized. Only for internal usage.
    pub(crate) version: u64,
}

impl ValueStruct {
    pub(crate) fn new(value: Vec<u8>, meta: u8, user_meta: u8) -> ValueStruct {
        ValueStruct {
            meta,
            user_meta,
            value,
            version: 0,
        }
    }
    
    pub(crate) fn new_with_version(value: Vec<u8>, meta: u8, user_meta: u8, version: u64) -> ValueStruct {
        ValueStruct {
            meta,
            user_meta,
            value,
            version,
        }
    }
    pub(crate) const fn header_size() -> usize {
        2
    }

    pub(crate) fn size(&self) -> usize {
        Self::header_size() + self.value.len()
    }

    pub(crate) fn write_data(&self, buffer: &mut [u8]) -> Result<(), std::io::Error> {
        use std::io::Write;
        let mut cursor = Cursor::new(buffer);
        cursor.write_u8(self.meta)?;
        cursor.write_u8(self.user_meta)?;
        cursor.write_all(&self.value)?;
        Ok(())
    }

    pub(crate) fn read_data(&mut self, buffer: &[u8]) -> Result<(), std::io::Error> {
        let mut cursor = Cursor::new(buffer);
        self.meta = cursor.read_u8()?;
        self.user_meta = cursor.read_u8()?;
        if let Some(slice) = buffer.get(Self::header_size()..) {
            self.value.extend_from_slice(slice);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn pretty(&self) -> String {
        use crate::hex_str;
        format!(
            "meta: {}, user_meta: {}, value: {}",
            self.meta,
            self.user_meta,
            hex_str(&self.value)
        )
    }
}

impl<T> From<T> for ValueStruct
where
    T: AsRef<[u8]>,
{
    fn from(buffer: T) -> Self {
        let mut v = ValueStruct::default();
        let _ = v.read_data(buffer.as_ref());
        v
    }
}

impl From<&ValueStruct> for Vec<u8> {
    fn from(val: &ValueStruct) -> Self {
        let mut buffer = vec![0; val.size()];
        let _ = val.write_data(&mut buffer);
        buffer
    }
}

/// A iterator trait
pub trait Xiterator {
    /// The iterator element
    type Output;
    /// Same to std iterator next
    fn next(&self) -> Option<Self::Output>;
    /// Same to std iterator rev (But not implement by now!)
    // fn rev(&self) -> Option<Self::Output> {
    //     todo!()
    // }
    /// Seeks to first element (or last element for reverse iterator).
    fn rewind(&self) -> Option<Self::Output>;
    /// Seek with key, return a element that it's key >= key or <= key.
    fn seek(&self, key: &[u8]) -> Option<Self::Output>;
    /// Peek current element
    fn peek(&self) -> Option<Self::Output> {
        None
    }
    /// The iterator indetify
    fn id(&self) -> String {
        "unknown_id".to_owned()
    }

    /// Close the iterator
    fn close(&self) {
        info!("close the iterator: {}", self.id());
    }
}

pub trait KeyValue<V> {
    fn key(&self) -> &[u8];
    fn value(&self) -> V;
}

// impl<T> Iterator for dyn Xiterator<Output=T> {
//     type Item = T;
//
//     fn next(&mut self) -> Option<Self::Item> {
//         todo!()
//     }
// }
