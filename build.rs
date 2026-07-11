fn main() {
    println!("cargo:rerun-if-changed=native/airwindows/airwindows_bridge.h");
    println!("cargo:rerun-if-changed=native/airwindows/airwindows_bridge.cpp");
    println!("cargo:rerun-if-changed=native/airwindows/totape8.cpp");
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .warnings(true)
        .flag_if_supported("-fno-exceptions")
        .flag_if_supported("-fno-rtti")
        .file("native/airwindows/airwindows_bridge.cpp")
        .file("native/airwindows/totape8.cpp")
        .compile("pureroad_airwindows");
}
