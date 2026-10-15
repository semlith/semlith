//! S18: one planted 8 MiB file costs seconds, not CPU-minutes, in the two
//! passes that were quadratic in it — secret scanning and graph extraction.
//! The numbers are a release build's: `cargo test --release --test security
//! s18 -- --ignored --nocapture`.

#[allow(unused_imports)]
use super::*;

const PLANT: usize = 8 * 1024 * 1024 - 1024;

fn planted(line: impl Fn(usize) -> String) -> String {
    let mut out = String::new();
    let mut i = 0;
    loop {
        let next = line(i);
        if out.len() + next.len() > PLANT {
            return out;
        }
        out.push_str(&next);
        i += 1;
    }
}

#[test]
#[ignore = "a timing: run in --release"]
fn s18_a_planted_file_of_key_shapes_scans_in_seconds() {
    let text = planted(|i| {
        if i % 2 == 0 {
            format!("AKIA{i:016}\n")
        } else {
            "-----BEGIN RSA PRIVATE KEY-----\n".to_string()
        }
    });
    let started = Instant::now();
    let found = semlith::keyscan::scan("keys.txt", &text);
    let took = started.elapsed();
    eprintln!("keyscan: {} matches in {took:?}", found.len());
    assert!(!found.is_empty());
    assert!(took < Duration::from_secs(5), "keyscan took {took:?}");
}

#[test]
#[ignore = "a timing: run in --release"]
fn s18_a_planted_file_of_functions_graphs_in_seconds() {
    let text = planted(|i| format!("function f{i}(){{g{i}()}}\n"));
    let started = Instant::now();
    let graph = semlith::graph::extract(Path::new("gen.js"), &text);
    let took = started.elapsed();
    eprintln!(
        "graph: {:?} in {took:?}",
        graph.as_ref().map(|g| g.is_some())
    );
    assert!(
        took < Duration::from_secs(5),
        "graph extraction took {took:?}"
    );
}
