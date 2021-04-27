extern crate version_check;

#[cfg(feature = "neon")]
fn main() {
    match version_check::Channel::read() {
        Some(c) if c.is_nightly() => {}
        Some(_) => panic!("The 'neon' feature requires a nightly compiler."),
        None => panic!("Unable to determine rustc version."),
    };
    match version_check::is_min_version("1.48.0") {
        Some(true) => {}
        Some(false) => panic!(
            "Unsupported rustc version: {}, CamillaDSP with the 'neon' feature needs at least: 1.48.0",
            version_check::Version::read().unwrap()
        ),
        None => panic!("Unable to determine rustc version."),
    };
}

#[cfg(not(feature = "neon"))]
fn main() {
    match version_check::is_min_version("1.43.0") {
        Some(true) => {}
        Some(false) => panic!(
            "Unsupported rustc version: {}, CamillaDSP needs at least: 1.43.0",
            version_check::Version::read().unwrap()
        ),
        None => panic!("Unable to determine rustc version."),
    };
}
