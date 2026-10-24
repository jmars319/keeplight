//! Platform-neutral logic for Keeplight.
//!
//! Keeplight never talks to lighting hardware itself. OpenRGB does, and this
//! crate is a small client for OpenRGB's SDK port on 127.0.0.1. In this
//! version the client only asks questions; it has no request that changes a
//! light. `docs/safety-model.md` lists every request it sends.

pub mod device;
pub mod sdk;
