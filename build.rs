fn main() {
    // Capture the host target triple at compile time so the `cc` crate
    // can be driven at *runtime* (when building grammars). `cc` normally
    // reads TARGET/HOST from the build-script environment, which isn't
    // present in the running binary — so we stash it in an env var.
    println!(
        "cargo:rustc-env=BUILD_TARGET={}",
        std::env::var("TARGET").expect("cargo sets TARGET for build scripts")
    );
}
