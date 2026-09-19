//! A reader that goes away is not an error.
//!
//! `semlith files | head` and `semlith search … | less` are the most ordinary
//! things anyone types at this binary, and until 0.17.1 both printed a panic
//! and exited non-zero (#75). On unix the Rust runtime ignores `SIGPIPE`, so
//! the write returns `EPIPE` and `println!` panics on it; on Windows the write
//! fails with `BrokenPipe` and reaches the same panic.
//!
//! No model and no store: the output under test is `semlith languages`, which
//! prints through this binary's own `println!` and needs nothing on disk.
//! `--help` would not do — clap prints that itself and already swallows a
//! write error, so a test against it passes whether or not this is fixed.

use std::io::Read;
use std::process::{Command, Stdio};

#[test]
fn closing_the_pipe_is_not_a_panic() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("languages")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("semlith runs");

    // The reader goes away at once, rather than after a read: `--help` is a few
    // kilobytes, which fits a pipe buffer whole, so a test that read first and
    // closed after would prove nothing on a fast machine. Closing the read end
    // before the child has printed is the same condition `head -1` creates on a
    // long listing, and it does not depend on how much is written.
    drop(child.stdout.take().expect("a pipe"));

    let mut said = String::new();
    child
        .stderr
        .take()
        .expect("a pipe")
        .read_to_string(&mut said)
        .expect("stderr reads");
    let status = child.wait().expect("semlith exits");

    assert!(
        !said.contains("panicked at"),
        "printing to a closed pipe panicked:\n{said}"
    );
    assert_ne!(
        status.code(),
        Some(101),
        "the process exited with a panic status: {said}"
    );
}
