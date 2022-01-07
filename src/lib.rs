pub mod client;
pub mod error;
pub mod peer;
pub mod server;

// TODO: prelude

mod packet;
mod queue;
mod seq;
mod socket;

use std::{
  collections::hash_map::DefaultHasher,
  hash::{Hash, Hasher},
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Protocol(u64);

impl<T: Hash> From<T> for Protocol {
  fn from(v: T) -> Self {
    let mut s = DefaultHasher::new();
    v.hash(&mut s);
    Self(s.finish())
  }
}
