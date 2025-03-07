use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, Mutex},
};

use byteorder::{LittleEndian, ReadBytesExt};
use rand::RngCore;
use wasmi::{
    AsContext, AsContextMut, Caller, Engine, ExternRef, ExternType, Func, ImportType, Instance,
    Linker, Module, ResumableCall, Store, StoreContext, StoreContextMut, Value,
    core::{HostError, Trap, ValueType},
};

fn memory<'a, T>(caller: &'a mut Caller<T>) -> (&'a mut [u8], &'a mut T) {
    caller
        .get_export("memory")
        .unwrap()
        .into_memory()
        .unwrap()
        .data_and_store_mut(caller.as_context_mut())
}

// ----------------------------------------------------------------------------
// Asyncify

/*
enum AsyncifyState {
    Normal = 0,
    Unwind = 1,
    Rewind = 2,
}

/// Where the unwind/rewind data structure will live.
const STACK_START: u32 = 16;
const STACK_END: u32 = 1024;

fn asyncify(blob: &[u8]) -> Vec<u8> {
    let mut module = binaryen::Module::read(blob).unwrap();
    module
        .run_optimization_passes(["asyncify"], &binaryen::CodegenConfig::default())
        .unwrap();
    module.write()
}
*/

// ----------------------------------------------------------------------------

fn fake_import<T>(linker: &mut Linker<T>, import: &ImportType, message: &'static str) {
    if let ExternType::Func(func) = import.ty() {
        let r = linker.func_new(
            import.module(),
            import.name(),
            func.clone(),
            move |_caller, _inputs, _outputs| {
                panic!("{}", message);
            },
        );
        if !matches!(
            r,
            Err(wasmi::errors::LinkerError::DuplicateDefinition { .. })
        ) {
            r.unwrap();
        }
    }
}

// ----------------------------------------------------------------------------

type ContractCodeId = String;

type CodeHash = [u8; 32];

fn hash_code(code: &[u8]) -> CodeHash {
    [0; 32] // TODO
}

// ----------------------------------------------------------------------------

#[derive(Debug)]
struct Yield {
    data: u32,
}

impl std::fmt::Display for Yield {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Yield")
    }
}

impl HostError for Yield {}

/// Fulfiller of imports from `env`.
fn starstream_env<T>(
    linker: &mut Linker<T>,
    module: &str,
    this_code: &ContractCode,
    coordination_code: impl Fn(&T) -> &ContractCode + Send + Sync + 'static,
) {
    let this_code = this_code.hash;

    linker
        .func_wrap(module, "abort", || -> () {
            panic!("abort() called");
        })
        .unwrap();
    linker
        .func_wrap(module, "starstream_log", |v: u32| -> () {
            eprintln!("starstream_log: {v}");
        })
        .unwrap();
    linker
        .func_wrap(
            module,
            "starstream_coordination_code",
            move |mut caller: Caller<T>, return_addr: u32| {
                let (memory, env) = memory(&mut caller);
                let hash = &coordination_code(env).hash;
                memory[return_addr as usize..return_addr as usize + hash.len()]
                    .copy_from_slice(hash);
            },
        )
        .unwrap();
    linker
        .func_wrap(
            module,
            "starstream_this_code",
            move |mut caller: Caller<T>, return_addr: u32| {
                let (memory, _) = memory(&mut caller);
                memory[return_addr as usize..return_addr as usize + this_code.len()]
                    .copy_from_slice(&this_code);
            },
        )
        .unwrap();
}

/// Fulfiller of imports from `starstream_utxo_env`.
fn starstream_utxo_env(linker: &mut Linker<UtxoInstance>, module: &str) {
    linker
        .func_wrap(
            module,
            "starstream_yield",
            |mut caller: Caller<UtxoInstance>,
             name: u32,
             name_len: u32,
             data: u32,
             data_len: u32,
             resume_arg: u32,
             resume_arg_len: u32|
             -> Result<(), Trap> {
                eprintln!("YIELD");
                Err(Trap::from(Yield { data }))
            },
        )
        .unwrap();
}

// ----------------------------------------------------------------------------

struct ContractCode {
    wasm: Vec<u8>,
    pub hash: CodeHash,
}

impl ContractCode {
    fn load(wasm: Vec<u8>) -> ContractCode {
        ContractCode {
            hash: hash_code(&wasm),
            wasm,
        }
    }

    fn module(&self, engine: &Engine) -> Module {
        Module::new(engine, &self.wasm[..]).unwrap()
    }
}

impl std::fmt::Debug for ContractCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContractCode")
            .field("hash", &self.hash)
            .finish()
    }
}

// ----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct TokenId {
    id: usize,
}

impl TokenId {
    fn to_wasm_u32(self, mut store: StoreContextMut<UtxoInstance>) -> Value {
        let scrambled = rand::rng().next_u32();
        store.data_mut().temporary_token_ids.insert(scrambled, self);
        Value::I32(scrambled as i32)
    }

    fn to_wasm_externref(self, store: StoreContextMut<UtxoInstance>) -> Value {
        Value::ExternRef(ExternRef::new::<TokenId>(store, Some(self)))
    }

    fn from_wasm(value: &Value, store: StoreContext<UtxoInstance>) -> Option<TokenId> {
        match value {
            Value::I32(scrambled) => store
                .data()
                .temporary_token_ids
                .get(&(*scrambled as u32))
                .copied(),
            Value::ExternRef(handle) => handle.data(store)?.downcast_ref::<TokenId>().copied(),
            _ => None,
        }
    }
}

struct UtxoInstance {
    coordination_code: Arc<ContractCode>,

    tokens: Vec<Token>,
    temporary_token_ids: HashMap<u32, TokenId>,
}

fn utxo_linker(
    engine: &Engine,
    utxo_code: &ContractCode,
    coordination_code: &Arc<ContractCode>,
) -> Linker<UtxoInstance> {
    let mut linker = Linker::new(engine);

    starstream_env(&mut linker, "env", utxo_code, |instance: &UtxoInstance| {
        &instance.coordination_code
    });

    starstream_utxo_env(&mut linker, "starstream_utxo_env");

    for import in utxo_code.module(engine).imports() {
        if let ExternType::Func(func_ty) = import.ty() {
            if let Some(rest) = import.module().strip_prefix("starstream_token:") {
                if import.name().starts_with("starstream_mint_") {
                    let name = import.name().to_owned();
                    let rest = rest.to_owned();
                    let coordination_code = coordination_code.clone();
                    linker
                        .func_new(
                            import.module(),
                            import.name(),
                            func_ty.clone(),
                            move |mut caller, inputs, outputs| {
                                eprintln!("MINT {name:?} {inputs:?}");
                                let utxo = Token::mint(
                                    coordination_code.clone(),
                                    Universe::load_debug_uncached(&rest),
                                    &name,
                                    inputs,
                                );
                                let mut store = caller.as_context_mut();
                                let local_tokens = &mut store.data_mut().tokens;
                                let id = local_tokens.len();
                                local_tokens.push(utxo);
                                outputs[0] = TokenId { id }.to_wasm_u32(caller.as_context_mut());
                                Ok(())
                            },
                        )
                        .unwrap();
                } else if import.name().starts_with("starstream_burn_") {
                    linker
                        .func_wrap(import.module(), import.name(), |handle: u32| -> () {
                            todo!()
                        })
                        .unwrap();
                }
            } else {
                fake_import(&mut linker, &import, "not available in UTXO context");
            }
        }
    }

    linker
}

// ----------------------------------------------------------------------------

struct Utxo {
    code: Arc<ContractCode>,
    entry_point: String,
    store: RefCell<Store<UtxoInstance>>,
    instance: Instance,
    status: ResumableCall,
}

impl Utxo {
    fn start(
        coordination_code: Arc<ContractCode>,
        utxo_code: Arc<ContractCode>,
        entry_point: String,
        inputs: &[Value],
    ) -> Utxo {
        let engine = Engine::default();
        let mut store = Store::new(
            &engine,
            UtxoInstance {
                coordination_code: coordination_code.clone(),
                tokens: Default::default(),
                temporary_token_ids: Default::default(),
            },
        );
        let linker = utxo_linker(&engine, &utxo_code, &coordination_code);
        let instance = linker
            .instantiate(&mut store, &utxo_code.module(&engine))
            .unwrap()
            .ensure_no_start(&mut store)
            .unwrap();
        let main = instance.get_func(&mut store, &entry_point).unwrap();
        // TODO: call_resumable is naturally what we want here, but it's not
        // serializable to disk yet. We could patch wasmi to make it so, or go
        // back to binaryen-asyncify.
        let status = main.call_resumable(&mut store, inputs, &mut []).unwrap();
        Utxo {
            code: utxo_code,
            entry_point,
            store: RefCell::new(store),
            instance,
            status,
        }
    }

    fn is_alive(&self) -> bool {
        matches!(self.status, ResumableCall::Resumable(_))
    }

    fn resume(&mut self) {
        let ResumableCall::Resumable(resumable) =
            std::mem::replace(&mut self.status, ResumableCall::Finished)
        else {
            panic!("Cannot resume() after exit")
        };
        self.status = resumable
            .resume(self.store.borrow_mut().as_context_mut(), &[], &mut [])
            .unwrap();
    }

    fn query(&self, method: &str, inputs: &[Value], outputs: &mut [Value]) {
        eprintln!("query {method:?} {inputs:?} {}", outputs.len());
        let ResumableCall::Resumable(resumable) = &self.status else {
            panic!("Cannot query() after exit");
        };
        let inputs = std::iter::once(Value::I32(
            resumable.host_error().downcast_ref::<Yield>().unwrap().data as i32,
        ))
        .chain(inputs.iter().cloned())
        .collect::<Vec<_>>();

        let func = self
            .instance
            .get_func(self.store.borrow().as_context(), method)
            .unwrap();
        func.call(self.store.borrow_mut().as_context_mut(), &inputs, outputs)
            .unwrap()
    }

    fn mutate(&mut self, method: &str, inputs: &[Value], outputs: &mut [Value]) {
        eprintln!("mutate {method:?} {inputs:?} {}", outputs.len());
        let ResumableCall::Resumable(resumable) = &self.status else {
            panic!("Cannot query() after exit");
        };
        let inputs: Vec<Value> = std::iter::once(Value::I32(
            resumable.host_error().downcast_ref::<Yield>().unwrap().data as i32,
        ))
        .chain(inputs.iter().cloned())
        .collect::<Vec<_>>();

        let func = self
            .instance
            .get_func(self.store.borrow().as_context(), method)
            .unwrap();
        func.call(self.store.borrow_mut().as_context_mut(), &inputs, outputs)
            .unwrap()
    }

    fn consume(&mut self, method: &str, inputs: &[Value], outputs: &mut [Value]) {
        eprintln!("consume {method:?} {inputs:?} {}", outputs.len());
        let ResumableCall::Resumable(resumable) = &self.status else {
            panic!("Cannot query() after exit");
        };
        let inputs: Vec<Value> = std::iter::once(Value::I32(
            resumable.host_error().downcast_ref::<Yield>().unwrap().data as i32,
        ))
        .chain(inputs.iter().cloned())
        .collect::<Vec<_>>();

        let func = self
            .instance
            .get_func(self.store.borrow().as_context(), method)
            .unwrap();
        let r = func
            .call(self.store.borrow_mut().as_context_mut(), &inputs, outputs)
            .unwrap();
        self.status = ResumableCall::Finished;
        r
    }
}

impl std::fmt::Debug for Utxo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("Utxo");
        s.field("type", &self.entry_point);
        match &self.status {
            ResumableCall::Finished => {
                s.field("finished", &true);
            }
            ResumableCall::Resumable(resumable) => {
                let inputs = [Value::I32(
                    resumable.host_error().downcast_ref::<Yield>().unwrap().data as i32,
                )];

                let mut store = self.store.borrow_mut();
                let funcs = self
                    .instance
                    .exports(store.as_context())
                    .filter_map(|e| {
                        let n = e.name().to_owned();
                        e.into_func().map(|x| (n, x))
                    })
                    .collect::<Vec<_>>();
                for (name, func) in funcs {
                    let mut outputs = [Value::ExternRef(ExternRef::null())];
                    // TODO: narrow to relevant type only
                    if name.starts_with("starstream_query_")
                        && func.ty(store.as_context()).params() == &[ValueType::I32]
                        && func.ty(store.as_context()).results().len() == 1
                    {
                        func.call(store.as_context_mut(), &inputs, &mut outputs)
                            .unwrap();
                        s.field(&name, &outputs[0]);
                    }
                }
            }
        }
        s.finish()
    }
}

// ----------------------------------------------------------------------------

struct TokenInstance {
    coordination_code: Arc<ContractCode>,
}

fn token_linker(engine: &Engine, token_code: &Arc<ContractCode>) -> Linker<TokenInstance> {
    let mut linker = Linker::new(engine);

    starstream_env(
        &mut linker,
        "env",
        token_code,
        |instance: &TokenInstance| &instance.coordination_code,
    );

    for import in token_code.module(engine).imports() {
        fake_import(&mut linker, &import, "Not available in UTXO context");
    }

    linker
}

// ----------------------------------------------------------------------------

struct Token {
    code: Arc<ContractCode>,
    // Note: doesn't save Store or Instance, instead recreates it from scratch
    // on burn() call therefore not needing to persist aribtrary memory for
    // tokens.
    burn_fn: String,
    id: u64,
    amount: u64,
}

impl Token {
    fn mint(
        coordination_code: Arc<ContractCode>,
        token_code: Arc<ContractCode>,
        mint_fn: &str,
        inputs: &[Value],
    ) -> Token {
        let burn_fn = mint_fn.replace("starstream_mint_", "starstream_burn_");
        assert_ne!(mint_fn, burn_fn);

        // Prepend struct return slot to inputs
        let return_addr: usize = 16;
        let inputs = std::iter::once(Value::I32(return_addr as i32))
            .chain(inputs.iter().cloned())
            .collect::<Vec<_>>();

        let engine = Engine::default();
        let mut store = Store::new(&engine, TokenInstance { coordination_code });
        let linker = token_linker(&engine, &token_code);
        let instance = linker
            .instantiate(&mut store, &token_code.module(&engine))
            .unwrap()
            .ensure_no_start(&mut store)
            .unwrap();
        let mint = instance.get_func(&mut store, &mint_fn).unwrap();
        mint.call(&mut store, &inputs[..], &mut []).unwrap();

        // Read id and amount
        let memory = instance
            .get_export(&store, "memory")
            .unwrap()
            .into_memory()
            .unwrap()
            .data(&store);
        let mut cursor = &memory[return_addr..];
        let id = cursor.read_u64::<LittleEndian>().unwrap();
        let amount = cursor.read_u64::<LittleEndian>().unwrap();
        Token {
            code: token_code,

            burn_fn,
            id,
            amount,
        }
    }

    fn burn(self, burn_fn: &str, coordination_code: Arc<ContractCode>) {
        assert_eq!(self.burn_fn, burn_fn);

        let engine = Engine::default();
        let mut store = Store::new(&engine, TokenInstance { coordination_code });
        let linker = token_linker(&engine, &self.code);
        let instance = linker
            .instantiate(&mut store, &self.code.module(&engine))
            .unwrap()
            .ensure_no_start(&mut store)
            .unwrap();
        let burn = instance.get_func(&mut store, burn_fn).unwrap();
        burn.call(
            &mut store,
            &[Value::I64(self.id as i64), Value::I64(self.amount as i64)],
            &mut [],
        )
        .unwrap();
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Token")
            .field("burn_fn", &self.burn_fn)
            .field("id", &self.id)
            .field("amount", &self.amount)
            .finish()
    }
}

// ----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct UtxoId {
    id: usize,
}

impl UtxoId {
    fn to_wasm_u32(self, mut store: StoreContextMut<CoordinationScriptInstance>) -> Value {
        let scrambled = rand::rng().next_u32();
        store.data_mut().temporary_utxo_ids.insert(scrambled, self);
        Value::I32(scrambled as i32)
    }

    fn to_wasm_externref(self, store: StoreContextMut<CoordinationScriptInstance>) -> Value {
        Value::ExternRef(ExternRef::new::<UtxoId>(store, Some(self)))
    }

    fn from_wasm(value: &Value, store: StoreContext<CoordinationScriptInstance>) -> Option<UtxoId> {
        match value {
            Value::I32(scrambled) => store
                .data()
                .temporary_utxo_ids
                .get(&(*scrambled as u32))
                .copied(),
            Value::ExternRef(handle) => handle.data(store)?.downcast_ref::<UtxoId>().copied(),
            _ => None,
        }
    }
}

struct CoordinationScriptInstance<'tx> {
    coordination_code: &'tx ContractCode,
    utxos: &'tx mut Vec<Utxo>,
    temporary_utxo_ids: HashMap<u32, UtxoId>,
}

fn coordination_script_linker<'tx>(
    engine: &Engine,
    universe: &mut Universe,
    coordination_code: Arc<ContractCode>,
) -> Linker<CoordinationScriptInstance<'tx>> {
    let mut linker = Linker::new(engine);

    starstream_env(
        &mut linker,
        "env",
        &coordination_code,
        |env: &CoordinationScriptInstance| &env.coordination_code,
    );

    for import in coordination_code.module(&engine).imports() {
        if import.module() == "env" {
            // handled by starstream_env above
        } else if let Some(rest) = import.module().strip_prefix("starstream_utxo:") {
            if let ExternType::Func(func_ty) = import.ty() {
                let name = import.name().to_owned();
                if import.name().starts_with("starstream_status_") {
                } else if import.name().starts_with("starstream_resume_") {
                    linker
                        .func_new(
                            import.module(),
                            import.name(),
                            func_ty.clone(),
                            move |mut caller, inputs, outputs| {
                                let utxo_id =
                                    UtxoId::from_wasm(&inputs[0], caller.as_context()).unwrap();
                                caller.as_context_mut().data_mut().utxos[utxo_id.id].query(
                                    &name,
                                    &inputs[1..],
                                    outputs,
                                );
                                Ok(())
                            },
                        )
                        .unwrap();
                } else if import.name().starts_with("starstream_new_") {
                    let utxo_code = universe.load_debug(rest); // TODO: lazy-load
                    let coordination_code = coordination_code.clone();
                    linker
                        .func_new(
                            import.module(),
                            import.name(),
                            func_ty.clone(),
                            move |mut caller, inputs, outputs| {
                                eprintln!("NEW {name:?} {inputs:?}");
                                let utxo = Utxo::start(
                                    coordination_code.clone(),
                                    utxo_code.clone(),
                                    name.clone(),
                                    inputs,
                                );
                                let mut store = caller.as_context_mut();
                                let local_utxos = &mut store.data_mut().utxos;
                                let id = local_utxos.len();
                                local_utxos.push(utxo);
                                outputs[0] = UtxoId { id }.to_wasm_u32(caller.as_context_mut());
                                Ok(())
                            },
                        )
                        .unwrap();
                } else if import.name().starts_with("starstream_query_") {
                    linker
                        .func_new(
                            import.module(),
                            import.name(),
                            func_ty.clone(),
                            move |mut caller, inputs, outputs| {
                                //eprintln!("inputs are {inputs:?}");
                                let utxo_id =
                                    UtxoId::from_wasm(&inputs[0], caller.as_context()).unwrap();
                                caller.as_context_mut().data_mut().utxos[utxo_id.id].query(
                                    &name,
                                    &inputs[1..],
                                    outputs,
                                );
                                Ok(())
                            },
                        )
                        .unwrap();
                } else if import.name().starts_with("starstream_mutate_") {
                    linker
                        .func_new(
                            import.module(),
                            import.name(),
                            func_ty.clone(),
                            move |mut caller, inputs, outputs| {
                                //eprintln!("inputs are {inputs:?}");
                                let utxo_id =
                                    UtxoId::from_wasm(&inputs[0], caller.as_context()).unwrap();
                                caller.as_context_mut().data_mut().utxos[utxo_id.id].mutate(
                                    &name,
                                    &inputs[1..],
                                    outputs,
                                );
                                Ok(())
                            },
                        )
                        .unwrap();
                } else if import.name().starts_with("starstream_consume_") {
                    linker
                        .func_new(
                            import.module(),
                            import.name(),
                            func_ty.clone(),
                            move |mut caller, inputs, outputs| {
                                eprintln!("inputs are {inputs:?}");
                                let utxo_id =
                                    UtxoId::from_wasm(&inputs[0], caller.as_context()).unwrap();
                                caller.as_context_mut().data_mut().utxos[utxo_id.id].consume(
                                    &name,
                                    &inputs[1..],
                                    outputs,
                                );
                                Ok(())
                            },
                        )
                        .unwrap();
                } else if import.name().starts_with("starstream_event_") {
                    fake_import(&mut linker, &import, "TODO starstream_event_");
                } else if import.name().starts_with("starstream_handle_") {
                    fake_import(&mut linker, &import, "TODO starstream_handle_");
                } else {
                    panic!("bad import {import:?}");
                }
            } else {
                panic!("bad import {import:?}");
            }
        } else {
            // Permit out-of-scope imports so a single .wasm module can be used as multiple things.
            fake_import(
                &mut linker,
                &import,
                "not available in Coordination context",
            );
        }
    }

    linker
}

// ----------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum ValueOrUtxo {
    Value(Value),
    Utxo(UtxoId),
}

impl From<Value> for ValueOrUtxo {
    fn from(value: Value) -> Self {
        ValueOrUtxo::Value(value)
    }
}

impl From<UtxoId> for ValueOrUtxo {
    fn from(value: UtxoId) -> Self {
        ValueOrUtxo::Utxo(value)
    }
}

#[derive(Default)]
struct Universe {
    engine: Engine,
    contract_code: HashMap<ContractCodeId, Arc<ContractCode>>,
    utxos: Vec<Utxo>,
}

impl Universe {
    // Cheap hack to get things working.
    fn load_debug_uncached(name: &str) -> Arc<ContractCode> {
        let path = format!("target/wasm32-unknown-unknown/debug/{name}.wasm");
        Arc::new(ContractCode::load(std::fs::read(path).unwrap()))
    }

    fn load_debug(&mut self, name: &str) -> Arc<ContractCode> {
        self.contract_code
            .entry(name.to_owned())
            .or_insert_with(|| {
                let path = format!("target/wasm32-unknown-unknown/debug/{name}.wasm");
                Arc::new(ContractCode::load(std::fs::read(path).unwrap()))
            })
            .clone()
    }

    fn run_transaction(
        &mut self,
        coordination_script: &Arc<ContractCode>,
        entry_point: &str,
        inputs: &[ValueOrUtxo],
    ) -> ValueOrUtxo {
        eprintln!("run_transaction({entry_point:?}, {inputs:?})");

        let linker =
            coordination_script_linker(&self.engine.clone(), self, coordination_script.clone());

        let mut store = Store::new(
            &self.engine,
            CoordinationScriptInstance {
                coordination_code: &coordination_script,
                utxos: &mut self.utxos,
                temporary_utxo_ids: Default::default(),
            },
        );

        // Turn ExternRefs into u32 UTXO refs
        let mut inputs2 = Vec::with_capacity(inputs.len());
        for value in inputs {
            inputs2.push(match value {
                ValueOrUtxo::Value(v) => v.clone(),
                ValueOrUtxo::Utxo(u) => u.to_wasm_u32(store.as_context_mut()),
            });
        }

        let instance = linker
            .instantiate(&mut store, &coordination_script.module(&self.engine))
            .unwrap()
            .ensure_no_start(&mut store)
            .unwrap();

        let mut outputs = [Value::from(ExternRef::null())];
        let main = instance.get_func(&mut store, entry_point).unwrap();
        let num_outputs = main.ty(&mut store).results().len();
        main.call(&mut store, &inputs2[..], &mut outputs[..num_outputs])
            .unwrap();
        //eprintln!("returned: {outputs:?}");

        if let Some(utxo_id) = UtxoId::from_wasm(&outputs[0], store.as_context()) {
            // TODO: collisions still technically possible here.
            // Should consider examining static types.
            ValueOrUtxo::Utxo(utxo_id)
        } else {
            ValueOrUtxo::Value(outputs[0].clone())
        }
    }
}

impl std::fmt::Debug for Universe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Universe")
            .field("utxos", &self.utxos)
            .finish()
    }
}

// ----------------------------------------------------------------------------

fn main() {
    let mut universe = Universe::default();
    dbg!(&universe);

    let example_contract = universe.load_debug("example_contract");
    let example_coordination = universe.load_debug("example_coordination");

    universe.run_transaction(&example_coordination, "produce", &[]);
    dbg!(&universe);

    let a = universe.run_transaction(&example_coordination, "star_mint", &[Value::I64(17).into()]);
    let b = universe.run_transaction(&example_contract, "star_mint", &[Value::I64(20).into()]);
    let c = universe.run_transaction(&example_contract, "star_combine", &[a, b]);
    universe.run_transaction(&example_contract, "star_split", &[c, Value::I64(5).into()]);
    dbg!(&universe);

    let nft_contract = universe.run_transaction(&example_coordination, "new_nft", &[]);
    universe.run_transaction(
        &example_contract,
        "star_nft_mint_to",
        &[nft_contract.clone() /* owner */],
    );
    universe.run_transaction(
        &example_contract,
        "star_nft_mint_count",
        &[nft_contract, /* owner, */ Value::I64(4).into()],
    );
    dbg!(&universe);
}
