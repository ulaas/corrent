#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| corrent_core::fuzz::z1(data));
