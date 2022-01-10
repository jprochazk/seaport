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

#[derive(Clone, Copy, PartialEq)]
pub struct Protocol(pub u64);

impl<T: Hash> From<T> for Protocol {
  fn from(v: T) -> Self {
    let mut s = DefaultHasher::new();
    v.hash(&mut s);
    Self(s.finish())
  }
}
