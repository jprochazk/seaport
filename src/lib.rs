pub mod client;
pub mod error;
pub mod peer;
pub mod server;

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

pub mod prelude {
  pub use crate::client;
  pub use client::connect;
  pub use client::connect_with;

  pub use crate::server;
  pub use server::listen;
  pub use server::listen_with;

  pub use crate::peer::Handler;
}
