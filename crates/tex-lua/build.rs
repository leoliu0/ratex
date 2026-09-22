fn main() {
    println!("cargo:rerun-if-changed=src/c_api_shim.c");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("wasm32") {
        cc::Build::new()
            .file("src/c_api_shim.c")
            // Keep the full Lua ABI in the native archive; these entry points
            // are consumed externally rather than referenced from Rust.
            .flag_if_supported("-fno-function-sections")
            .flag_if_supported("-fno-data-sections")
            .warnings(true)
            .compile("tex_lua_c_api_shim");
    }
}
