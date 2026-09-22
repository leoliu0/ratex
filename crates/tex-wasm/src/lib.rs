//! JavaScript boundary for the shared in-memory document compiler.
#[cfg(target_arch = "wasm32")]
#[global_allocator]
static GLOBAL: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

#[cfg(target_arch = "wasm32")]
mod bindings {
    use tex_runtime::{Compilation, Session};
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(js_name = TexSession)]
    pub struct JsSession(Session);

    #[wasm_bindgen(js_class = TexSession)]
    impl JsSession {
        #[wasm_bindgen(constructor)]
        pub fn new() -> Self {
            Self(Session::new())
        }
        #[wasm_bindgen(js_name = addFile)]
        pub fn add_file(&mut self, name: &str, bytes: &[u8]) -> Result<(), JsValue> {
            self.0
                .add_file(name, bytes)
                .map_err(|s| JsValue::from_str(&s))
        }
        #[wasm_bindgen(js_name = removeFile)]
        pub fn remove_file(&mut self, name: &str) -> Result<(), JsValue> {
            self.0.remove_file(name).map_err(|s| JsValue::from_str(&s))
        }
        #[wasm_bindgen(js_name = setEpoch)]
        pub fn set_epoch(&mut self, epoch: Option<f64>) -> Result<(), JsValue> {
            if epoch.is_some_and(|n| {
                !n.is_finite() || n < 0.0 || n.fract() != 0.0 || n > 253_402_300_799.0
            }) {
                return Err(JsValue::from_str(
                    "expected nonnegative integer Unix seconds through year 9999",
                ));
            }
            self.0
                .set_epoch(epoch.map(|n| n as u64))
                .map_err(|s| JsValue::from_str(&s))
        }
        pub fn compile(&self, entry: &str) -> CompileResult {
            CompileResult(
                self.0
                    .compile_at(entry, (js_sys::Date::now() / 1000.0) as u64),
            )
        }
    }

    #[wasm_bindgen]
    pub struct CompileResult(Compilation);

    #[wasm_bindgen]
    impl CompileResult {
        #[wasm_bindgen(getter)]
        pub fn status(&self) -> u32 {
            self.0.status as u32
        }
        #[wasm_bindgen(getter)]
        pub fn pdf(&self) -> Vec<u8> {
            self.0.pdf.clone()
        }
        #[wasm_bindgen(getter)]
        pub fn log(&self) -> String {
            self.0.log.clone()
        }
        #[wasm_bindgen(getter)]
        pub fn diagnostics(&self) -> String {
            self.0.diagnostics.clone()
        }
        #[wasm_bindgen(getter)]
        pub fn passes(&self) -> u32 {
            self.0.passes
        }
        #[wasm_bindgen(getter, js_name = bibtexRuns)]
        pub fn bibtex_runs(&self) -> u32 {
            self.0.bibtex_runs
        }
        #[wasm_bindgen(getter, js_name = fileNames)]
        pub fn file_names(&self) -> Vec<String> {
            self.0.files.keys().cloned().collect()
        }
        pub fn file(&self, name: &str) -> Option<Vec<u8>> {
            self.0.files.get(name).cloned()
        }
    }
}
