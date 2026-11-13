//! A fake OpenRGB SDK server for the smoke tests: two RAM sticks and a
//! keyboard, all red, that accept Keeplight's requests without any real
//! lights. Prints its address on the first line, then serves until killed.
//!
//!   cargo run -p keeplight-core --features testing --example fake-openrgb [-- 127.0.0.1:0]

use keeplight_core::testing::{self, FakeServer, RED};

fn main() {
    let bind = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:0".into());
    let server = FakeServer::start_on(
        &bind,
        vec![testing::ram("SMBus 0x71"), testing::ram("SMBus 0x73"), testing::keyboard(RED)],
    );
    println!("{}", server.address);
    loop {
        std::thread::park();
    }
}
