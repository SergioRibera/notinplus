//! Bytes from the LAN, a relay or a peer, after any seal opened: every
//! decoder must fail cleanly, never abort or allocate past what the input
//! justifies.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| notin_sync::fuzz_decode(data));
