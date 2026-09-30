//! The usage readers against this machine's real client logs, on a copy of a
//! real ledger. Opt-in: set `SEMLITH_LIVE_LEDGER` to a `store.db` copied out
//! of a store (never the store itself), with usage turned on. Prints one line
//! per row it filled, for the release record.
//!
//! ```sh
//! sqlite3 ~/.semlith/stores/<name>/store.db ".backup /tmp/ledger.db"
//! SEMLITH_LIVE_LEDGER=/tmp/ledger.db cargo test --release --test usage_live -- --ignored --nocapture
//! ```

#[test]
#[ignore = "reads this machine's client logs; needs SEMLITH_LIVE_LEDGER"]
fn real_logs_fill_a_copy_of_a_real_ledger() {
    let Some(path) = std::env::var_os("SEMLITH_LIVE_LEDGER") else {
        eprintln!("SEMLITH_LIVE_LEDGER is not set; nothing to do");
        return;
    };
    assert!(
        semlith::usage::enabled(),
        "turn usage on first: semlith ledger --usage on"
    );
    let db = semlith::store::open(std::path::Path::new(&path)).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let started = std::time::Instant::now();
    let filled = semlith::usage::enrich(&db, now).unwrap();
    println!("filled {filled} rows in {:?}", started.elapsed());
    let mut q = db
        .prepare(
            "SELECT datetime(at, 'unixepoch'), client, tool, model, input_tokens, output_tokens,
                    cache_read_tokens, cache_write_tokens, reasoning_tokens, cost_usd, cost_source,
                    usage_source
               FROM retrievals WHERE usage_source IS NOT NULL ORDER BY at",
        )
        .unwrap();
    let rows = q
        .query_map([], |r| {
            Ok(format!(
                "{} | {} | {} | {} | in {:?} out {:?} cr {:?} cw {:?} re {:?} | ${:?} {:?} | {}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<i64>>(6)?,
                r.get::<_, Option<i64>>(7)?,
                r.get::<_, Option<i64>>(8)?,
                r.get::<_, Option<f64>>(9)?,
                r.get::<_, Option<String>>(10)?,
                r.get::<_, String>(11)?,
            ))
        })
        .unwrap();
    for row in rows {
        println!("{}", row.unwrap());
    }
}
