// @generated
#![allow(clippy::all)]

use prost::Message;
#[allow(unused_imports)]
use crate::manifest::ManifestChangeBuilder;
use crate::Result;

// Re-export the modules for easier access
pub mod badgerpb3 {
    include!(concat!(env!("OUT_DIR"), "/badgerpb3.rs"));
}

pub mod protos {
    include!(concat!(env!("OUT_DIR"), "/protos.rs"));
}

// Re-export the main types for convenience
#[allow(unused_imports)]
pub use badgerpb3::{ManifestChange, ManifestChangeSet};
#[allow(unused_imports)]
pub use protos::KvPair;

pub(crate) fn convert_manifest_set_to_vec(mf_set: &ManifestChangeSet) -> Vec<u8> {
    let mut buffer = Vec::new();
    let _ = mf_set.encode(&mut buffer);
    buffer
}

pub(crate) fn parse_manifest_set_from_vec(buffer: &[u8]) -> Result<ManifestChangeSet> {
    let set: ManifestChangeSet = ManifestChangeSet::decode(buffer).map_err(|err| crate::Error::from(format!("{}", err)))?;
    Ok(set)
}

#[test]
fn enc_dec() {
    let mut mf = ManifestChangeSet::default();
    mf.changes
        .push(ManifestChangeBuilder::new(1).build());
    let buffer = convert_manifest_set_to_vec(&mf);
    let got = parse_manifest_set_from_vec(&buffer).unwrap();
    assert_eq!(got, mf);
}
