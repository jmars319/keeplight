//! Platform-neutral logic for Keeplight: the engine the window calls.
//!
//! Keeplight never talks to lighting hardware itself. OpenRGB does, and this
//! crate is a small client for OpenRGB's SDK port on 127.0.0.1 (`sdk`), the
//! device model (`device`), and the keeper's setup: what you chose to keep,
//! and putting it back (`keep`). Only `keep` sends requests that change a
//! light, and only for devices in the setup. `docs/safety-model.md` lists
//! every request.

pub mod device;
pub mod keep;
pub mod sdk;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
