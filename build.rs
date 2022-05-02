extern crate version_check;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    match version_check::is_min_version("1.61.0") {
        Some(true) => {}
        Some(false) => panic!(
            "Unsupported rustc version: {}, CamillaDSP needs at least: 1.61.0",
            version_check::Version::read().unwrap()
        ),
        None => panic!("Unable to determine rustc version."),
    };
}
