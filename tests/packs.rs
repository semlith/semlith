//! A pack whose bytes do not match the digest pinned for it is refused and
//! deleted, never kept; one that matches is unpacked and stamped.
//!
//! Ignored by default: it fetches a small file over the network.
//!
//! ```sh
//! cargo test --test packs -- --ignored
//! ```

use semlith::packs::{Asset, Form, Pack};

/// This repository's own licence at a fixed commit: small, and on a host a
/// CI runner can always reach.
const URL: &str = "https://raw.githubusercontent.com/semlith/semlith/33e23899e79ee0144cc63a582f5ff22f43d2d809/LICENSE";

#[test]
#[ignore = "fetches a file over the network"]
fn a_corrupted_pack_fails_its_digest_and_is_deleted() {
    let cache = tempfile::tempdir().unwrap();
    let wrong: &'static [Asset] = Box::leak(Box::new([Asset {
        url: URL,
        sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        size: 11_357,
        form: Form::File { name: "LICENSE" },
    }]));
    let pack = Pack {
        name: "digest-check",
        version: "1",
        assets: wrong,
    };
    let error = semlith::packs::fetch(cache.path(), &pack, &mut |_| {}).unwrap_err();
    let text = format!("{error:#}");
    assert!(
        text.contains("does not match the digest"),
        "the refusal should name the digest: {text}"
    );
    assert!(semlith::packs::installed(cache.path(), &pack).is_none());
    let dir = pack.dir(cache.path());
    let left: Vec<_> = std::fs::read_dir(&dir)
        .map(|d| d.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "the refused download was kept: {left:?}");
}
