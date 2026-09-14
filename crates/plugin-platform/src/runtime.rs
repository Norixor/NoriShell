use std::time::{Duration, Instant};

use norishell_core_api::{PluginHostRequest, PluginRuntimeOutput};
use wasmi::{
    Caller, Config, Engine, Extern, Instance, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, TypedFunc,
};

use crate::{PluginPlatformError, Result};

pub const WASM_MEMORY_EXPORT: &str = "memory";
pub const WASM_ALLOC_EXPORT: &str = "nvx_alloc";
pub const WASM_DEALLOC_EXPORT: &str = "nvx_dealloc";
pub const WASM_HANDLE_EXPORT: &str = "nvx_handle";
pub const HOST_IMPORT_MODULE: &str = "norishell.host";
pub const HOST_EMIT_IMPORT: &str = "emit";

#[derive(Debug, Clone, Copy)]
pub struct RuntimeLimits {
    pub max_module_bytes: usize,
    pub max_request_bytes: usize,
    pub max_output_bytes: usize,
    pub max_outputs: usize,
    pub fuel: u64,
    pub memory_bytes: usize,
    pub table_elements: usize,
    pub instances: usize,
    pub timeout: Duration,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            max_module_bytes: 16 * 1024 * 1024,
            max_request_bytes: 256 * 1024,
            max_output_bytes: 256 * 1024,
            max_outputs: 32,
            fuel: 20_000_000,
            memory_bytes: 32 * 1024 * 1024,
            table_elements: 4_096,
            instances: 1,
            timeout: Duration::from_secs(2),
        }
    }
}

pub trait PluginOutputWriter {
    fn write(&mut self, output: PluginRuntimeOutput) -> std::result::Result<(), String>;
}

impl<F> PluginOutputWriter for F
where
    F: FnMut(PluginRuntimeOutput) -> std::result::Result<(), String>,
{
    fn write(&mut self, output: PluginRuntimeOutput) -> std::result::Result<(), String> {
        self(output)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionReport {
    pub output_count: usize,
    pub fuel_consumed: u64,
    pub elapsed_milliseconds: u128,
}

struct HostState {
    limits: StoreLimits,
    request_id: String,
    outputs: Vec<PluginRuntimeOutput>,
    output_bytes: usize,
    max_outputs: usize,
    max_output_bytes: usize,
    rejected_output: bool,
    deadline: Option<Instant>,
}

impl HostState {
    fn new(runtime_limits: RuntimeLimits) -> Self {
        Self {
            limits: store_limits(runtime_limits),
            request_id: String::new(),
            outputs: Vec::new(),
            output_bytes: 0,
            max_outputs: runtime_limits.max_outputs,
            max_output_bytes: runtime_limits.max_output_bytes,
            rejected_output: false,
            deadline: None,
        }
    }

    fn begin_request(&mut self, request_id: &str, deadline: Instant) {
        self.request_id.clear();
        self.request_id.push_str(request_id);
        self.outputs.clear();
        self.output_bytes = 0;
        self.rejected_output = false;
        self.deadline = Some(deadline);
    }
}

/// A reusable current-protocol Wasm instance. `&mut self` serializes guest events,
/// so a guest can retain only its own Wasm state, never host objects or
/// concurrent callbacks.
pub struct WasmRuntime {
    // Keep all Wasmi objects alive for the complete guest instance lifetime.
    _engine: Engine,
    _module: Module,
    store: Store<HostState>,
    _instance: Instance,
    memory: Memory,
    allocate: TypedFunc<i32, i32>,
    deallocate: TypedFunc<(i32, i32), ()>,
    handle: TypedFunc<(i32, i32), i32>,
    limits: RuntimeLimits,
    poisoned: bool,
}

impl WasmRuntime {
    /// The current ABI requires `nvx_dealloc`, allowing the guest to release or
    /// reuse every host-written request buffer after handling the event.
    pub fn new(module_bytes: &[u8], limits: RuntimeLimits) -> Result<Self> {
        let started = Instant::now();
        if module_bytes.is_empty() || module_bytes.len() > limits.max_module_bytes {
            return Err(PluginPlatformError::InvalidWasmAbi);
        }
        let engine = runtime_engine();
        let module =
            Module::new(&engine, module_bytes).map_err(|_| PluginPlatformError::InvalidWasmAbi)?;
        check_timeout(started, limits.timeout)?;
        validate_imports(&module)?;

        let mut store = Store::new(&engine, HostState::new(limits));
        store.limiter(|state| &mut state.limits);
        let mut linker = Linker::new(&engine);
        install_host_import(&mut linker)?;
        let instance = linker
            .instantiate_and_start(&mut store, &module)
            .map_err(|_| PluginPlatformError::InvalidWasmAbi)?;
        check_timeout(started, limits.timeout)?;
        let memory = exported_memory(&instance, &store)?;
        let allocate = exported_allocate(&instance, &store)?;
        let deallocate = instance
            .get_typed_func::<(i32, i32), ()>(&store, WASM_DEALLOC_EXPORT)
            .map_err(|_| PluginPlatformError::InvalidWasmAbi)?;
        let handle = exported_handle(&instance, &store)?;

        Ok(Self {
            _engine: engine,
            _module: module,
            store,
            _instance: instance,
            memory,
            allocate,
            deallocate,
            handle,
            limits,
            poisoned: false,
        })
    }

    /// A failed guest transition poisons the instance. The process owner must
    /// rebuild it instead of resuming an unknown partial Wasm state.
    pub fn execute(
        &mut self,
        request: &PluginHostRequest,
        mut writer: impl PluginOutputWriter,
    ) -> Result<ExecutionReport> {
        if self.poisoned {
            return Err(PluginPlatformError::RuntimePoisoned);
        }
        validate_request(request, self.limits)?;
        let request_bytes = serde_json::to_vec(request)?;
        let result = self
            .execute_request(request, &request_bytes)
            .and_then(|(outputs, report)| {
                for output in outputs {
                    writer
                        .write(output)
                        .map_err(|_| PluginPlatformError::InvalidRuntimeOutput)?;
                }
                Ok(report)
            });
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn execute_request(
        &mut self,
        request: &PluginHostRequest,
        request_bytes: &[u8],
    ) -> Result<(Vec<PluginRuntimeOutput>, ExecutionReport)> {
        let started = Instant::now();
        self.store
            .data_mut()
            .begin_request(&request.request_id, started + self.limits.timeout);
        self.store
            .set_fuel(self.limits.fuel)
            .map_err(|_| PluginPlatformError::RuntimeQuotaExceeded)?;

        let request_length = i32::try_from(request_bytes.len())
            .map_err(|_| PluginPlatformError::RuntimeQuotaExceeded)?;
        let pointer = self
            .allocate
            .call(&mut self.store, request_length)
            .map_err(|_| PluginPlatformError::RuntimeQuotaExceeded)?;
        let pointer_usize =
            usize::try_from(pointer).map_err(|_| PluginPlatformError::InvalidWasmAbi)?;

        // Once allocation succeeds, deallocation is attempted even after a
        // failed memory write or handler trap. It runs under this event's fuel
        // and deadline limits, before the result can be reported.
        let status = self
            .memory
            .write(&mut self.store, pointer_usize, request_bytes)
            .map_err(|_| PluginPlatformError::InvalidWasmAbi)
            .and_then(|()| {
                self.handle
                    .call(&mut self.store, (pointer, request_length))
                    .map_err(|_| PluginPlatformError::RuntimeQuotaExceeded)
            });
        let deallocation = self
            .deallocate
            .call(&mut self.store, (pointer, request_length))
            .map_err(|_| PluginPlatformError::RuntimeQuotaExceeded);
        check_store_timeout(&self.store)?;
        let status = status?;
        deallocation?;
        if status != 0 || self.store.data().rejected_output {
            return Err(PluginPlatformError::InvalidRuntimeOutput);
        }
        let remaining = self
            .store
            .get_fuel()
            .map_err(|_| PluginPlatformError::RuntimeQuotaExceeded)?;
        let outputs = std::mem::take(&mut self.store.data_mut().outputs);
        let output_count = outputs.len();
        Ok((
            outputs,
            ExecutionReport {
                output_count,
                fuel_consumed: self.limits.fuel.saturating_sub(remaining),
                elapsed_milliseconds: started.elapsed().as_millis(),
            },
        ))
    }
}

/// Executes one current-ABI request. Long-lived Plugin Hosts must retain a
/// [`WasmRuntime`] instead so guest state survives consecutive events.
pub fn execute_wasm(
    module_bytes: &[u8],
    request: &PluginHostRequest,
    mut writer: impl PluginOutputWriter,
    limits: RuntimeLimits,
) -> Result<ExecutionReport> {
    WasmRuntime::new(module_bytes, limits)?.execute(request, |output| writer.write(output))
}

fn runtime_engine() -> Engine {
    let mut config = Config::default();
    config.consume_fuel(true);
    Engine::new(&config)
}

fn store_limits(limits: RuntimeLimits) -> StoreLimits {
    StoreLimitsBuilder::new()
        .memory_size(limits.memory_bytes)
        .table_elements(limits.table_elements)
        .instances(limits.instances)
        .memories(1)
        .tables(1)
        .trap_on_grow_failure(true)
        .build()
}

fn validate_request(request: &PluginHostRequest, limits: RuntimeLimits) -> Result<()> {
    if !norishell_core_api::plugin_protocol_is_compatible(
        request.protocol_major,
        request.protocol_minor,
    ) || norishell_core_api::plugin_message_min_protocol_minor(request.kind)
        > request.protocol_minor
        || request.request_id.is_empty()
        || request.request_id.len() > 120
    {
        return Err(PluginPlatformError::InvalidWasmAbi);
    }
    if serde_json::to_vec(request)?.len() > limits.max_request_bytes {
        return Err(PluginPlatformError::RuntimeQuotaExceeded);
    }
    Ok(())
}

fn validate_imports(module: &Module) -> Result<()> {
    let imports = module.imports().collect::<Vec<_>>();
    if imports.len() != 1
        || imports[0].module() != HOST_IMPORT_MODULE
        || imports[0].name() != HOST_EMIT_IMPORT
    {
        return Err(PluginPlatformError::InvalidWasmAbi);
    }
    Ok(())
}

fn install_host_import(linker: &mut Linker<HostState>) -> Result<()> {
    linker
        .func_wrap(
            HOST_IMPORT_MODULE,
            HOST_EMIT_IMPORT,
            |mut caller: Caller<'_, HostState>, pointer: i32, length: i32| -> i32 {
                match capture_output(&mut caller, pointer, length) {
                    Ok(()) => 0,
                    Err(()) => {
                        caller.data_mut().rejected_output = true;
                        -1
                    }
                }
            },
        )
        .map_err(|_| PluginPlatformError::InvalidWasmAbi)?;
    Ok(())
}

fn exported_memory(instance: &Instance, store: &Store<HostState>) -> Result<Memory> {
    instance
        .get_export(store, WASM_MEMORY_EXPORT)
        .and_then(Extern::into_memory)
        .ok_or(PluginPlatformError::InvalidWasmAbi)
}

fn exported_allocate(instance: &Instance, store: &Store<HostState>) -> Result<TypedFunc<i32, i32>> {
    instance
        .get_typed_func::<i32, i32>(store, WASM_ALLOC_EXPORT)
        .map_err(|_| PluginPlatformError::InvalidWasmAbi)
}

fn exported_handle(
    instance: &Instance,
    store: &Store<HostState>,
) -> Result<TypedFunc<(i32, i32), i32>> {
    instance
        .get_typed_func::<(i32, i32), i32>(store, WASM_HANDLE_EXPORT)
        .map_err(|_| PluginPlatformError::InvalidWasmAbi)
}

fn capture_output(
    caller: &mut Caller<'_, HostState>,
    pointer: i32,
    length: i32,
) -> std::result::Result<(), ()> {
    if caller
        .data()
        .deadline
        .is_some_and(|deadline| Instant::now() > deadline)
    {
        return Err(());
    }
    let pointer = usize::try_from(pointer).map_err(|_| ())?;
    let length = usize::try_from(length).map_err(|_| ())?;
    if length == 0
        || caller.data().outputs.len() >= caller.data().max_outputs
        || caller.data().output_bytes.saturating_add(length) > caller.data().max_output_bytes
    {
        return Err(());
    }
    let memory = caller
        .get_export(WASM_MEMORY_EXPORT)
        .and_then(Extern::into_memory)
        .ok_or(())?;
    let mut bytes = vec![0_u8; length];
    memory.read(&*caller, pointer, &mut bytes).map_err(|_| ())?;
    let output: PluginRuntimeOutput = serde_json::from_slice(&bytes).map_err(|_| ())?;
    if output.request_id != caller.data().request_id
        || output.kind.is_empty()
        || output.kind.len() > 80
        || output.payload_json.len() > caller.data().max_output_bytes
        || serde_json::from_str::<serde_json::Value>(&output.payload_json).is_err()
    {
        return Err(());
    }
    let state = caller.data_mut();
    state.output_bytes += length;
    state.outputs.push(output);
    Ok(())
}

fn check_timeout(started: Instant, timeout: Duration) -> Result<()> {
    if started.elapsed() > timeout {
        Err(PluginPlatformError::RuntimeTimedOut)
    } else {
        Ok(())
    }
}

fn check_store_timeout(store: &Store<HostState>) -> Result<()> {
    if store
        .data()
        .deadline
        .is_some_and(|deadline| Instant::now() > deadline)
    {
        Err(PluginPlatformError::RuntimeTimedOut)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use norishell_core_api::{
        PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR, PluginHostMessageKind, PluginHostRequest,
        PluginRuntimeOutput,
    };

    use super::{PluginPlatformError, RuntimeLimits, WasmRuntime, execute_wasm};

    fn request(protocol_minor: u16) -> PluginHostRequest {
        PluginHostRequest {
            protocol_major: PLUGIN_PROTOCOL_MAJOR,
            protocol_minor,
            request_id: "request-1".to_owned(),
            kind: PluginHostMessageKind::Initialize,
            payload_json: "{}".to_owned(),
        }
    }

    fn wat_string(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("\\{byte:02x}")).collect()
    }

    fn persistent_state_wasm() -> Vec<u8> {
        let first = PluginRuntimeOutput {
            request_id: "request-1".to_owned(),
            kind: "state".to_owned(),
            payload_json: "{\"call\":1}".to_owned(),
        };
        let second = PluginRuntimeOutput {
            request_id: "request-1".to_owned(),
            kind: "state".to_owned(),
            payload_json: "{\"call\":2}".to_owned(),
        };
        let first = serde_json::to_vec(&first).expect("first output");
        let second = serde_json::to_vec(&second).expect("second output");
        wat::parse_str(format!(
            "(module
               (import \"norishell.host\" \"emit\" (func $emit (param i32 i32) (result i32)))
               (memory (export \"memory\") 1)
               (global $freed (mut i32) (i32.const 0))
               (data (i32.const 0) \"{}\")
               (data (i32.const 512) \"{}\")
               (func (export \"nvx_alloc\") (param i32) (result i32) i32.const 4096)
               (func (export \"nvx_dealloc\") (param i32 i32) global.get $freed i32.const 1 i32.add global.set $freed)
               (func (export \"nvx_handle\") (param i32 i32) (result i32)
                 global.get $freed
                 (if (then i32.const 512 i32.const {} call $emit drop)
                     (else i32.const 0 i32.const {} call $emit drop))
                 i32.const 0))",
            wat_string(&first),
            wat_string(&second),
            second.len(),
            first.len(),
        ))
        .expect("persistent wasm")
    }

    #[test]
    fn current_abi_delivers_bounded_json_output() {
        let output = PluginRuntimeOutput {
            request_id: "request-1".to_owned(),
            kind: "initialized".to_owned(),
            payload_json: "{\"ready\":true}".to_owned(),
        };
        let encoded = serde_json::to_vec(&output).expect("output json");
        let wasm = wat::parse_str(format!(
            "(module
               (import \"norishell.host\" \"emit\" (func $emit (param i32 i32) (result i32)))
               (memory (export \"memory\") 1)
               (data (i32.const 0) \"{}\")
               (func (export \"nvx_alloc\") (param i32) (result i32) i32.const 4096)
               (func (export \"nvx_dealloc\") (param i32 i32))
               (func (export \"nvx_handle\") (param i32 i32) (result i32)
                 i32.const 0 i32.const {} call $emit drop i32.const 0))",
            wat_string(&encoded),
            encoded.len()
        ))
        .expect("wasm");
        let mut outputs = Vec::new();
        let report = execute_wasm(
            &wasm,
            &request(PLUGIN_PROTOCOL_MINOR),
            |output| {
                outputs.push(output);
                Ok(())
            },
            RuntimeLimits::default(),
        )
        .expect("execute");
        assert_eq!(report.output_count, 1);
        assert_eq!(outputs, vec![output]);
    }

    #[test]
    fn persistent_runtime_retains_guest_state_and_deallocates_every_request() {
        let mut runtime = WasmRuntime::new(&persistent_state_wasm(), RuntimeLimits::default())
            .expect("persistent runtime");
        let mut outputs = Vec::new();
        runtime
            .execute(&request(PLUGIN_PROTOCOL_MINOR), |output| {
                outputs.push(output);
                Ok(())
            })
            .expect("first execution");
        runtime
            .execute(&request(PLUGIN_PROTOCOL_MINOR), |output| {
                outputs.push(output);
                Ok(())
            })
            .expect("second execution");
        assert_eq!(outputs[0].payload_json, "{\"call\":1}");
        assert_eq!(outputs[1].payload_json, "{\"call\":2}");
    }

    #[test]
    fn persistent_runtime_resets_fuel_for_each_request() {
        let wasm = wat::parse_str(
            "(module
               (import \"norishell.host\" \"emit\" (func (param i32 i32) (result i32)))
               (memory (export \"memory\") 1)
               (func (export \"nvx_alloc\") (param i32) (result i32) i32.const 0)
               (func (export \"nvx_dealloc\") (param i32 i32))
               (func (export \"nvx_handle\") (param i32 i32) (result i32)
                 (local $count i32)
                 block $done
                   loop $loop
                     local.get $count i32.const 3000 i32.ge_u br_if $done
                     local.get $count i32.const 1 i32.add local.set $count
                     br $loop
                   end
                 end
                 i32.const 0))",
        )
        .expect("wasm");
        let limits = RuntimeLimits {
            // Each event uses about 12k fuel. Both calls only succeed when the
            // persistent store receives a fresh 15k budget for each event.
            fuel: 15_000,
            ..RuntimeLimits::default()
        };
        let mut runtime = WasmRuntime::new(&wasm, limits).expect("persistent runtime");
        let first = runtime
            .execute(&request(PLUGIN_PROTOCOL_MINOR), |_| Ok(()))
            .expect("first execution");
        let second = runtime
            .execute(&request(PLUGIN_PROTOCOL_MINOR), |_| Ok(()))
            .expect("second execution");
        assert!(first.fuel_consumed > 10_000);
        assert!(second.fuel_consumed > 10_000);
    }

    #[test]
    fn persistent_runtime_rejects_missing_deallocator() {
        let wasm = wat::parse_str(
            "(module
               (import \"norishell.host\" \"emit\" (func (param i32 i32) (result i32)))
               (memory (export \"memory\") 1)
               (func (export \"nvx_alloc\") (param i32) (result i32) i32.const 0)
               (func (export \"nvx_handle\") (param i32 i32) (result i32) i32.const 0))",
        )
        .expect("wasm");
        assert!(matches!(
            WasmRuntime::new(&wasm, RuntimeLimits::default()),
            Err(PluginPlatformError::InvalidWasmAbi)
        ));
    }

    #[test]
    fn invalid_output_request_association_poisons_persistent_runtime() {
        let output = PluginRuntimeOutput {
            request_id: "other-request".to_owned(),
            kind: "state".to_owned(),
            payload_json: "{}".to_owned(),
        };
        let encoded = serde_json::to_vec(&output).expect("output json");
        let wasm = wat::parse_str(format!(
            "(module
               (import \"norishell.host\" \"emit\" (func $emit (param i32 i32) (result i32)))
               (memory (export \"memory\") 1)
               (data (i32.const 0) \"{}\")
               (func (export \"nvx_alloc\") (param i32) (result i32) i32.const 4096)
               (func (export \"nvx_dealloc\") (param i32 i32))
               (func (export \"nvx_handle\") (param i32 i32) (result i32)
                 i32.const 0 i32.const {} call $emit drop i32.const 0))",
            wat_string(&encoded),
            encoded.len()
        ))
        .expect("wasm");
        let mut runtime = WasmRuntime::new(&wasm, RuntimeLimits::default()).expect("runtime");
        assert!(matches!(
            runtime.execute(&request(PLUGIN_PROTOCOL_MINOR), |_| Ok(())),
            Err(PluginPlatformError::InvalidRuntimeOutput)
        ));
        assert!(matches!(
            runtime.execute(&request(PLUGIN_PROTOCOL_MINOR), |_| Ok(())),
            Err(PluginPlatformError::RuntimePoisoned)
        ));
    }

    #[test]
    fn fuel_stops_an_infinite_current_plugin() {
        let wasm = wat::parse_str(
            "(module
               (import \"norishell.host\" \"emit\" (func (param i32 i32) (result i32)))
               (memory (export \"memory\") 1)
               (func (export \"nvx_alloc\") (param i32) (result i32) i32.const 0)
               (func (export \"nvx_handle\") (param i32 i32) (result i32)
                 (loop $forever br $forever) i32.const 0))",
        )
        .expect("wasm");
        let limits = RuntimeLimits {
            fuel: 1_000,
            ..RuntimeLimits::default()
        };
        assert!(execute_wasm(&wasm, &request(PLUGIN_PROTOCOL_MINOR), |_| Ok(()), limits).is_err());
    }

    #[test]
    fn wasi_or_additional_imports_are_rejected() {
        let wasm = wat::parse_str(
            "(module
               (import \"wasi_snapshot_preview1\" \"fd_write\"
                 (func (param i32 i32 i32 i32) (result i32)))
               (memory (export \"memory\") 1)
               (func (export \"nvx_alloc\") (param i32) (result i32) i32.const 0)
               (func (export \"nvx_handle\") (param i32 i32) (result i32) i32.const 0))",
        )
        .expect("wasm");
        assert!(
            execute_wasm(
                &wasm,
                &request(PLUGIN_PROTOCOL_MINOR),
                |_| Ok(()),
                RuntimeLimits::default()
            )
            .is_err()
        );
    }
}
