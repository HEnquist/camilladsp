extern crate version_check;

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
