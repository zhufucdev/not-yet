pub use schedule::{Scheduler, Schedule};
use serde::{Serialize, de::DeserializeOwned};

pub mod error;
pub mod schedule;
pub mod task;
pub mod trigger;

pub trait DataContract: Clone + Serialize + DeserializeOwned {}
