#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| corrent_server::webapi::fuzz::z4(data));
