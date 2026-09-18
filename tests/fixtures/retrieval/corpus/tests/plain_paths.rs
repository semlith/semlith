//! Every path an MCP tool hands back is one an editor opens and a shell
//! completes.
//!
//! The store's key is the verbatim form — `\\?\C:\work\api\src\lock.rs` — which
//! is what makes a path longer than 260 characters work on Windows and what
//! every lookup is done against. Only the text on its way out is plain, and
//! 0.19.0 fixed the two graph tools and the daemon's own events, which were
//! still handing the raw key to agents.
//!
//! The fixture store is written with verbatim keys by hand rather than indexed,
//! so this asserts the rendering on every platform rather than only on the one
//! whose `canonicalize` produces them.

use semlith::{Semlith, fleet::Fleet, graph::Symbol, store};
use std::io::Cursor;

/// The store's key, exactly as Windows records it.
const KEY: &str = r"\\?\C:\work\api\src\lock.rs";

fn fixture(dir: &std::path::Path) {
    let mut s = Semlith::open(dir, None).unwrap();
    s.quiet = true;
    let db = s.db();
    // A store connection refuses writes unless a run has lifted the refusal.
    // This is a fixture, not a run, so it lifts it itself and puts it back.
    store::read_only(db, false).unwrap();
    let file = store::insert_file(db, KEY, "hash", 64, 0).unwrap();
    let chunk = store::insert_chunk(db, file, 0, 1, 3, "fn acquire() {}\n").unwrap();
    store::insert_symbol(
        db,
        file,
        Some(chunk),
        &Symbol {
            kind: "function".to_string(),
            name: "acquire".to_string(),
            qualified: "acquire".to_string(),
            start_line: 1,
            end_line: 3,
        },
    )
    .unwrap();
    store::read_only(db, true).unwrap();
}

/// Drive the stdio server with one `tools/call` and return its text.
fn call(dir: &std::path::Path, tool: &str, args: serde_json::Value) -> String {
    let mut fleet = Fleet::open(&[dir.to_path_buf()]).unwrap();
    fleet.quiet = true;
    // 0.21.0 moved the model load off the handshake, so the server takes the
    // fleet behind a lock and the store names `tools/list` prints separately.
    let labels = fleet.labels().join(", ");
    let fleet = std::sync::Mutex::new(fleet);
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": args },
    });
    let input = format!("{request}\n");
    let mut out = Vec::new();
    semlith::mcp::serve(&fleet, &labels, Cursor::new(input.into_bytes()), &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn no_mcp_tool_hands_back_a_verbatim_path() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());

    for (tool, args) in [
        ("semlith_files", serde_json::json!({})),
        ("semlith_symbol", serde_json::json!({ "name": "acquire" })),
        (
            "semlith_path",
            serde_json::json!({ "from": "acquire", "to": "acquire" }),
        ),
        (
            "semlith_neighbors",
            serde_json::json!({ "name": "acquire" }),
        ),
    ] {
        let answer = call(dir.path(), tool, args);
        assert!(
            !answer.contains(r"\\?\") && !answer.contains(r"\\\\?\\"),
            "{tool} handed back a verbatim path:\n{answer}"
        );
        assert!(
            !answer.contains("UNC\\"),
            "{tool} handed back a UNC verbatim path:\n{answer}"
        );
    }
}

/// And the store still finds the file under the key it actually holds. A
/// release that stripped the prefix where the path is stored would have made
/// every lookup miss.
#[test]
fn the_store_still_holds_the_verbatim_key() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let s = Semlith::open(dir.path(), None).unwrap();
    let paths = store::all_paths(s.db()).unwrap();
    assert_eq!(paths, vec![KEY.to_string()]);
}
