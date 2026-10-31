//! The macOS layer. Like `win32`, the only module tree here allowed to
//! use `unsafe`; every `unsafe` block says why it is sound.
//!
//! So far it only takes readings (CPU, RAM, network); drawing on the
//! desktop is still to come (`docs/PORTING.md`).

#![allow(unsafe_code)]

mod mach;
mod sensors;

pub use sensors::Sensors;
