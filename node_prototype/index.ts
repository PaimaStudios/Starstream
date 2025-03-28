import binaryen from "binaryen";
import { readFile } from "fs/promises";
import { inspect } from "util";

interface MemoryExports {
  memory: WebAssembly.Memory;
}

interface IndirectFunctionTableExports {
  __indirect_function_table: WebAssembly.Table,
}

// ----------------------------------------------------------------------------
// Asyncify

interface AsyncifyExports {
  asyncify_get_state(): number;
  asyncify_start_unwind(addr: number): void;
  asyncify_stop_unwind(): void;
  asyncify_start_rewind(addr: number): void;
  asyncify_stop_rewind(): void;
}

enum AsyncifyState {
  NORMAL = 0,
  UNWIND = 1,
  REWIND = 2,
}

/** Where the unwind/rewind data structure will live. */
const STACK_START = 16;
const STACK_END = 1024;

function asyncify(blob: Uint8Array): Uint8Array {
  binaryen.setOptimizeLevel(4);
  binaryen.setPassArgument(
    "asyncify-imports",
    [
      `env.${UtxoEnv.prototype.starstream_yield.name}`,
      //`env.${UtxoInstance.utxoEnv.starstream_effect_my_effect.name}`,
    ].join(),
  );

  const ir = binaryen.readBinary(blob);
  // BulkMemory is called for by AssemblyScript; stuff blows up w/o something else.
  ir.setFeatures(binaryen.Features.All);
  ir.runPasses(["asyncify"]);
  return ir.emitBinary();
}

// ----------------------------------------------------------------------------

function fakeModule(message: string, items: Record<string, WebAssembly.ModuleImportDescriptor>): WebAssembly.ModuleImports {
  const r: WebAssembly.ModuleImports = {};
  for (const [k, v] of Object.entries(items)) {
    r[k] = () => {
      throw new Error(message);
    };
  }
  return r;
}

function collateImports(module: WebAssembly.Module) {
  const r: Record<string, Record<string, WebAssembly.ModuleImportDescriptor>> = {};
  for (const entry of WebAssembly.Module.imports(module)) {
    (r[entry.module] ??= {})[entry.name] = entry;
  }
  return Object.entries(r);
}

/** Generate a random integer in the range [1, 1 << 30]. */
function randomU32() {
  return Math.ceil((1 - Math.random()) * (1 << 30));
}

// ----------------------------------------------------------------------------

type ContractCodeId = string;

class ContractCode {
  readonly module: WebAssembly.Module;
  readonly hash: ArrayBufferLike;

  readonly #wasm: Uint8Array | null;

  private constructor(module: WebAssembly.Module, hash: ArrayBufferLike, wasm: Uint8Array | null) {
    this.module = module;
    this.hash = hash;
    this.#wasm = wasm;
  }

  static async load(wasm: Uint8Array): Promise<ContractCode> {
    return new ContractCode(
      new WebAssembly.Module(wasm),
      await crypto.subtle.digest("sha-256", wasm),
      wasm,
    );
  }

  asyncify(): ContractCode {
    if (this.#wasm) {
      return new ContractCode(
        new WebAssembly.Module(asyncify(this.#wasm)),
        this.hash,
        null,
      );
    } else {
      return this;
    }
  }
}

// ----------------------------------------------------------------------------

interface CoordinationContext {
  coordinationCode: ContractCode;
}

// TODO: needs to be asynclocal or something crazy?
let coordinationContext: CoordinationContext | null = null;
const effectHandlers = new Map<string, Function>();

/** Fulfiller of imports from `env` */
class StarstreamEnv {
  constructor(
    private readonly me: ContractInstance,
  ) {
    this.abort = this.abort.bind(this);
    this.starstream_log = this.starstream_log.bind(this);
    this.starstream_coordination_code = this.starstream_coordination_code.bind(this);
    this.starstream_this_code = this.starstream_this_code.bind(this);
  }

  abort() {
    throw new Error("abort() called");
  }

  starstream_log(...args: unknown[]) {
    console.log('starstream_log', ...args);
  }

  starstream_coordination_code(return_addr: number) {
    // TODO: this should probably be an effect?
    if (!coordinationContext) {
      throw new Error("bad context");
    }
    //new Uint8Array(this.me.memory.buffer, return_addr, 32).set(new Uint8Array(coordinationContext.coordinationCode.hash));
    new Uint8Array(this.me.memory.buffer, return_addr, 32).set(new Uint8Array(this.me.code.hash));
  }

  starstream_this_code(return_addr: number) {
    new Uint8Array(this.me.memory.buffer, return_addr, 32).set(new Uint8Array(this.me.code.hash));
  }
}

/** Fulfiller of imports from `starstream_utxo_env` */
class UtxoEnv {
  constructor(
    private readonly me: UtxoInstance,
  ) {
    this.starstream_yield = this.starstream_yield.bind(this);
  }

  starstream_yield(
    name: number,
    name_len: number,
    data: number,
    data_size: number,
    resume_arg: number,
    resume_arg_size: number,
  ) {
    const view = new Int32Array(this.me.exports.memory.buffer);
    if (this.me.exports.asyncify_get_state() == AsyncifyState.NORMAL) {
      this.me._setState({
        state: "yielded",
        yielded: {
          type_name: new Uint8Array(this.me.exports.memory.buffer, name, name_len),
          data: new Uint8Array(this.me.exports.memory.buffer, data, data_size),
          resume_arg: new Uint8Array(this.me.exports.memory.buffer, resume_arg, resume_arg_size),
        },
      });
      view[STACK_START >> 2] = STACK_START + 8;
      view[(STACK_START + 4) >> 2] = STACK_END;
      this.me.exports.asyncify_start_unwind(STACK_START);
    } else {
      this.me.exports.asyncify_stop_rewind();
    }
  }
}

/** Fulfiller of imports from `starstream_utxo:${addr}` */
class UtxoImport {
  [k: string]: Function;

  constructor(
    me: CoordinationScriptInstance,
    targetCodeId: ContractCodeId,
    want: Record<string, WebAssembly.ModuleImportDescriptor>,
  ) {
    for (const entry of Object.values(want)) {
      if (entry.kind === "function") {
        if (entry.name.startsWith("starstream_status_")) {
          this[entry.name] = (utxo_handle: number) => {
            return me.getUtxo(utxo_handle).isAlive();
          };
        } else if (entry.name.startsWith("starstream_resume_")) {
          this[entry.name] = (utxo_handle: number, resume_arg: number, resume_arg_size: number) => {
            const slice = new Uint8Array(me.memory.buffer).slice(resume_arg, resume_arg + resume_arg_size);
            me.getUtxo(utxo_handle).load().resume(slice);
          };
        } else if (entry.name.startsWith("starstream_new_")) {
          this[entry.name] = (...args: unknown[]) => {
            console.log('NEW', entry.name, args);
            const utxo = new Utxo(me.universe, targetCodeId, entry.name);
            utxo.load().start(...args);
            return me.setUtxo(utxo);
          };
        } else if (entry.name.startsWith("starstream_query_")) {
          // query = &self
          this[entry.name] = (utxo_handle: number, ...args: unknown[]) => {
            return me.getUtxo(utxo_handle).load().query(entry.name, ...args);
          };
        } else if (entry.name.startsWith("starstream_mutate_")) {
          // mutate = &mut self
          this[entry.name] = (utxo_handle: number, ...args: unknown[]) => {
            return me.getUtxo(utxo_handle).load().mutate(entry.name, ...args);
          };
        } else if (entry.name.startsWith("starstream_consume_")) {
          // consume = self
          this[entry.name] = (utxo_handle: number, ...args: unknown[]) => {
            return me.getUtxo(utxo_handle).load().consume(entry.name, ...args);
          };
        } else if (entry.name.startsWith("starstream_event_")) {
          this[entry.name] = (...args: unknown[]) => {
            console.log('EVENT', ...args);
          }
        } else if (entry.name.startsWith("starstream_handle_")) {
          this[entry.name] = (handler: number) => {
            console.log('HANDLER =', handler);
            if (handler == 0) {
              effectHandlers.delete(entry.name);
            } else {
              effectHandlers.set(entry.name, (...args: unknown[]) => {
                me.getFunctionPointer(handler)(...args);
              });
            }
          }
        } else {
          throw new Error("bad import " + JSON.stringify(entry));
        }
      } else {
        throw new Error("bad import " + JSON.stringify(entry));
      }
    }
  }
}

/** Fulfiller of imports from `starstream_token:${addr}` */
class TokenImport {
  [k: string]: Function;

  readonly #tokens = new Map<number, Token>();

  constructor(
    me: UtxoInstance,
    targetCodeId: ContractCodeId,
    want: Record<string, WebAssembly.ModuleImportDescriptor>,
  ) {
    for (const entry of Object.values(want)) {
      if (entry.kind === "function") {
        if (entry.name.startsWith("starstream_mint_")) {
          this[entry.name] = (...args: unknown[]) => {
            const handle = randomU32();
            const token = me.universe.tokenMint(targetCodeId, entry.name, args);
            this.#tokens.set(handle, token);
            me.utxo.tokens.add(token);
            return handle;
          };
        } else if (entry.name.startsWith("starstream_burn_")) {
          this[entry.name] = (handle: number) => {
            const token = this.#tokens.get(handle);
            if (!token) {
              throw new Error("bad token handle to burn");
            }
            const intermediate = token.burn(entry.name);
            me.utxo.tokens.delete(token);
            return intermediate;
          };
        } else {
          throw new Error("bad import " + JSON.stringify(entry));
        }
      } else {
        throw new Error("bad import " + JSON.stringify(entry));
      }
    }
  }
}

// ----------------------------------------------------------------------------

type ContractExports = MemoryExports & Partial<IndirectFunctionTableExports>;

class ContractInstance {
  readonly universe: Universe;
  readonly code: ContractCode;
  readonly wasm: WebAssembly.Instance;
  // exports
  readonly memory: WebAssembly.Memory;
  readonly exports: ContractExports;

  constructor(universe: Universe, code: ContractCode) {
    this.universe = universe;
    this.code = code;

    const imports: Record<string, object> = {};
    for (const [module, items] of collateImports(code.module)) {
      if (module === "env") {
        imports[module] = new StarstreamEnv(this);
      } else if (module === "starstream_utxo_env") {
        if (this instanceof UtxoInstance) {
          imports[module] = new UtxoEnv(this);
        } else {
          imports[module] = fakeModule("available in UTXO context only", items);
        }
      } else if (module.startsWith("starstream_utxo:")) {
        if (this instanceof CoordinationScriptInstance) {
          const id = module.substring("starstream_utxo:".length);
          imports[module] = new UtxoImport(this, id, items);
        } else {
          imports[module] = fakeModule("available in Coordination context only", items);
        }
      } else if (module.startsWith("starstream_token:")) {
        if (this instanceof UtxoInstance) {
          const id = module.substring("starstream_token:".length);
          imports[module] = new TokenImport(this, id, items);
        } else {
          imports[module] = fakeModule("available in UTXO context only", items);
        }
      }
    }

    this.wasm = new WebAssembly.Instance(code.module, imports as WebAssembly.Imports);
    this.exports = (this.wasm.exports as unknown as ContractExports);
    this.memory = this.exports.memory;
  }

  getFunction(name: string): Function {
    const f = this.wasm.exports[name];
    if (!f) {
      throw new Error(`getEntryPoint(${name}): export does not exist`);
    }
    if (typeof f !== 'function') {
      throw new Error(`getEntryPoint(${name}): not a function: ${f}`);
    }
    return f;
  }

  getFunctionPointer(index: number): Function {
    if (!this.exports.__indirect_function_table) {
      throw new Error("getFunctionPointer: no __indirect_function_table exported");
    }
    const f = this.exports.__indirect_function_table.get(index);
    if (!f) {
      throw new Error(`getFunctionPointer: no entry for ${index}`);
    }
    if (typeof f !== "function") {
      throw new Error(`getFunctionPointer: not a function: ${index} = ${f}`);
    }
    return f;
  }
}

// ----------------------------------------------------------------------------

/** A UTXO that has a WebAssembly instance currently in memory. */
class UtxoInstance extends ContractInstance {
  declare exports: typeof ContractInstance.prototype.exports & AsyncifyExports;

  #entryPoint: Function;
  #start_args: unknown[] | undefined;
  #state: {
    state: "not_started",
  } | {
    state: "yielded",
    yielded: {
      type_name: Uint8Array,
      data: Uint8Array,
      resume_arg: Uint8Array,
    },
  } | {
    state: "returned",
    value: unknown,
  } | {
    state: "errored",
    args: unknown[],
  } | {
    state: "effect",
    effect: string,
    args: unknown[],
  } | {
    state: "consumed",
  } = {
    state: "not_started"
  };

  constructor(
    public readonly utxo: Utxo,
    memory?: Uint8Array
  ) {
    super(utxo.universe, utxo.universe.getCodeSync(utxo.codeId).asyncify());

    if (memory) {
      // memcpy saved memory on top
      new Uint8Array(this.memory.buffer).set(memory);
    }

    this.#entryPoint = this.wasm.exports[utxo.entryPoint] as Function;
    if (typeof this.#entryPoint !== 'function') {
      throw new Error("bad UTXO entry point: " + utxo.entryPoint);
    }
  }

  _setState(s: any) {
    this.#state = s;
  }

  #raw_resume() {
    const returned = this.#entryPoint(...this.#start_args!);
    if (this.exports.asyncify_get_state() == AsyncifyState.NORMAL) {
      // Normal exit; it's spent.
      this.#state = {
        state: "returned",
        value: returned,
      }
      return false;
    }
    this.exports.asyncify_stop_unwind();
    return true;
  }

  start(...args: unknown[]): boolean {
    if (this.#state.state !== "not_started") {
      throw new Error("Cannot start() in state " + JSON.stringify(this.#state));
    }
    this.#start_args = args;
    return this.#raw_resume();
  }

  resume(resume_data?: Uint8Array): boolean {
    if (this.#state.state !== "yielded") {
      throw new Error("Cannot resume() in state " + JSON.stringify(this.#state));
    }
    if (this.#state.yielded.resume_arg.byteLength !== (resume_data?.byteLength ?? 0)) {
      throw new Error("resume_arg size mismatch");
    } else if (resume_data) {
      this.#state.yielded.resume_arg.set(resume_data);
    }
    this.exports.asyncify_start_rewind(STACK_START);
    return this.#raw_resume();
  }

  // &self
  query(name: string, ...args: unknown[]): unknown {
    if (this.#state.state !== "yielded") {
      throw new Error("Cannot query() in state " + JSON.stringify(this.#state));
    }
    // TODO: enforce asyncify_get_state is NORMAL after this call
    return (this.wasm.exports[name] as Function)(this.#state.yielded.data.byteOffset, ...args);
  }

  // &mut self
  mutate(name: string, ...args: unknown[]): unknown {
    if (this.#state.state !== "yielded") {
      throw new Error("Cannot mutate() in state " + JSON.stringify(this.#state));
    }
    // TODO: enforce asyncify_get_state is NORMAL after this call
    return (this.wasm.exports[name] as Function)(this.#state.yielded.data.byteOffset, ...args);
  }

  // self
  consume(name: string, ...args: unknown[]): unknown {
    if (this.#state.state !== "yielded") {
      throw new Error("Cannot consume() in state " + JSON.stringify(this.#state));
    }
    // TODO: enforce asyncify_get_state is NORMAL after this call
    const r = (this.wasm.exports[name] as Function)(this.#state.yielded.data.byteOffset, ...args);
    this.#state = { state: "consumed" };
    return r;
  }

  isAlive(): boolean {
    return this.#state.state !== "returned";
  }

  debug() {
    if (this.#state.state === "yielded") {
      const result: Record<string, any> = {};
      const name = new TextDecoder().decode(this.#state.yielded.type_name);
      result.__type = name;
      const last_part = name.split("::").pop();
      const prefix = `starstream_query_${last_part}_`;
      for (var key of Object.keys(this.wasm.exports)) {
        if (key.startsWith(prefix)) {
          try {
            result[key.substring(prefix.length)] = this.query(key);
          } catch (e) {
            result[key.substring(prefix.length)] = e;
          }
        }
      }
      return result;
    } else {
      return { state: this.#state.state };
    }
  }
}

// ----------------------------------------------------------------------------

class Utxo {
  readonly universe: Universe;
  readonly codeId: ContractCodeId;
  readonly entryPoint: string;
  #loaded?: UtxoInstance;

  readonly tokens = new Set<Token>();

  constructor(universe: Universe, codeId: ContractCodeId, entryPoint: string) {
    this.universe = universe;
    this.codeId = codeId;
    this.entryPoint = entryPoint;
    this.universe.resolveCode(this.codeId);
  }

  unload() {}

  load(): UtxoInstance {
    return (this.#loaded ??= new UtxoInstance(this));
  }

  isAlive(): boolean {
    // TODO: know state of UTXO without loading its code and memory
    return this.#loaded ? this.#loaded.isAlive() : false;
  }

  debug() {
    return Object.assign(
      this.#loaded ? this.#loaded.debug() : { unloaded: this.codeId },
      this.tokens.size > 0 ? { tokens: [...this.tokens].map(t => t.debug()) } : {},
    );
  }
}

// ----------------------------------------------------------------------------

class TokenInstance extends ContractInstance {
  constructor(universe: Universe, code: ContractCode) {
    super(universe, code);
  }
}

// ----------------------------------------------------------------------------

class Token {
  #burnFn: string;
  id: bigint;
  amount: bigint;

  constructor(private universe: Universe, private code: ContractCode, mintFn: string, mintArgs: unknown[]) {
    this.#burnFn = mintFn.replace(/^starstream_mint_/, "starstream_burn_");
    if (mintFn === this.#burnFn) {
      throw new Error(`bad mintFn: ${mintFn}`);
    }
    const returnAddr = 16;
    const instance = new TokenInstance(universe, code);
    instance.getFunction(mintFn)(returnAddr, ...mintArgs);
    [this.id, this.amount] = new BigUint64Array(instance.memory.buffer, returnAddr, 2);
  }

  burn(burnFn: string) {
    if (burnFn !== this.#burnFn) {
      throw new Error(`bad burnFn: ${burnFn}, expected: ${this.#burnFn}`);
    }
    return new TokenInstance(this.universe, this.code).getFunction(burnFn)(this.id, this.amount);
  }

  debug() {
    return {
      __type: this.#burnFn.replace(/^starstream_burn_/, ""),
      id: this.id,
      amount: this.amount,
    }
  }
}

// ----------------------------------------------------------------------------

class CoordinationScriptInstance extends ContractInstance {
  utxos = new Map<number, Utxo>();

  constructor(universe: Universe, code: ContractCode) {
    super(universe, code);
  }

  setUtxo(utxo: Utxo): number {
    const handle = randomU32();
    this.utxos.set(handle, utxo);
    return handle;
  }

  getUtxo(handle: number): Utxo {
    const utxo = this.utxos.get(handle);
    if (!utxo) {
      throw new Error(`Invalid UTXO handle: ${handle}; known: ${JSON.stringify(this.utxos)}`);
    }
    return utxo;
  }
}

// ----------------------------------------------------------------------------

class Universe {
  readonly contractCode = new Map<string, ContractCode>();
  readonly utxos = new Set<Utxo>();

  getCodeSync(hash: ContractCodeId): ContractCode {
    let code = this.contractCode.get(hash);
    if (!code) {
      throw new Error("blah");
    }
    return code;
  }

  async resolveCode(hash: ContractCodeId): Promise<ContractCode> {
    let code = this.contractCode.get(hash);
    if (!code) {
      const debugFname = `target/wasm32-unknown-unknown/debug/${hash}.wasm`;
      code = await ContractCode.load(await readFile(debugFname));
      this.contractCode.set(hash, code);
    }
    return code;
  }

  tokenMint(codeId: ContractCodeId, mintFn: string, mintArgs: unknown[]): Token {
    const code = this.getCodeSync(codeId);
    return new Token(this, code, mintFn, mintArgs);
  }

  runTransaction(coordinationScript: ContractCode, entryPoint: string, inputs: unknown[] = []) {
    // We aren't suspending this, we want to run it to completion always, so
    // we don't need to asyncify it.
    console.log('CALL', entryPoint, inputs);

    // Fulfill imports and instantiate WASM
    const instance = new CoordinationScriptInstance(this, coordinationScript);

    // Prepare inputs
    const inputs2 = [...inputs];
    for (let i = 0; i < inputs2.length; ++i) {
      const v = inputs2[i];
      if (v instanceof Utxo) {
        inputs2[i] = instance.setUtxo(v);
      }
    }

    coordinationContext = {
      coordinationCode: coordinationScript,
    };
    const result: unknown = instance.getFunction(entryPoint)(...inputs2);
    coordinationContext = null;
    console.log(' ->', result);
    // TODO: Rollback UTXO memories on error.

    // Update UTXO set
    for (const utxo of instance.utxos.values()) {
      if (utxo.isAlive()) {
        // TODO: Commit UTXO memories on success.
        this.utxos.add(utxo);
      } else {
        this.utxos.delete(utxo);
      }
    }

    if (typeof result === 'number' && instance.utxos.has(result)) {
      // TODO: What of collisions between ordinary numeric returns and random Utxo IDs?
      return instance.utxos.get(result);
    }
    return result;
  }

  debug() {
    return inspect([...this.utxos].map(u => u.debug()), { depth: null, colors: true });
  }
}

// ----------------------------------------------------------------------------

let n = 0;
const universe = new Universe();
console.log(++n, '--', universe.debug());

const exampleContract = await universe.resolveCode("example_contract");
const exampleCoordination = exampleContract;

console.log(++n, '--', universe.debug());

universe.runTransaction(
  exampleCoordination,
  "produce"
);
console.log(++n, '--', universe.debug());

/*
universe.runTransaction(
  exampleCoordination,
  "consume",
  [
    universe.utxos.values().next().value
  ]
);
console.log(++n, '--', universe, universe.utxos.values().next().value?.load().query("starstream_query_MyMain_get_supply"));
*/

const nft_contract = universe.runTransaction(
  exampleCoordination,
  "new_nft",
);
console.log(++n, '--', universe.debug());

/*
universe.runTransaction(
  exampleCoordination,
  "mint_seven_nfts",
  [nft_contract],
);
console.log(++n, '--', universe.debug());

universe.runTransaction(
  exampleCoordination,
  "mint_until_10_nfts",
  [nft_contract],
);
console.log(++n, '--', universe.debug());
*/

universe.runTransaction(
  exampleContract,
  "star_nft_mint_to",
  [nft_contract, /*owner,*/],
)
console.log(++n, '--', universe.debug());

universe.runTransaction(
  exampleContract,
  "star_nft_mint_count",
  [nft_contract, /*owner,*/ 4n],
)
console.log(++n, '--', universe.debug());
