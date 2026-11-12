//! Integration regression tests for `validate_config()` and the RawFile probe.
//!
//! These exercise the REAL `validate_config()` (not a mirror of its logic), so a
//! refactor that reintroduces a blocking open, or that changes the probe's error
//! behaviour, is caught here.
//!
//! The key case is `no_writer_fifo_is_reachable_without_blocking`: against the
//! unpatched blocking `File::open()` it would hang, so the test wraps the call in
//! a watchdog and fails rather than hanging the suite.

#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use camillalib::config::{Configuration, validate_config};

/// Build a minimal RawFile+Alsa config pointing capture at `fifo`.
/// Playback is Stdout so no hardware is touched.
fn rawfile_config(fifo: &Path) -> Configuration {
    let yaml = format!(
        r#"
devices:
  samplerate: 44100
  chunksize: 2048
  queuelimit: 1
  capture:
    type: RawFile
    channels: 2
    format: S32_LE
    filename: "{fifo}"
  playback:
    type: Stdout
    channels: 2
    format: S32_LE
pipeline: []
"#,
        fifo = fifo.display()
    );
    yaml_serde::from_str(&yaml).expect("config should parse")
}

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "camilla-validate-test-{}-{}",
        std::process::id(),
        tag
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Run `validate_config` on a worker thread with a watchdog. Returns
/// `Ok(true)` if it returned Ok, `Ok(false)` if it returned Err, or
/// `Err(elapsed)` if it did not finish within `budget`.
fn validate_with_watchdog(conf: Configuration, budget: Duration) -> Result<bool, Duration> {
    // validate_config needs &mut; move the config into the thread.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut c = conf;
        let r = validate_config(&mut c, None);
        let _ = tx.send(r.is_ok());
    });
    let start = Instant::now();
    match rx.recv_timeout(budget) {
        Ok(ok) => Ok(ok),
        Err(_) => Err(start.elapsed()),
    }
}

#[test]
fn no_writer_fifo_is_reachable_without_blocking() {
    let d = tmpdir("fifo");
    let fifo = d.join("input.fifo");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(
        unsafe { nix::libc::mkfifo(c.as_ptr(), 0o644) },
        0,
        "mkfifo failed"
    );
    assert!(fifo.metadata().unwrap().file_type().is_fifo());

    let conf = rawfile_config(&fifo);
    let res = validate_with_watchdog(conf, Duration::from_secs(5));

    match res {
        Ok(true) => { /* reached Ok: the probe did not block and accepted the FIFO */ }
        Ok(false) => panic!("validate_config returned Err for a valid writer-less FIFO"),
        Err(elapsed) => panic!(
            "validate_config did NOT return within 5s (blocked for {:?}); \
             this is the regression: the RawFile probe must not block",
            elapsed
        ),
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn regular_readable_file_is_accepted() {
    let d = tmpdir("regular");
    let f = d.join("input.raw");
    let mut fh = std::fs::File::create(&f).unwrap();
    fh.write_all(&[0u8; 64]).unwrap();
    drop(fh);

    let conf = rawfile_config(&f);
    assert!(validate_with_watchdog(conf, Duration::from_secs(5)).unwrap());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn missing_file_is_rejected() {
    let d = tmpdir("missing");
    let missing = d.join("nope.raw");
    let conf = rawfile_config(&missing);
    let ok = validate_with_watchdog(conf, Duration::from_secs(5)).unwrap();
    assert!(!ok, "a missing capture file must be rejected");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn unreadable_file_is_rejected() {
    let d = tmpdir("perms");
    let f = d.join("secret.raw");
    std::fs::File::create(&f).unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).unwrap();

    // root bypasses permission bits; skip the assertion there
    if unsafe { nix::libc::geteuid() } == 0 {
        println!("running as root; permission semantics not enforced");
        let _ = std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644));
        let _ = std::fs::remove_dir_all(&d);
        return;
    }

    let conf = rawfile_config(&f);
    let ok = validate_with_watchdog(conf, Duration::from_secs(5)).unwrap();
    assert!(!ok, "an unreadable capture file must be rejected");
    let _ = std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn directory_is_not_a_valid_raw_capture() {
    // A directory is not a usable raw capture source. The probe may accept the
    // open (Linux permits O_RDONLY on a directory) but validate_config must not
    // hang; assert it returns within the watchdog and does not block.
    let d = tmpdir("dir");
    let conf = rawfile_config(&d);
    let res = validate_with_watchdog(conf, Duration::from_secs(5));
    assert!(
        res.is_ok(),
        "validate_config must return (Ok or Err) for a directory, not block"
    );
    let _ = std::fs::remove_dir_all(&d);
}
