//! The image path of the index pipeline (src/pipeline.rs): the header check
//! `image::too_large`, then — only when it passes — the decode the parse
//! worker's OP_IMAGE runs (`image::load_from_memory`, src/worker.rs).
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if semlith::image::too_large(data).is_none() {
        let _ = image::load_from_memory(data);
    }
});
