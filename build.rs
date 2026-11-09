fn main() {
    // Text recognition links against the system's Tesseract
    pkg_config::Config::new()
        .atleast_version("5.0")
        .probe("tesseract")
        .expect("Tesseract 5 is needed for text recognition");
}
