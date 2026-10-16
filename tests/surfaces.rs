//! One ledger, every surface (0.42.0 item 13).
//!
//! The 2026-10-14 audit recomputed every figure from 16 real rows and found
//! the surfaces disagreeing with each other and with the rows: the savings
//! report said 16 retrievals at 75 % where the portal said 12 at 66 %, a
//! clients tile said 2 beside 3 clients listed. Here one fixture — two
//! clients, a search two stores answered, a zero hit, a read, a PDF hit, two
//! refunds and a person's portal search — is read back from `/api/ledger`,
//! `semlith ledger`, and the savings, access and gaps reports, and every
//! figure must be the same number `ledger::figures` computes from the rows.
//!
//! No model is loaded: the rows are written as the recorder writes them.

use semlith::Semlith;
use semlith::store::NewRetrieval;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn semlith(home: &Path) -> Command {
    let bin = std::env::var_os("SEMLITH_TEST_BIN")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_semlith").into());
    let mut command = Command::new(bin);
    command
        .env("SEMLITH_HOME", home)
        .env("HOME", home)
        .env("SEMLITH_MODEL_CACHE", home.join("no-models"))
        .env("SEMLITH_AIRGAP", "1")
        .env_remove("SEMLITH_STORE")
        .env_remove("SEMLITH_PORT")
        .env_remove("SEMLITH_LEDGER");
    command
}

fn row<'a>(client: &'a str, session: &'a str, query: &'a str, qid: &'a str) -> NewRetrieval<'a> {
    NewRetrieval {
        client,
        client_version: "2.1.299",
        session,
        tool: "search",
        query,
        hits: 1,
        micros: 60_000,
        excerpt_tokens: 100,
        whole_file_tokens: 1_000,
        stale_hits: 0,
        tokenizer: semlith::ledger::MODEL,
        query_id: qid,
    }
}

/// The two stores, registered, holding the audit's shape of rows.
fn fixture(root: &Path) -> (PathBuf, Vec<PathBuf>) {
    let home = root.join("home");
    let mut dirs = Vec::new();
    let mut registry = serde_json::Map::new();
    for name in ["alpha", "beta"] {
        let dir = home.join("stores").join(name);
        let work = root.join("work").join(name);
        std::fs::create_dir_all(&work).unwrap();
        drop(Semlith::open(&dir, None).unwrap());
        registry.insert(
            name.into(),
            serde_json::json!({ "roots": [work], "created": 0 }),
        );
        dirs.push(dir);
    }
    std::fs::write(
        home.join("registry.json"),
        serde_json::json!({ "stores": registry }).to_string(),
    )
    .unwrap();

    let (a, b) = (
        Semlith::open(&dirs[0], None).unwrap(),
        Semlith::open(&dirs[1], None).unwrap(),
    );
    let put = |s: &Semlith, r: NewRetrieval<'_>| {
        semlith::store::read_only(s.db(), false).unwrap();
        semlith::store::record_counted(s.db(), &r, semlith::store::BASIS_TEXT).unwrap();
    };
    // One search answered by both stores: one retrieval.
    put(
        &a,
        NewRetrieval {
            hits: 2,
            excerpt_tokens: 200,
            whole_file_tokens: 3_000,
            ..row("Claude Code", "c1", "how is a blueprint registered", "q1")
        },
    );
    put(
        &b,
        row("Claude Code", "c1", "how is a blueprint registered", "q1"),
    );
    // A zero hit, from a second client in a second conversation.
    put(
        &a,
        NewRetrieval {
            hits: 0,
            excerpt_tokens: 20,
            whole_file_tokens: 0,
            micros: 50_000,
            ..row("Cursor", "k1", "where is the tariff ledger", "q2")
        },
    );
    // A read, and a PDF hit counted from its text.
    put(
        &a,
        NewRetrieval {
            tool: "read",
            micros: 2_000,
            excerpt_tokens: 300,
            whole_file_tokens: 2_500,
            ..row("Claude Code", "c1", "src/app.py:1-80", "q3")
        },
    );
    put(
        &b,
        NewRetrieval {
            micros: 70_000,
            excerpt_tokens: 150,
            whole_file_tokens: 11_500,
            ..row("Claude Code", "c1", "attention is all you need", "q4")
        },
    );
    // Two refunds the steering hook saw, and a person at the portal.
    for (path, q) in [("/w/alpha/x.py", "q5"), ("/w/alpha/y.py", "q6")] {
        put(
            &a,
            NewRetrieval {
                tool: semlith::ledger::RAW_READ,
                hits: 0,
                micros: 0,
                excerpt_tokens: 0,
                whole_file_tokens: 900,
                ..row("Claude Code", "c1", path, q)
            },
        );
    }
    put(
        &b,
        NewRetrieval {
            client_version: "0.42.0",
            excerpt_tokens: 50,
            whole_file_tokens: 40_000,
            ..row("portal", "portal", "by hand", "q7")
        },
    );
    (home, dirs)
}

fn json_of(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

/// Every number in a typed report's figures, by KPI or fact label.
fn report_figures(home: &Path, kind: &str) -> serde_json::Value {
    let out = semlith(home)
        .args(["report", kind, "--format", "json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    json_of(&String::from_utf8_lossy(&out.stdout))
}

/// The figure under `label` anywhere in a report: a KPI or a fact.
fn figure(report: &serde_json::Value, label: &str) -> serde_json::Value {
    let mut found = None;
    let mut walk = vec![report];
    while let Some(v) = walk.pop() {
        match v {
            serde_json::Value::Object(map) => {
                if map.get("label").and_then(|l| l.as_str()) == Some(label)
                    && map.contains_key("value")
                {
                    found = Some(v.clone());
                }
                walk.extend(map.values());
            }
            serde_json::Value::Array(list) => walk.extend(list),
            _ => {}
        }
    }
    found.unwrap_or_else(|| panic!("no figure labelled {label} in {report}"))
}

#[test]
fn every_surface_prints_the_figures_the_rows_add_up_to() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, dirs) = fixture(tmp.path());
    let (a, b) = (
        Semlith::open(&dirs[0], None).unwrap(),
        Semlith::open(&dirs[1], None).unwrap(),
    );
    let f = semlith::ledger::figures(
        &[("alpha", a.db()), ("beta", b.db())],
        semlith::ledger::Window::ALL,
        false,
    )
    .unwrap();
    drop((a, b));
    // The rows, by hand: q1..q4 are the agents' four retrievals, q2 found
    // nothing, the refunds and the portal row are not queries.
    assert_eq!(
        (f.queries, f.credited, f.zero_hit, f.coverage),
        (4, 3, 1, 75)
    );
    assert_eq!(f.whole_file_tokens, 3_000 + 1_000 + 2_500 + 11_500);
    assert_eq!(f.excerpt_tokens, 200 + 100 + 300 + 150);
    assert_eq!(f.net_tokens, 18_000 - 750);
    assert_eq!((f.refunds, f.human.queries, f.sessions), (2, 1, 2));
    assert_eq!(f.clients, vec!["Claude Code", "Cursor"]);
    assert_eq!(f.by_client["Claude Code"], 3);
    assert_eq!((f.p50_micros, f.p95_micros), (60_000, 70_000));

    // `semlith ledger`.
    let out = semlith(&home).arg("ledger").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let want = format!(
        "saved {} tokens over {} of {} retrievals — coverage {} %",
        semlith::report::number(f.net_tokens),
        f.credited,
        f.queries,
        f.coverage
    );
    assert!(
        text.contains(&want),
        "the CLI says otherwise:\n{text}\nwanted: {want}"
    );
    assert!(text.contains("refunds 2"), "{text}");
    assert!(text.contains("latency p50 60 ms · p95 70 ms"), "{text}");

    // The reports.
    let savings = report_figures(&home, "savings");
    assert_eq!(
        figure(&savings, "Net tokens not read")["value"],
        f.net_tokens,
        "{savings}"
    );
    assert_eq!(
        figure(&savings, "Coverage")["value"],
        f.credited as f64 / f.queries as f64,
        "{savings}"
    );
    assert_eq!(figure(&savings, "Refunds")["value"], f.refunds, "{savings}");
    assert_eq!(
        figure(&savings, "Zero-hit")["value"],
        f.zero_hit,
        "{savings}"
    );
    assert_eq!(figure(&savings, "Human searches")["value"], f.human.queries);
    assert_eq!(
        figure(&savings, "Latency p50")["value"],
        f.p50_micros as f64 / 1000.0,
        "{savings}"
    );
    assert_eq!(
        figure(&savings, "Latency p95")["value"],
        f.p95_micros as f64 / 1000.0,
        "{savings}"
    );
    let access = report_figures(&home, "access");
    assert_eq!(figure(&access, "Queries")["value"], f.queries, "{access}");
    assert_eq!(
        figure(&access, "Agent sessions")["value"],
        f.sessions,
        "{access}"
    );
    assert_eq!(
        figure(&access, "Read whole")["value"],
        f.refunds,
        "{access}"
    );
    let gaps = report_figures(&home, "gaps");
    let gap_text = gaps.to_string();
    assert!(gap_text.contains("where is the tariff ledger"), "{gaps}");
    assert!(
        !gap_text.contains("x.py"),
        "a refund listed as a question: {gaps}"
    );

    // `/api/ledger`, the Ledger page's and Home's one source.
    let mut child = semlith(&home)
        .args(["start", "--port", "0"])
        .current_dir(tmp.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().unwrap())
        .read_line(&mut line)
        .unwrap();
    let rest = line
        .trim()
        .strip_prefix("http://127.0.0.1:")
        .unwrap()
        .to_string();
    let (port, token) = rest.split_once("/?token=").unwrap();
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap())).unwrap();
    write!(
        stream,
        "GET /api/ledger HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nSemlith-Token: {token}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let _ = child.kill();
    let _ = child.wait();
    let api = json_of(raw.split_once("\r\n\r\n").unwrap().1);
    for (key, want) in [
        ("queries", f.queries),
        ("credited", f.credited),
        ("coverage", f.coverage),
        ("net_tokens", f.net_tokens),
        ("whole_file_tokens", f.whole_file_tokens),
        ("excerpt_tokens", f.excerpt_tokens),
        ("refunds", f.refunds),
        ("zero_hit", f.zero_hit),
        ("clients", f.clients.len() as i64),
        ("sessions_total", f.sessions),
    ] {
        assert_eq!(api[key], want, "/api/ledger {key}: {api}");
    }
    assert_eq!(api["by_client"]["Claude Code"], 3, "{api}");
    assert_eq!(api["human"]["queries"], 1, "{api}");
    let store_net: i64 = api["by_store"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["net"].as_i64().unwrap())
        .sum();
    // Per store, a search two stores answered is counted in each; summed, the
    // shares are the whole: nothing clamped, nothing lost.
    assert_eq!(store_net, f.net_tokens, "{api}");
}
