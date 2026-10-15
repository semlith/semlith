//! The HTTP request head parser (`read_head`, src/http.rs) through the facade
//! `semlith::fuzzing::http_head`: the bytes are what a peer sent before closing.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    semlith::fuzzing::http_head(data);
});
