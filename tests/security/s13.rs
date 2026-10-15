//! S13: every read of a file's bytes goes through one open that refuses a
//! link, a FIFO or a device and caps what it reads. The walk-to-read race
//! itself cannot be staged reliably from outside, so this proves the
//! primitive every remaining reader now shares.

#[allow(unused_imports)]
use super::*;

#[cfg(unix)]
#[test]
fn s13_reads_refuse_a_link_a_fifo_and_a_device_and_stop_at_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.md");
    std::fs::write(&target, "zqtarget\n").unwrap();

    let link = dir.path().join("link.md");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(
        semlith::read_regular(&link, 1 << 20).is_err(),
        "a link was followed"
    );

    let fifo = dir.path().join("fifo.md");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let started = Instant::now();
    assert!(
        semlith::read_regular(&fifo, 1 << 20).is_err(),
        "a FIFO was read"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the read blocked on a FIFO"
    );

    assert!(
        semlith::read_regular(Path::new("/dev/zero"), 1 << 20).is_err(),
        "a device was read"
    );

    let big = dir.path().join("big.md");
    std::fs::write(&big, vec![b'a'; 2048]).unwrap();
    let err = semlith::read_regular(&big, 1024).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::FileTooLarge);
    assert_eq!(semlith::read_regular(&target, 1024).unwrap(), b"zqtarget\n");
}
