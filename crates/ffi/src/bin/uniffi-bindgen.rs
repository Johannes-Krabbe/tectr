//! Generates the Swift bindings, from the same UniFFI version the library is built with.

fn main() {
    uniffi::uniffi_bindgen_main();
}
