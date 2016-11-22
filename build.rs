fn main() {
    if cfg!(windows) {
        println!("cargo:rustc-link-search=native={}", "C:/Windows/System32/");
    }
}
