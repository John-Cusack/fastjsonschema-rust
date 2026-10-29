//! Fail is moved only on validation failure, never on the hot Ok path.
#![allow(clippy::result_large_err)]

 pub mod engine;
 pub mod plan;
pub use engine::{validate_one, Ctxt};
pub use plan::Plan;
