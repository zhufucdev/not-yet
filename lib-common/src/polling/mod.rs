use std::fmt::Debug;
use std::hash::Hash;

pub use schedule::{Schedule, Scheduler};
use serde::{Serialize, de::DeserializeOwned};

pub mod error;
pub mod schedule;
pub mod task;
#[cfg(test)]
mod test;
pub mod trigger;

pub trait KeyContract:
    Debug + Hash + Eq + PartialEq + Clone + Serialize + DeserializeOwned
{
}

impl<T> KeyContract for T where
    T: Debug + Hash + Eq + PartialEq + Clone + Serialize + DeserializeOwned
{
}
