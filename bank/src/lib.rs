//! Saltation bank server library. The binary in `main.rs` wires it together;
//! integration tests build the same routers through [`app`].

pub mod app;
pub mod features;
pub mod shared;
