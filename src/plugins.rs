use crate::demod::{DemodPlugin, DemodPluginRegistry};
use anyhow::{Context, Result};
use libloading::{Library, Symbol};
use std::path::Path;

/// FFI interface that plugins must implement
///
/// Plugins are compiled as dynamic libraries (.so on Linux, .dll on Windows, .dylib on macOS)
/// and must export a `create_dsp_plugin` function that returns a boxed DSPPlugin trait object.
///
/// # Example C plugin:
/// ```c
/// typedef struct {
///     const char* name;
///     void (*process)(float* input, float* output, size_t count, float sample_rate);
///     void (*destroy)(void* state);
///     void* state;
/// } DspPlugin;
///
/// DspPlugin* create_dsp_plugin() {
///     DspPlugin* p = malloc(sizeof(DspPlugin));
///     p->name = "my_filter";
///     p->process = my_process;
///     p->destroy = my_destroy;
///     p->state = NULL;
///     return p;
/// }
/// ```
#[repr(C)]
pub struct CDspPlugin {
    pub name: *const libc::c_char,
    pub process: extern "C" fn(
        state: *mut libc::c_void,
        input_i: *const f32,
        input_q: *const f32,
        output_i: *mut f32,
        output_q: *mut f32,
        count: libc::size_t,
        sample_rate: f32,
    ),
    pub destroy: extern "C" fn(state: *mut libc::c_void),
    pub state: *mut libc::c_void,
    pub set_param: extern "C" fn(state: *mut libc::c_void, key: *const libc::c_char, value: f64),
}

/// Trait for DSP plugins (Rust-native)
pub trait DspPlugin: Send + Sync {
    fn name(&self) -> &str;

    /// Process a block of IQ samples
    /// input and output are interleaved I/Q: [I0, Q0, I1, Q1, ...]
    fn process(&mut self, input: &[f32], output: &mut [f32], sample_rate: f32);

    /// Set a parameter by name
    fn set_param(&mut self, key: &str, value: f64);

    /// Get a parameter by name
    fn get_param(&self, key: &str) -> Option<f64>;

    /// Plugin description
    fn description(&self) -> &str {
        ""
    }
}

/// Wrapper for C plugins loaded via libloading
pub struct CPluginWrapper {
    _lib: Library, // keep library loaded
    plugin: *mut CDspPlugin,
    name: String,
    description: String,
}

// Safety: We assume the C plugin is thread-safe or used from one thread
unsafe impl Send for CPluginWrapper {}
unsafe impl Sync for CPluginWrapper {}

impl CPluginWrapper {
    /// Create a wrapper from a loaded dynamic library.
    ///
    /// # Safety
    ///
    /// `lib` must be a loaded plugin library exporting a `create_dsp_plugin`
    /// symbol with the expected FFI signature. The library is kept alive for
    /// the lifetime of the wrapper; the plugin's `destroy` callback is invoked
    /// on drop.
    pub unsafe fn new(lib: Library) -> Result<Self> {
        type CreateFn = unsafe extern "C" fn() -> *mut CDspPlugin;
        let create: Symbol<CreateFn> = lib
            .get(b"create_dsp_plugin\0")
            .context("Plugin missing 'create_dsp_plugin' symbol")?;

        let plugin = create();
        if plugin.is_null() {
            return Err(anyhow::anyhow!("Plugin create_dsp_plugin returned null"));
        }

        let name = if (*plugin).name.is_null() {
            "unnamed_plugin".to_string()
        } else {
            std::ffi::CStr::from_ptr((*plugin).name)
                .to_string_lossy()
                .into_owned()
        };

        Ok(CPluginWrapper {
            _lib: lib,
            plugin,
            name,
            description: String::new(),
        })
    }
}

impl DspPlugin for CPluginWrapper {
    fn name(&self) -> &str {
        &self.name
    }

    fn process(&mut self, input: &[f32], output: &mut [f32], sample_rate: f32) {
        unsafe {
            let count = input.len() / 2;
            if count == 0 {
                return;
            }

            // Split interleaved input into I/Q
            let mut input_i = Vec::with_capacity(count);
            let mut input_q = Vec::with_capacity(count);
            for chunk in input.chunks_exact(2) {
                input_i.push(chunk[0]);
                input_q.push(chunk[1]);
            }

            let mut output_i = vec![0.0f32; count];
            let mut output_q = vec![0.0f32; count];

            ((*self.plugin).process)(
                (*self.plugin).state,
                input_i.as_ptr(),
                input_q.as_ptr(),
                output_i.as_mut_ptr(),
                output_q.as_mut_ptr(),
                count,
                sample_rate,
            );

            // Interleave output
            for i in 0..count {
                output[i * 2] = output_i[i];
                output[i * 2 + 1] = output_q[i];
            }
        }
    }

    fn set_param(&mut self, key: &str, value: f64) {
        unsafe {
            let c_key = std::ffi::CString::new(key).unwrap_or_default();
            ((*self.plugin).set_param)((*self.plugin).state, c_key.as_ptr(), value);
        }
    }

    fn get_param(&self, _key: &str) -> Option<f64> {
        // C plugins don't support get_param in this simple interface
        None
    }

    fn description(&self) -> &str {
        &self.description
    }
}

impl Drop for CPluginWrapper {
    fn drop(&mut self) {
        unsafe {
            if !self.plugin.is_null() {
                ((*self.plugin).destroy)((*self.plugin).state);
                // Don't free the plugin struct itself - the destroy function handles state cleanup
                // The plugin struct is owned by the library
            }
        }
    }
}

/// Plugin manager that loads and manages DSP plugins (both signal processing and demodulation)
pub struct PluginManager {
    dsp_plugins: Vec<Box<dyn DspPlugin>>,
    demod_plugins: DemodPluginRegistry,
    plugin_paths: Vec<String>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            dsp_plugins: Vec::new(),
            demod_plugins: DemodPluginRegistry::default(),
            plugin_paths: Vec::new(),
        }
    }

    /// Get the demod plugin registry
    pub fn demod_registry(&self) -> &DemodPluginRegistry {
        &self.demod_plugins
    }

    /// Get mutable demod plugin registry
    pub fn demod_registry_mut(&mut self) -> &mut DemodPluginRegistry {
        &mut self.demod_plugins
    }

    /// Register a demod plugin
    pub fn register_demod(&mut self, plugin: Box<dyn DemodPlugin>) {
        self.demod_plugins.register(plugin);
    }

    /// List all demod plugins
    pub fn list_demod_plugins(&self) -> Vec<(&str, &str)> {
        self.demod_plugins.list()
    }

    /// Add a directory to search for plugins
    pub fn add_path(&mut self, path: impl Into<String>) {
        self.plugin_paths.push(path.into());
    }

    /// Load a signal processing plugin from a specific file path
    pub fn load_plugin<P: AsRef<Path>>(&mut self, path: P) -> Result<usize> {
        let path = path.as_ref();
        let lib = unsafe { Library::new(path) }
            .with_context(|| format!("Failed to load plugin: {}", path.display()))?;

        let plugin = unsafe { CPluginWrapper::new(lib) }
            .with_context(|| format!("Failed to initialize plugin: {}", path.display()))?;

        let name = plugin.name().to_string();
        let idx = self.dsp_plugins.len();
        self.dsp_plugins.push(Box::new(plugin));

        println!("Loaded DSP plugin '{}' from {}", name, path.display());
        Ok(idx)
    }

    /// Scan plugin paths and load all valid plugins
    pub fn scan_and_load(&mut self) -> Result<usize> {
        let mut files_to_load = Vec::new();

        for path_str in &self.plugin_paths {
            let path = Path::new(path_str);
            if !path.exists() {
                continue;
            }

            if path.is_dir() {
                let entries = std::fs::read_dir(path)?;
                for entry in entries {
                    let entry = entry?;
                    let file_path = entry.path();
                    let ext = file_path.extension().and_then(|e| e.to_str());

                    if ext == Some("so") || ext == Some("dll") || ext == Some("dylib") {
                        files_to_load.push(file_path);
                    }
                }
            } else if path.is_file() {
                files_to_load.push(path.to_path_buf());
            }
        }

        let mut loaded = 0;
        for file_path in files_to_load {
            if self.load_plugin(&file_path).is_ok() {
                loaded += 1;
            }
        }
        Ok(loaded)
    }

    pub fn get_dsp(&self, idx: usize) -> Option<&dyn DspPlugin> {
        self.dsp_plugins.get(idx).map(|p| p.as_ref())
    }

    /// Find DSP plugin by name
    pub fn find_dsp(&self, name: &str) -> Option<usize> {
        self.dsp_plugins.iter().position(|p| p.name() == name)
    }

    /// List all loaded DSP plugins
    pub fn list_dsp(&self) -> Vec<(&str, usize)> {
        self.dsp_plugins
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name(), i))
            .collect()
    }

    /// Number of loaded DSP plugins
    pub fn dsp_count(&self) -> usize {
        self.dsp_plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.dsp_plugins.is_empty() && self.demod_plugins.is_empty()
    }

    /// Process samples through all loaded DSP plugins (chained)
    pub fn process_chain(&mut self, input: &[f32], output: &mut [f32], sample_rate: f32) {
        if self.dsp_plugins.is_empty() {
            output.copy_from_slice(input);
            return;
        }

        let mut temp_in = input.to_vec();
        let mut temp_out = vec![0.0f32; output.len()];

        for plugin in self.dsp_plugins.iter_mut() {
            plugin.process(&temp_in, &mut temp_out, sample_rate);
            std::mem::swap(&mut temp_in, &mut temp_out);
        }

        output.copy_from_slice(&temp_in);
    }

    /// Process samples through a specific DSP plugin
    pub fn process_single(
        &mut self,
        idx: usize,
        input: &[f32],
        output: &mut [f32],
        sample_rate: f32,
    ) -> Result<()> {
        let plugin = self
            .dsp_plugins
            .get_mut(idx)
            .ok_or_else(|| anyhow::anyhow!("Plugin index {} not found", idx))?;
        plugin.process(input, output, sample_rate);
        Ok(())
    }

    /// Set parameter on a specific DSP plugin
    pub fn set_param(&mut self, idx: usize, key: &str, value: f64) -> Result<()> {
        let plugin = self
            .dsp_plugins
            .get_mut(idx)
            .ok_or_else(|| anyhow::anyhow!("Plugin index {} not found", idx))?;
        plugin.set_param(key, value);
        Ok(())
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Built-in example plugin: gain/attenuation
pub struct GainPlugin {
    gain_db: f64,
    name: String,
}

impl GainPlugin {
    pub fn new(gain_db: f64) -> Self {
        Self {
            gain_db,
            name: "gain".to_string(),
        }
    }
}

impl DspPlugin for GainPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn process(&mut self, input: &[f32], output: &mut [f32], _sample_rate: f32) {
        let gain_linear = 10.0f64.powf(self.gain_db / 20.0) as f32;
        for (i, &sample) in input.iter().enumerate() {
            output[i] = sample * gain_linear;
        }
    }

    fn set_param(&mut self, key: &str, value: f64) {
        if key == "gain" || key == "gain_db" {
            self.gain_db = value;
        }
    }

    fn get_param(&self, key: &str) -> Option<f64> {
        if key == "gain" || key == "gain_db" {
            Some(self.gain_db)
        } else {
            None
        }
    }

    fn description(&self) -> &str {
        "Simple gain/attenuation plugin. Parameter: gain (dB)"
    }
}

/// Built-in example plugin: noise gate
pub struct NoiseGatePlugin {
    threshold: f64,
    name: String,
}

impl NoiseGatePlugin {
    pub fn new(threshold_db: f64) -> Self {
        Self {
            threshold: threshold_db,
            name: "noise_gate".to_string(),
        }
    }
}

impl DspPlugin for NoiseGatePlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn process(&mut self, input: &[f32], output: &mut [f32], _sample_rate: f32) {
        let threshold_linear = 10.0f64.powf(self.threshold / 20.0) as f32;
        for (i, &sample) in input.iter().enumerate() {
            let mag = sample.abs();
            output[i] = if mag > threshold_linear { sample } else { 0.0 };
        }
    }

    fn set_param(&mut self, key: &str, value: f64) {
        if key == "threshold" || key == "threshold_db" {
            self.threshold = value;
        }
    }

    fn get_param(&self, key: &str) -> Option<f64> {
        if key == "threshold" || key == "threshold_db" {
            Some(self.threshold)
        } else {
            None
        }
    }

    fn description(&self) -> &str {
        "Noise gate plugin. Parameter: threshold (dB)"
    }
}
