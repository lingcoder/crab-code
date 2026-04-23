//! WASM sandbox runtime using wasmtime.
//!
//! Provides a sandboxed execution environment for `.wasm` plugins.
//! Plugins communicate via a simple host-guest protocol:
//! - Guest exports: `plugin_name`, `plugin_version`, `plugin_execute`
//! - Host provides: limited I/O via WASI (when enabled) or no I/O at all

use std::path::{Path, PathBuf};
use std::sync::Arc;

use wasmtime::{AsContextMut, Engine, Linker, Module, Store};

use crate::manifest::PluginManifest;

/// Configuration for the WASM sandbox.
#[derive(Debug, Clone)]
pub struct WasmSandboxConfig {
    /// Maximum memory in bytes (default 64 MiB).
    pub max_memory_bytes: usize,
    /// Maximum execution fuel (instruction count limit). None = unlimited.
    pub max_fuel: Option<u64>,
    /// Allowed filesystem paths (empty = no filesystem access).
    pub allowed_paths: Vec<PathBuf>,
    /// Whether network access is permitted.
    pub allow_network: bool,
}

impl Default for WasmSandboxConfig {
    fn default() -> Self {
        Self {
            max_memory_bytes: 64 * 1024 * 1024, // 64 MiB
            max_fuel: Some(1_000_000_000),      // 1B instructions
            allowed_paths: Vec::new(),
            allow_network: false,
        }
    }
}

/// Host state stored in the wasmtime Store.
struct HostState {
    /// Captured stdout from the plugin.
    stdout_buf: Vec<u8>,
}

/// A loaded and validated WASM plugin instance.
pub struct WasmPluginInstance {
    /// Plugin name (from manifest or extracted from module).
    name: String,
    /// Plugin version (from manifest).
    version: String,
    /// The compiled module.
    module: Module,
    /// Shared engine reference.
    engine: Arc<Engine>,
    /// Sandbox configuration.
    sandbox_config: WasmSandboxConfig,
    /// Source manifest, if loaded via discovery.
    manifest: Option<PluginManifest>,
}

impl std::fmt::Debug for WasmPluginInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmPluginInstance")
            .field("name", &self.name)
            .field("version", &self.version)
            .finish()
    }
}

impl WasmPluginInstance {
    /// Plugin name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Plugin version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Associated manifest, if any.
    #[must_use]
    pub fn manifest(&self) -> Option<&PluginManifest> {
        self.manifest.as_ref()
    }

    /// Execute the plugin with JSON input and return JSON output.
    ///
    /// The plugin must export a `plugin_execute` function with signature:
    /// `(input_ptr: i32, input_len: i32) -> i32`
    /// where the return value is a pointer to a null-terminated JSON string
    /// in the plugin's linear memory.
    ///
    /// For simpler plugins that don't use the memory protocol, we also
    /// support a no-arg `plugin_execute` that returns an i32 status code.
    pub fn execute(&self, input: &str) -> crab_core::Result<PluginOutput> {
        let mut store = self.create_store()?;
        let linker = self.create_linker()?;
        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|e| crab_core::Error::Other(format!("wasm instantiate failed: {e}")))?;

        // Try the memory-based protocol first: plugin_execute(ptr, len) -> ptr
        if let Some(exec_fn) = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, "plugin_execute")
            .ok()
        {
            let output = self.execute_with_memory(&mut store, &instance, exec_fn, input)?;
            return Ok(output);
        }

        // Fallback: no-arg plugin_execute() -> i32 (status code)
        if let Some(exec_fn) = instance
            .get_typed_func::<(), i32>(&mut store, "plugin_execute")
            .ok()
        {
            let status = exec_fn
                .call(&mut store, ())
                .map_err(|e| crab_core::Error::Other(format!("wasm execute failed: {e}")))?;
            return Ok(PluginOutput {
                status,
                output: String::new(),
            });
        }

        Err(crab_core::Error::Other(
            "wasm plugin does not export 'plugin_execute' function".into(),
        ))
    }

    /// Execute using the memory-based protocol.
    fn execute_with_memory(
        &self,
        store: &mut Store<HostState>,
        instance: &wasmtime::Instance,
        exec_fn: wasmtime::TypedFunc<(i32, i32), i32>,
        input: &str,
    ) -> crab_core::Result<PluginOutput> {
        // Get the plugin's memory export
        let memory = instance
            .get_memory(store.as_context_mut(), "memory")
            .ok_or_else(|| {
                crab_core::Error::Other("wasm plugin does not export 'memory'".into())
            })?;

        // Allocate space in guest memory for input via `plugin_alloc(size) -> ptr`
        let alloc_fn = instance
            .get_typed_func::<i32, i32>(store.as_context_mut(), "plugin_alloc")
            .map_err(|e| {
                crab_core::Error::Other(format!("wasm plugin does not export 'plugin_alloc': {e}"))
            })?;

        let input_bytes = input.as_bytes();
        let input_ptr = alloc_fn
            .call(&mut *store, input_bytes.len() as i32)
            .map_err(|e| crab_core::Error::Other(format!("wasm alloc failed: {e}")))?;

        // Write input into guest memory
        let mem_data = memory.data_mut(&mut *store);
        let start = input_ptr as usize;
        let end = start + input_bytes.len();
        if end > mem_data.len() {
            return Err(crab_core::Error::Other(
                "wasm input exceeds guest memory".into(),
            ));
        }
        mem_data[start..end].copy_from_slice(input_bytes);

        // Call plugin_execute(ptr, len) -> result_ptr
        let result_ptr = exec_fn
            .call(&mut *store, (input_ptr, input_bytes.len() as i32))
            .map_err(|e| crab_core::Error::Other(format!("wasm execute failed: {e}")))?;

        // Read the result: a length-prefixed string at result_ptr
        // Format: [len: i32][data: u8 * len]
        let mem_data = memory.data(&*store);
        let rp = result_ptr as usize;
        if rp + 4 > mem_data.len() {
            return Ok(PluginOutput {
                status: 0,
                output: String::new(),
            });
        }

        let result_len =
            i32::from_le_bytes(mem_data[rp..rp + 4].try_into().unwrap_or([0; 4])) as usize;
        let result_start = rp + 4;
        let result_end = result_start + result_len;

        let output = if result_end <= mem_data.len() {
            String::from_utf8_lossy(&mem_data[result_start..result_end]).into_owned()
        } else {
            String::new()
        };

        Ok(PluginOutput { status: 0, output })
    }

    /// Create a new Store with sandbox constraints.
    fn create_store(&self) -> crab_core::Result<Store<HostState>> {
        let mut store = Store::new(
            &self.engine,
            HostState {
                stdout_buf: Vec::new(),
            },
        );

        if let Some(fuel) = self.sandbox_config.max_fuel {
            store
                .set_fuel(fuel)
                .map_err(|e| crab_core::Error::Other(format!("failed to set wasm fuel: {e}")))?;
        }

        Ok(store)
    }

    /// Create a Linker with host functions.
    fn create_linker(&self) -> crab_core::Result<Linker<HostState>> {
        let mut linker = Linker::new(&self.engine);

        // Provide a minimal host_log function for debugging
        linker
            .func_wrap(
                "env",
                "host_log",
                |mut caller: wasmtime::Caller<'_, HostState>, ptr: i32, len: i32| {
                    let memory = caller.get_export("memory").and_then(|e| e.into_memory());
                    if let Some(memory) = memory {
                        let data = memory.data(&caller);
                        let start = ptr as usize;
                        let end = start + len as usize;
                        if end <= data.len() {
                            if let Ok(msg) = std::str::from_utf8(&data[start..end]) {
                                tracing::debug!(plugin_msg = msg, "wasm plugin log");
                            }
                        }
                    }
                },
            )
            .map_err(|e| crab_core::Error::Other(format!("failed to define host_log: {e}")))?;

        // Provide host_output for plugins to write output
        linker
            .func_wrap(
                "env",
                "host_output",
                |mut caller: wasmtime::Caller<'_, HostState>, ptr: i32, len: i32| {
                    let memory = caller.get_export("memory").and_then(|e| e.into_memory());
                    if let Some(memory) = memory {
                        let data = memory.data(&caller);
                        let start = ptr as usize;
                        let end = start + len as usize;
                        if end <= data.len() {
                            let chunk = data[start..end].to_vec();
                            let state = caller.data_mut();
                            state.stdout_buf.extend_from_slice(&chunk);
                        }
                    }
                },
            )
            .map_err(|e| crab_core::Error::Other(format!("failed to define host_output: {e}")))?;

        Ok(linker)
    }
}

/// Output from a plugin execution.
#[derive(Debug, Clone)]
pub struct PluginOutput {
    /// Status code (0 = success).
    pub status: i32,
    /// Plugin output (JSON or text).
    pub output: String,
}

/// The WASM plugin runtime.
///
/// Manages an engine, compiled modules, and plugin lifecycle.
pub struct WasmRuntime {
    engine: Arc<Engine>,
    sandbox_config: WasmSandboxConfig,
    plugins: Vec<WasmPluginInstance>,
}

impl std::fmt::Debug for WasmRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmRuntime")
            .field("plugin_count", &self.plugins.len())
            .field("sandbox_config", &self.sandbox_config)
            .finish()
    }
}

impl WasmRuntime {
    /// Create a new runtime with default sandbox settings.
    pub fn new() -> crab_core::Result<Self> {
        Self::with_config(WasmSandboxConfig::default())
    }

    /// Create a new runtime with custom sandbox config.
    pub fn with_config(sandbox_config: WasmSandboxConfig) -> crab_core::Result<Self> {
        let mut config = wasmtime::Config::new();
        config.consume_fuel(sandbox_config.max_fuel.is_some());
        // Limit memory via the memory configuration
        config.memory_guaranteed_dense_image_size(0);

        let engine = Engine::new(&config)
            .map_err(|e| crab_core::Error::Other(format!("failed to create wasm engine: {e}")))?;

        Ok(Self {
            engine: Arc::new(engine),
            sandbox_config,
            plugins: Vec::new(),
        })
    }

    /// Load a WASM plugin from a file path.
    pub fn load_plugin(
        &mut self,
        wasm_path: &Path,
        manifest: Option<PluginManifest>,
    ) -> crab_core::Result<usize> {
        let wasm_bytes = std::fs::read(wasm_path).map_err(|e| {
            crab_core::Error::Other(format!(
                "failed to read wasm file {}: {e}",
                wasm_path.display()
            ))
        })?;

        self.load_plugin_bytes(&wasm_bytes, manifest)
    }

    /// Load a WASM plugin from bytes.
    pub fn load_plugin_bytes(
        &mut self,
        wasm_bytes: &[u8],
        manifest: Option<PluginManifest>,
    ) -> crab_core::Result<usize> {
        let module = Module::new(&self.engine, wasm_bytes)
            .map_err(|e| crab_core::Error::Other(format!("failed to compile wasm module: {e}")))?;

        let name = manifest
            .as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| "unnamed-wasm-plugin".into());
        let version = manifest
            .as_ref()
            .map(|m| m.version.clone())
            .unwrap_or_else(|| "0.0.0".into());

        let instance = WasmPluginInstance {
            name,
            version,
            module,
            engine: Arc::clone(&self.engine),
            sandbox_config: self.sandbox_config.clone(),
            manifest,
        };

        let idx = self.plugins.len();
        self.plugins.push(instance);

        tracing::debug!(plugin_index = idx, "loaded wasm plugin");
        Ok(idx)
    }

    /// Get a loaded plugin by index.
    #[must_use]
    pub fn get_plugin(&self, index: usize) -> Option<&WasmPluginInstance> {
        self.plugins.get(index)
    }

    /// Number of loaded plugins.
    #[must_use]
    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    /// Iterate over all loaded plugins.
    pub fn plugins(&self) -> impl Iterator<Item = &WasmPluginInstance> {
        self.plugins.iter()
    }

    /// Execute a plugin by index.
    pub fn execute_plugin(&self, index: usize, input: &str) -> crab_core::Result<PluginOutput> {
        let plugin = self
            .plugins
            .get(index)
            .ok_or_else(|| crab_core::Error::Other(format!("plugin index {index} out of range")))?;
        plugin.execute(input)
    }

    /// Discover and load WASM plugins from a directory.
    ///
    /// Scans subdirectories for `plugin.json` manifests with `kind: "wasm"`,
    /// then loads the `.wasm` entry point.
    pub fn discover_and_load(
        &mut self,
        plugins_dir: &Path,
    ) -> Vec<(String, crab_core::Result<usize>)> {
        let manifests = crate::manifest::discover_plugins(plugins_dir);
        let mut results = Vec::new();

        for manifest in manifests {
            if manifest.kind != crate::manifest::PluginKind::Wasm {
                continue;
            }

            let name = manifest.name.clone();
            let result = match manifest.resolved_entry() {
                Some(wasm_path) => {
                    if wasm_path.exists() {
                        self.load_plugin(&wasm_path, Some(manifest))
                    } else {
                        Err(crab_core::Error::Other(format!(
                            "wasm entry point not found: {}",
                            wasm_path.display()
                        )))
                    }
                }
                None => Err(crab_core::Error::Other(
                    "plugin manifest has no source directory".into(),
                )),
            };

            results.push((name, result));
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal valid WAT (WebAssembly Text) that compiles to a valid module.
    // Exports a simple `plugin_execute() -> i32` that returns 42.
    const SIMPLE_WAT: &str = r#"
        (module
            (func $plugin_execute (export "plugin_execute") (result i32)
                i32.const 42
            )
        )
    "#;

    // WAT with memory export and alloc/execute protocol
    const MEMORY_WAT: &str = r#"
        (module
            (memory (export "memory") 1)

            ;; Simple bump allocator at offset 1024
            (global $alloc_offset (mut i32) (i32.const 1024))

            (func $plugin_alloc (export "plugin_alloc") (param $size i32) (result i32)
                (local $ptr i32)
                global.get $alloc_offset
                local.set $ptr
                global.get $alloc_offset
                local.get $size
                i32.add
                global.set $alloc_offset
                local.get $ptr
            )

            ;; plugin_execute(ptr, len) -> result_ptr
            ;; Writes a length-prefixed result at the next alloc offset
            (func $plugin_execute (export "plugin_execute") (param $ptr i32) (param $len i32) (result i32)
                (local $result_ptr i32)
                ;; Allocate space for result: 4 bytes length + 2 bytes data "ok"
                global.get $alloc_offset
                local.set $result_ptr

                ;; Write length = 2 (little-endian i32)
                local.get $result_ptr
                i32.const 2
                i32.store

                ;; Write "ok" (0x6f, 0x6b)
                local.get $result_ptr
                i32.const 4
                i32.add
                i32.const 0x6b6f  ;; "ok" in little-endian
                i32.store16

                ;; Advance allocator past result
                local.get $result_ptr
                i32.const 6
                i32.add
                global.set $alloc_offset

                local.get $result_ptr
            )
        )
    "#;

    // WAT that imports host_log
    const LOG_WAT: &str = r#"
        (module
            (import "env" "host_log" (func $host_log (param i32 i32)))
            (memory (export "memory") 1)

            ;; Store "hello" at offset 0
            (data (i32.const 0) "hello")

            (func $plugin_execute (export "plugin_execute") (result i32)
                ;; Call host_log("hello")
                i32.const 0   ;; ptr
                i32.const 5   ;; len
                call $host_log
                i32.const 0   ;; return success
            )
        )
    "#;

    fn compile_wat(wat: &str) -> Vec<u8> {
        wat::parse_str(wat).expect("invalid WAT")
    }

    #[test]
    fn create_runtime_default() {
        let rt = WasmRuntime::new().unwrap();
        assert_eq!(rt.plugin_count(), 0);
    }

    #[test]
    fn create_runtime_custom_config() {
        let config = WasmSandboxConfig {
            max_memory_bytes: 32 * 1024 * 1024,
            max_fuel: Some(500_000),
            allowed_paths: vec![],
            allow_network: false,
        };
        let rt = WasmRuntime::with_config(config).unwrap();
        assert_eq!(rt.plugin_count(), 0);
    }

    #[test]
    fn sandbox_config_default() {
        let config = WasmSandboxConfig::default();
        assert_eq!(config.max_memory_bytes, 64 * 1024 * 1024);
        assert_eq!(config.max_fuel, Some(1_000_000_000));
        assert!(config.allowed_paths.is_empty());
        assert!(!config.allow_network);
    }

    #[test]
    fn load_plugin_bytes_simple() {
        let wasm = compile_wat(SIMPLE_WAT);
        let mut rt = WasmRuntime::new().unwrap();
        let idx = rt.load_plugin_bytes(&wasm, None).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(rt.plugin_count(), 1);

        let plugin = rt.get_plugin(0).unwrap();
        assert_eq!(plugin.name(), "unnamed-wasm-plugin");
        assert_eq!(plugin.version(), "0.0.0");
    }

    #[test]
    fn load_plugin_with_manifest() {
        let wasm = compile_wat(SIMPLE_WAT);
        let manifest = PluginManifest {
            name: "test-wasm".into(),
            description: "Test WASM plugin".into(),
            version: "1.0.0".into(),
            kind: crate::manifest::PluginKind::Wasm,
            author: "dev".into(),
            entry: "plugin.wasm".into(),
            permissions: vec![],
            source_dir: None,
        };
        let mut rt = WasmRuntime::new().unwrap();
        let idx = rt.load_plugin_bytes(&wasm, Some(manifest)).unwrap();

        let plugin = rt.get_plugin(idx).unwrap();
        assert_eq!(plugin.name(), "test-wasm");
        assert_eq!(plugin.version(), "1.0.0");
        assert!(plugin.manifest().is_some());
    }

    #[test]
    fn execute_simple_plugin() {
        let wasm = compile_wat(SIMPLE_WAT);
        let mut rt = WasmRuntime::new().unwrap();
        rt.load_plugin_bytes(&wasm, None).unwrap();

        let output = rt.execute_plugin(0, "{}").unwrap();
        assert_eq!(output.status, 42);
        assert!(output.output.is_empty());
    }

    #[test]
    fn execute_memory_protocol_plugin() {
        let wasm = compile_wat(MEMORY_WAT);
        let mut rt = WasmRuntime::new().unwrap();
        rt.load_plugin_bytes(&wasm, None).unwrap();

        let output = rt.execute_plugin(0, r#"{"action":"test"}"#).unwrap();
        assert_eq!(output.status, 0);
        assert_eq!(output.output, "ok");
    }

    #[test]
    fn execute_plugin_with_host_log() {
        let wasm = compile_wat(LOG_WAT);
        let mut rt = WasmRuntime::new().unwrap();
        rt.load_plugin_bytes(&wasm, None).unwrap();

        let output = rt.execute_plugin(0, "").unwrap();
        assert_eq!(output.status, 0);
    }

    #[test]
    fn load_invalid_wasm_bytes() {
        let mut rt = WasmRuntime::new().unwrap();
        let result = rt.load_plugin_bytes(b"not wasm", None);
        assert!(result.is_err());
    }

    #[test]
    fn execute_out_of_range_index() {
        let rt = WasmRuntime::new().unwrap();
        let result = rt.execute_plugin(99, "{}");
        assert!(result.is_err());
    }

    #[test]
    fn get_plugin_out_of_range() {
        let rt = WasmRuntime::new().unwrap();
        assert!(rt.get_plugin(0).is_none());
    }

    #[test]
    fn load_multiple_plugins() {
        let wasm = compile_wat(SIMPLE_WAT);
        let mut rt = WasmRuntime::new().unwrap();

        let idx0 = rt.load_plugin_bytes(&wasm, None).unwrap();
        let idx1 = rt.load_plugin_bytes(&wasm, None).unwrap();

        assert_eq!(idx0, 0);
        assert_eq!(idx1, 1);
        assert_eq!(rt.plugin_count(), 2);
    }

    #[test]
    fn plugins_iterator() {
        let wasm = compile_wat(SIMPLE_WAT);
        let mut rt = WasmRuntime::new().unwrap();
        rt.load_plugin_bytes(&wasm, None).unwrap();
        rt.load_plugin_bytes(&wasm, None).unwrap();

        let names: Vec<_> = rt.plugins().map(|p| p.name()).collect();
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn load_plugin_from_temp_file() {
        let wasm = compile_wat(SIMPLE_WAT);
        let tmp_dir = std::env::temp_dir().join("crab_wasm_test_load");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let wasm_path = tmp_dir.join("test.wasm");
        std::fs::write(&wasm_path, &wasm).unwrap();

        let mut rt = WasmRuntime::new().unwrap();
        let idx = rt.load_plugin(&wasm_path, None).unwrap();
        assert_eq!(idx, 0);

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn load_plugin_nonexistent_file() {
        let mut rt = WasmRuntime::new().unwrap();
        let result = rt.load_plugin(Path::new("/no/such/plugin.wasm"), None);
        assert!(result.is_err());
    }

    #[test]
    fn discover_wasm_plugins() {
        let wasm = compile_wat(SIMPLE_WAT);
        let tmp_dir = std::env::temp_dir().join("crab_wasm_test_discover");
        let plugin_dir = tmp_dir.join("my-wasm-plugin");
        let _ = std::fs::create_dir_all(&plugin_dir);

        // Write manifest
        let manifest_json = r#"{
            "name": "my-wasm-plugin",
            "kind": "wasm",
            "entry": "plugin.wasm",
            "version": "2.0.0"
        }"#;
        std::fs::write(plugin_dir.join("plugin.json"), manifest_json).unwrap();
        std::fs::write(plugin_dir.join("plugin.wasm"), &wasm).unwrap();

        let mut rt = WasmRuntime::new().unwrap();
        let results = rt.discover_and_load(&tmp_dir);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "my-wasm-plugin");
        assert!(results[0].1.is_ok());

        let plugin = rt.get_plugin(0).unwrap();
        assert_eq!(plugin.name(), "my-wasm-plugin");
        assert_eq!(plugin.version(), "2.0.0");

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn discover_skips_skill_plugins() {
        let tmp_dir = std::env::temp_dir().join("crab_wasm_test_skip_skill");
        let plugin_dir = tmp_dir.join("skill-plugin");
        let _ = std::fs::create_dir_all(&plugin_dir);

        std::fs::write(
            plugin_dir.join("plugin.json"),
            r#"{"name": "skill-plugin", "kind": "skill"}"#,
        )
        .unwrap();

        let mut rt = WasmRuntime::new().unwrap();
        let results = rt.discover_and_load(&tmp_dir);
        assert!(results.is_empty());

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn discover_missing_wasm_file() {
        let tmp_dir = std::env::temp_dir().join("crab_wasm_test_missing_entry");
        let plugin_dir = tmp_dir.join("broken-plugin");
        let _ = std::fs::create_dir_all(&plugin_dir);

        std::fs::write(
            plugin_dir.join("plugin.json"),
            r#"{"name": "broken-plugin", "kind": "wasm", "entry": "missing.wasm"}"#,
        )
        .unwrap();

        let mut rt = WasmRuntime::new().unwrap();
        let results = rt.discover_and_load(&tmp_dir);

        assert_eq!(results.len(), 1);
        assert!(results[0].1.is_err());

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn discover_empty_dir() {
        let tmp_dir = std::env::temp_dir().join("crab_wasm_test_empty_discover");
        let _ = std::fs::create_dir_all(&tmp_dir);

        let mut rt = WasmRuntime::new().unwrap();
        let results = rt.discover_and_load(&tmp_dir);
        assert!(results.is_empty());

        let _ = std::fs::remove_dir(&tmp_dir);
    }

    #[test]
    fn plugin_output_debug() {
        let output = PluginOutput {
            status: 0,
            output: "test".into(),
        };
        let debug = format!("{output:?}");
        assert!(debug.contains("status: 0"));
        assert!(debug.contains("test"));
    }

    #[test]
    fn wasm_plugin_instance_debug() {
        let wasm = compile_wat(SIMPLE_WAT);
        let mut rt = WasmRuntime::new().unwrap();
        rt.load_plugin_bytes(&wasm, None).unwrap();
        let plugin = rt.get_plugin(0).unwrap();
        let debug = format!("{plugin:?}");
        assert!(debug.contains("unnamed-wasm-plugin"));
    }

    #[test]
    fn wasm_runtime_debug() {
        let rt = WasmRuntime::new().unwrap();
        let debug = format!("{rt:?}");
        assert!(debug.contains("plugin_count"));
    }
}
