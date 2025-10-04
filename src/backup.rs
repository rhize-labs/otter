use std::io::Write;
use byteorder::{LittleEndian, WriteBytesExt};
use prost::Message;
use crate::pb::protos::KvPair;

pub fn write_to<W>(entry: &KvPair, wt: &mut W) -> crate::Result<()> where W: Write {
    let mut buf = Vec::new();
    let _ = entry.encode(&mut buf);
    wt.write_u64::<LittleEndian>(buf.len() as u64)?;
    wt.write_all(&buf)?;
    Ok(())
}
