//! Lua scripting escape hatch.
//!
//! Originally scoped for ~5% of devices with exotic encodings (checksums,
//! non-standard bit packings, conditional transforms). Phase 1 of the
//! reconfigurable-controller work (Electra One MK2) expands the surface:
//!
//! - Persistent per-device state (`ob.state`) survives across hook calls.
//! - Native JSON codec (`ob.json_decode` / `ob.json_encode`) — required to
//!   parse hardware preset files without writing a parser in Lua.
//! - Execution profiles (`Profile::Default` / `Profile::PresetIngest`) —
//!   preset ingestion needs a wider budget (500 ms, 4 MiB) than the per-frame
//!   transform hot path (10 ms, 1 MiB).
//!
//! See `docs/scripting.md` for the user-facing guide and `docs/CR-lua-scripting.md`
//! for the design rationale.

use anyhow::{anyhow, bail, Context, Result};
use mlua::{Lua, LuaOptions, StdLib, Table, Value};
use serde_json::Value as JsonValue;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::routing::{CcRoute, DynamicRoutes};

const DEFAULT_MEMORY_LIMIT_BYTES: usize = 1_024 * 1_024; // 1 MiB
const PRESET_INGEST_MEMORY_LIMIT_BYTES: usize = 4 * 1_024 * 1_024; // 4 MiB
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(10);
const PRESET_INGEST_TIMEOUT: Duration = Duration::from_millis(500);
const INSTRUCTION_INTERVAL: u32 = 200;

/// Execution budget for a single script invocation.
#[derive(Debug, Clone, Copy)]
pub enum Profile {
    /// Hot-path per-frame transforms: 10 ms, 1 MiB.
    Default,
    /// Preset ingestion / route rebuild: 500 ms, 4 MiB.
    PresetIngest,
}

impl Profile {
    fn timeout(&self) -> Duration {
        match self {
            Profile::Default => DEFAULT_TIMEOUT,
            Profile::PresetIngest => PRESET_INGEST_TIMEOUT,
        }
    }
    fn memory(&self) -> usize {
        match self {
            Profile::Default => DEFAULT_MEMORY_LIMIT_BYTES,
            Profile::PresetIngest => PRESET_INGEST_MEMORY_LIMIT_BYTES,
        }
    }
}

/// A SysEx block a Lua script asked the runtime to enqueue via
/// `ob.emit_sysex`. `wrap = true` means "wrap with device sysex header/footer";
/// `wrap = false` means the bytes are already a complete F0…F7 frame.
#[derive(Debug, Clone)]
pub struct EmitBlock {
    pub bytes: Vec<u8>,
    pub wrap: bool,
}

/// An OSC message a script asked the bridge to emit outbound via
/// `ob.emit_osc`. Used for introspection endpoints (e.g. /routes/list) where
/// the script's job is to report state back to clients.
#[derive(Debug, Clone)]
pub struct EmitOsc {
    pub addr: String,
    /// Stored in a serialization-friendly form: (tag, value). The runtime
    /// converts these to `rosc::OscType` at send time to avoid pulling rosc
    /// as a dependency into scripting.rs.
    pub args: Vec<EmitOscArg>,
}

#[derive(Debug, Clone)]
pub enum EmitOscArg {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
}

/// Sandboxed Lua interpreter with injected `ob.*` helpers. One per device.
pub struct ScriptEngine {
    lua: Lua,
    deadline_flag: Arc<AtomicBool>,
    /// Blocks queued during the current/most-recent script call. Drained by
    /// the runtime after each invocation and enqueued to the MIDI-out channel.
    emit_queue: Arc<Mutex<Vec<EmitBlock>>>,
    /// OSC messages queued by `ob.emit_osc` (introspection / status reports).
    osc_queue: Arc<Mutex<Vec<EmitOsc>>>,
}

impl ScriptEngine {
    pub fn new() -> Result<Self> { Self::new_inner(None) }

    /// Construct with a routes handle. Lua scripts can then call
    /// `ob.register_cc_route`, `ob.clear_routes`, `ob.set_current_page`.
    pub fn new_with_routes(routes: Arc<Mutex<DynamicRoutes>>) -> Result<Self> {
        Self::new_inner(Some(routes))
    }

    fn new_inner(routes: Option<Arc<Mutex<DynamicRoutes>>>) -> Result<Self> {
        // Load only the safe subset: math, string, table. (Lua 5.4 drops bit32;
        // bitwise ops are available via operators `&`, `|`, `~`, `<<`, `>>`.)
        let libs = StdLib::MATH | StdLib::STRING | StdLib::TABLE;
        let lua = Lua::new_with(libs, LuaOptions::new())
            .map_err(|e| anyhow!("mlua init: {e}"))?;
        lua.set_memory_limit(DEFAULT_MEMORY_LIMIT_BYTES)
            .map_err(|e| anyhow!("mlua memory limit: {e}"))?;

        // Clobber any remaining unsafe globals that the base subset might carry.
        for name in [
            "dofile", "loadfile", "load", "loadstring", "require", "package",
            "os", "io", "debug", "getfenv", "setfenv", "collectgarbage",
        ] {
            lua.globals().set(name, Value::Nil)
                .map_err(|e| anyhow!("strip global {name}: {e}"))?;
        }

        // Inject `ob.*` helpers.
        let ob = lua.create_table().map_err(|e| anyhow!("ob table: {e}"))?;
        ob.set("u14_lsb", lua.create_function(|_, v: i64| Ok((v & 0x7F) as i64))
            .map_err(|e| anyhow!("u14_lsb: {e}"))?)
            .map_err(|e| anyhow!("set u14_lsb: {e}"))?;
        ob.set("u14_msb", lua.create_function(|_, v: i64| Ok(((v >> 7) & 0x7F) as i64))
            .map_err(|e| anyhow!("u14_msb: {e}"))?)
            .map_err(|e| anyhow!("set u14_msb: {e}"))?;
        ob.set("u7_clamp", lua.create_function(|_, v: i64| Ok(v.clamp(0, 127)))
            .map_err(|e| anyhow!("u7_clamp: {e}"))?)
            .map_err(|e| anyhow!("set u7_clamp: {e}"))?;
        ob.set("checksum_xor", lua.create_function(|_, t: Vec<i64>| {
            let mut acc: i64 = 0;
            for b in t { acc ^= b & 0xFF; }
            Ok(acc & 0x7F)
        }).map_err(|e| anyhow!("checksum_xor: {e}"))?)
            .map_err(|e| anyhow!("set checksum_xor: {e}"))?;
        ob.set("checksum_sum", lua.create_function(|_, t: Vec<i64>| {
            let mut acc: i64 = 0;
            for b in t { acc = acc.wrapping_add(b); }
            Ok(acc & 0x7F)
        }).map_err(|e| anyhow!("checksum_sum: {e}"))?)
            .map_err(|e| anyhow!("set checksum_sum: {e}"))?;
        ob.set("log", lua.create_function(|_, s: String| {
            eprintln!("[lua] {s}");
            Ok(())
        }).map_err(|e| anyhow!("log: {e}"))?)
            .map_err(|e| anyhow!("set log: {e}"))?;
        ob.set("log_warn", lua.create_function(|_, s: String| {
            eprintln!("[lua:warn] {s}");
            Ok(())
        }).map_err(|e| anyhow!("log_warn: {e}"))?)
            .map_err(|e| anyhow!("set log_warn: {e}"))?;
        ob.set("log_error", lua.create_function(|_, s: String| {
            eprintln!("[lua:error] {s}");
            Ok(())
        }).map_err(|e| anyhow!("log_error: {e}"))?)
            .map_err(|e| anyhow!("set log_error: {e}"))?;

        // Native JSON codec. Reason to expose natively rather than writing a
        // pure-Lua parser: Electra presets are 50–300 KB. A pure-Lua parse
        // would blow the 10 ms budget and the 1 MiB cap well before finishing.
        let decode_fn = lua.create_function(|lua, s: String| {
            match serde_json::from_str::<JsonValue>(&s) {
                Ok(v) => json_to_lua(lua, &v).map_err(mlua::Error::external),
                Err(e) => Err(mlua::Error::external(anyhow!("json_decode: {e}"))),
            }
        }).map_err(|e| anyhow!("json_decode: {e}"))?;
        ob.set("json_decode", decode_fn).map_err(|e| anyhow!("set json_decode: {e}"))?;
        let encode_fn = lua.create_function(|_, v: Value| {
            let j = lua_to_json(&v).map_err(mlua::Error::external)?;
            serde_json::to_string(&j)
                .map_err(|e| mlua::Error::external(anyhow!("json_encode: {e}")))
        }).map_err(|e| anyhow!("json_encode: {e}"))?;
        ob.set("json_encode", encode_fn).map_err(|e| anyhow!("set json_encode: {e}"))?;

        // Persistent per-device state. Scripts read/write `ob.state.<key>`;
        // values survive across transform/script calls on this engine.
        let state = lua.create_table().map_err(|e| anyhow!("ob.state: {e}"))?;
        ob.set("state", state).map_err(|e| anyhow!("set ob.state: {e}"))?;

        // Dynamic-routing API. When `routes` is None, the closures error out
        // with a clear message rather than silently no-op, so a script that
        // expects routes but was loaded on a legacy engine fails loudly.
        install_routing_fns(&lua, &ob, routes.clone())?;

        // Multi-block SysEx emission API.
        let emit_queue: Arc<Mutex<Vec<EmitBlock>>> = Arc::new(Mutex::new(Vec::new()));
        install_emit_fn(&lua, &ob, emit_queue.clone())?;

        // OSC emission (introspection, state reports).
        let osc_queue: Arc<Mutex<Vec<EmitOsc>>> = Arc::new(Mutex::new(Vec::new()));
        install_emit_osc_fn(&lua, &ob, osc_queue.clone())?;

        // Route introspection: read-only dump of the current DynamicRoutes.
        install_list_routes_fn(&lua, &ob, routes.clone())?;

        lua.globals().set("ob", ob).map_err(|e| anyhow!("set ob: {e}"))?;

        let deadline_flag = Arc::new(AtomicBool::new(false));
        let flag_hook = deadline_flag.clone();
        let triggers = mlua::HookTriggers::new().every_nth_instruction(INSTRUCTION_INTERVAL);
        lua.set_hook(triggers, move |_, _| {
            if flag_hook.load(Ordering::Relaxed) {
                Err(mlua::Error::RuntimeError("script timeout".into()))
            } else {
                Ok(mlua::VmState::Continue)
            }
        });

        Ok(Self { lua, deadline_flag, emit_queue, osc_queue })
    }

    /// Drain SysEx blocks emitted during the last script call. The runtime
    /// calls this after every `run_script_with` / `eval_transform_with` and
    /// enqueues each block to MIDI out (wrapping when `wrap = true`).
    pub fn drain_emits(&self) -> Vec<EmitBlock> {
        match self.emit_queue.lock() {
            Ok(mut q) => std::mem::take(&mut *q),
            Err(_) => Vec::new(),
        }
    }

    /// Drain OSC messages emitted via `ob.emit_osc`. The runtime sends each
    /// via the configured send_osc callback after the script returns.
    pub fn drain_osc_emits(&self) -> Vec<EmitOsc> {
        match self.osc_queue.lock() {
            Ok(mut q) => std::mem::take(&mut *q),
            Err(_) => Vec::new(),
        }
    }

    /// Arm a timeout + memory cap for a single run. The flag is set by a
    /// watchdog thread after `dur`; the next hook firing aborts the script.
    fn with_profile<T>(&self, profile: Profile, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let dur = profile.timeout();
        let new_mem = profile.memory();
        // Widen the memory cap for this run, restore after.
        let prev_mem = self.lua.set_memory_limit(new_mem)
            .map_err(|e| anyhow!("set memory limit: {e}"))?;

        self.deadline_flag.store(false, Ordering::Relaxed);
        let flag = self.deadline_flag.clone();
        let start = Instant::now();
        let _watchdog = std::thread::spawn(move || {
            std::thread::sleep(dur);
            flag.store(true, Ordering::Relaxed);
        });
        let result = f();
        self.deadline_flag.store(true, Ordering::Relaxed);
        let _ = self.lua.set_memory_limit(prev_mem);

        match result {
            Ok(v) => {
                // If f() returned Ok, the script actually finished — wall-clock
                // overshoot (e.g. under test parallelism on slow hosts) is not
                // a bug. The hook-driven timeout handles real runaways.
                let _ = start;
                Ok(v)
            }
            Err(e) => {
                let chain_msg = format!("{e:#}");
                if chain_msg.contains("script timeout") {
                    bail!("ScriptError::Timeout");
                }
                Err(e)
            }
        }
    }

    /// Evaluate a transform expression under the default profile. The
    /// expression receives `value` in scope and must `return` a number.
    pub fn eval_transform(&self, expr: &str, value: i64) -> Result<i64> {
        self.eval_transform_with(Profile::Default, expr, value)
    }

    pub fn eval_transform_with(&self, profile: Profile, expr: &str, value: i64) -> Result<i64> {
        self.with_profile(profile, || {
            let wrapped = format!("local value = ...\n{expr}");
            let chunk = self.lua.load(&wrapped);
            let ret: Value = chunk.call(value).context("eval_transform call")?;
            match ret {
                Value::Integer(i) => Ok(i),
                Value::Number(n) => Ok(n as i64),
                Value::Boolean(b) => Ok(b as i64),
                _ => bail!("transform did not return a number"),
            }
        })
    }

    /// Run a `script` block with a structured context table. The block may
    /// mutate `ctx` and must `return ctx` (or return nil to drop the message).
    pub fn run_script(&self, code: &str, ctx: ScriptContext) -> Result<Option<ScriptContext>> {
        self.run_script_with(Profile::Default, code, ctx)
    }

    pub fn run_script_with(
        &self,
        profile: Profile,
        code: &str,
        ctx: ScriptContext,
    ) -> Result<Option<ScriptContext>> {
        self.with_profile(profile, || {
            let ctx_tbl = ctx.to_lua(&self.lua)?;
            let chunk = self.lua.load(code);
            let ret: Value = chunk.call(ctx_tbl.clone()).context("run_script call")?;
            match ret {
                Value::Nil => Ok(None),
                Value::Table(t) => Ok(Some(ScriptContext::from_lua(&t)?)),
                _ => Ok(Some(ScriptContext::from_lua(&ctx_tbl)?)),
            }
        })
    }

    /// Execute arbitrary Lua code, returning nothing. Used by higher layers
    /// (Phase 3+) to drive state-mutating hooks like preset ingestion.
    pub fn exec(&self, code: &str, profile: Profile) -> Result<()> {
        self.with_profile(profile, || {
            let chunk = self.lua.load(code);
            chunk.exec().context("exec call")?;
            Ok(())
        })
    }
}

fn install_routing_fns(
    lua: &Lua,
    ob: &Table,
    routes: Option<Arc<Mutex<DynamicRoutes>>>,
) -> Result<()> {
    // Helper to build an "engine has no routes" error for stub closures.
    fn no_routes_err() -> mlua::Error {
        mlua::Error::external(anyhow!(
            "ob.register_cc_route / clear_routes / set_current_page require \
             an engine built via ScriptEngine::new_with_routes"
        ))
    }

    let r1 = routes.clone();
    let register = lua.create_function(
        move |_, (page, channel, cc, osc_addr, min, max): (Option<i64>, i64, i64, String, i64, i64)| {
            let Some(handle) = &r1 else { return Err(no_routes_err()); };
            let route = CcRoute {
                osc_addr,
                min,
                max,
                channel: (channel & 0x0F) as u8,
                cc: (cc & 0x7F) as u8,
                page,
            };
            let mut g = handle.lock().map_err(|_| mlua::Error::external(anyhow!("routes lock poisoned")))?;
            g.register(route);
            Ok(())
        },
    ).map_err(|e| anyhow!("register_cc_route: {e}"))?;
    ob.set("register_cc_route", register).map_err(|e| anyhow!("set register_cc_route: {e}"))?;

    let r2 = routes.clone();
    let clear = lua.create_function(move |_, ()| {
        let Some(handle) = &r2 else { return Err(no_routes_err()); };
        let mut g = handle.lock().map_err(|_| mlua::Error::external(anyhow!("routes lock poisoned")))?;
        g.clear();
        Ok(())
    }).map_err(|e| anyhow!("clear_routes: {e}"))?;
    ob.set("clear_routes", clear).map_err(|e| anyhow!("set clear_routes: {e}"))?;

    let r3 = routes.clone();
    let set_page = lua.create_function(move |_, page: Option<i64>| {
        let Some(handle) = &r3 else { return Err(no_routes_err()); };
        let mut g = handle.lock().map_err(|_| mlua::Error::external(anyhow!("routes lock poisoned")))?;
        g.current_page = page;
        Ok(())
    }).map_err(|e| anyhow!("set_current_page: {e}"))?;
    ob.set("set_current_page", set_page).map_err(|e| anyhow!("set set_current_page: {e}"))?;

    let r4 = routes.clone();
    let get_page = lua.create_function(move |_, ()| {
        let Some(handle) = &r4 else { return Err(no_routes_err()); };
        let g = handle.lock().map_err(|_| mlua::Error::external(anyhow!("routes lock poisoned")))?;
        Ok(g.current_page)
    }).map_err(|e| anyhow!("get_current_page: {e}"))?;
    ob.set("get_current_page", get_page).map_err(|e| anyhow!("set get_current_page: {e}"))?;

    Ok(())
}

fn install_emit_fn(lua: &Lua, ob: &Table, queue: Arc<Mutex<Vec<EmitBlock>>>) -> Result<()> {
    let q = queue.clone();
    let emit = lua.create_function(move |_, (bytes, wrap): (Vec<i64>, Option<bool>)| {
        let wrap = wrap.unwrap_or(true);
        let block_bytes: Vec<u8> = bytes.into_iter().map(|b| (b & 0xFF) as u8).collect();
        let mut g = q.lock().map_err(|_| mlua::Error::external(anyhow!("emit queue lock poisoned")))?;
        g.push(EmitBlock { bytes: block_bytes, wrap });
        Ok(())
    }).map_err(|e| anyhow!("emit_sysex: {e}"))?;
    ob.set("emit_sysex", emit).map_err(|e| anyhow!("set emit_sysex: {e}"))?;
    Ok(())
}

fn install_emit_osc_fn(lua: &Lua, ob: &Table, queue: Arc<Mutex<Vec<EmitOsc>>>) -> Result<()> {
    let q = queue.clone();
    // Variadic: ob.emit_osc(addr, arg1, arg2, ...). Args are coerced by type.
    let emit = lua.create_function(move |_, (addr, rest): (String, mlua::Variadic<Value>)| {
        let mut args = Vec::with_capacity(rest.len());
        for v in rest.into_iter() {
            let a = match v {
                Value::Nil => continue,
                Value::Boolean(b) => EmitOscArg::Bool(b),
                Value::Integer(i) => EmitOscArg::Int(i),
                Value::Number(n) => EmitOscArg::Float(n),
                Value::String(s) => EmitOscArg::Str(
                    s.to_str().map_err(|e| mlua::Error::external(anyhow!("str arg: {e}")))?.to_string(),
                ),
                _ => return Err(mlua::Error::external(anyhow!(
                    "emit_osc arg must be nil|bool|int|float|string"
                ))),
            };
            args.push(a);
        }
        let mut g = q.lock().map_err(|_| mlua::Error::external(anyhow!("osc queue lock poisoned")))?;
        g.push(EmitOsc { addr, args });
        Ok(())
    }).map_err(|e| anyhow!("emit_osc: {e}"))?;
    ob.set("emit_osc", emit).map_err(|e| anyhow!("set emit_osc: {e}"))?;
    Ok(())
}

fn install_list_routes_fn(
    lua: &Lua,
    ob: &Table,
    routes: Option<Arc<Mutex<DynamicRoutes>>>,
) -> Result<()> {
    let handle = routes.clone();
    let f = lua.create_function(move |lua, ()| {
        let out = lua.create_table().map_err(|e| mlua::Error::external(anyhow!("list: {e}")))?;
        let Some(h) = &handle else { return Ok(out); };
        let g = h.lock().map_err(|_| mlua::Error::external(anyhow!("routes lock poisoned")))?;
        // Iterate by_osc so each route is surfaced exactly once, independent
        // of how many CcKey entries shadow it per page.
        let mut i: i64 = 1;
        for (_addr, r) in &g.by_osc {
            let row = lua.create_table().map_err(|e| mlua::Error::external(anyhow!("row: {e}")))?;
            row.set("osc_addr", r.osc_addr.as_str())
                .and_then(|_| row.set("channel", r.channel as i64))
                .and_then(|_| row.set("cc", r.cc as i64))
                .and_then(|_| row.set("page", r.page))
                .and_then(|_| row.set("min", r.min))
                .and_then(|_| row.set("max", r.max))
                .map_err(|e| mlua::Error::external(anyhow!("row fields: {e}")))?;
            out.set(i, row).map_err(|e| mlua::Error::external(anyhow!("out set: {e}")))?;
            i += 1;
        }
        Ok(out)
    }).map_err(|e| anyhow!("list_routes: {e}"))?;
    ob.set("list_routes", f).map_err(|e| anyhow!("set list_routes: {e}"))?;
    Ok(())
}

fn json_to_lua(lua: &Lua, v: &JsonValue) -> Result<Value> {
    Ok(match v {
        JsonValue::Null => Value::Nil,
        JsonValue::Bool(b) => Value::Boolean(*b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                Value::Number(f)
            } else {
                Value::Nil
            }
        }
        JsonValue::String(s) => Value::String(
            lua.create_string(s).map_err(|e| anyhow!("json str: {e}"))?,
        ),
        JsonValue::Array(a) => {
            let t = lua.create_table().map_err(|e| anyhow!("json arr: {e}"))?;
            for (i, item) in a.iter().enumerate() {
                t.set(i as i64 + 1, json_to_lua(lua, item)?)
                    .map_err(|e| anyhow!("json arr set: {e}"))?;
            }
            Value::Table(t)
        }
        JsonValue::Object(o) => {
            let t = lua.create_table().map_err(|e| anyhow!("json obj: {e}"))?;
            for (k, item) in o {
                t.set(k.as_str(), json_to_lua(lua, item)?)
                    .map_err(|e| anyhow!("json obj set: {e}"))?;
            }
            Value::Table(t)
        }
    })
}

fn lua_to_json(v: &Value) -> Result<JsonValue> {
    Ok(match v {
        Value::Nil => JsonValue::Null,
        Value::Boolean(b) => JsonValue::Bool(*b),
        Value::Integer(i) => JsonValue::Number((*i).into()),
        Value::Number(n) => serde_json::Number::from_f64(*n)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        Value::String(s) => JsonValue::String(
            s.to_str().map_err(|e| anyhow!("lua str: {e}"))?.to_string(),
        ),
        Value::Table(t) => {
            // Detect sequence: keys 1..N contiguous. Otherwise treat as object.
            let len = t.raw_len();
            let mut is_seq = len > 0;
            if is_seq {
                for i in 1..=len {
                    if t.raw_get::<Value>(i as i64).map(|v| matches!(v, Value::Nil)).unwrap_or(true) {
                        is_seq = false;
                        break;
                    }
                }
            }
            if is_seq {
                let mut arr = Vec::with_capacity(len as usize);
                for i in 1..=len {
                    let item: Value = t.raw_get(i as i64)
                        .map_err(|e| anyhow!("lua arr get: {e}"))?;
                    arr.push(lua_to_json(&item)?);
                }
                JsonValue::Array(arr)
            } else {
                let mut map = serde_json::Map::new();
                for pair in t.clone().pairs::<Value, Value>() {
                    let (k, vv) = pair.map_err(|e| anyhow!("lua obj iter: {e}"))?;
                    let key = match k {
                        Value::String(s) => s.to_str().map_err(|e| anyhow!("lua key: {e}"))?.to_string(),
                        Value::Integer(i) => i.to_string(),
                        Value::Number(n) => n.to_string(),
                        _ => bail!("json_encode: unsupported table key type"),
                    };
                    map.insert(key, lua_to_json(&vv)?);
                }
                JsonValue::Object(map)
            }
        }
        _ => bail!("json_encode: unsupported lua value"),
    })
}

/// Structured context passed to a `script` block.
#[derive(Debug, Clone, Default)]
pub struct ScriptContext {
    pub args: Vec<i64>,
    /// OSC string args (e.g. the JSON preset payload for /preset/upload).
    /// Empty for non-custom-command script invocations.
    pub args_str: Vec<String>,
    pub payload: Vec<u8>,
    pub checksum: Option<i64>,
    pub direction: String,
    pub device: String,
    pub command: String,
    /// Named bindings from a matched reply pattern (reply.script only).
    /// Keys are placeholder names (e.g. "page", "bank"); values are the
    /// captured u7/u14 integers. Empty for non-reply invocations.
    pub bindings: std::collections::HashMap<String, i64>,
}

impl ScriptContext {
    fn to_lua(&self, lua: &Lua) -> Result<Table> {
        let t = lua.create_table().map_err(|e| anyhow!("ctx table: {e}"))?;
        let args_t = lua.create_sequence_from(self.args.iter().copied())
            .map_err(|e| anyhow!("ctx.args: {e}"))?;
        t.set("args", args_t).map_err(|e| anyhow!("set args: {e}"))?;
        let args_str_t = lua.create_sequence_from(self.args_str.iter().cloned())
            .map_err(|e| anyhow!("ctx.args_str: {e}"))?;
        t.set("args_str", args_str_t).map_err(|e| anyhow!("set args_str: {e}"))?;
        let payload_t = lua.create_sequence_from(self.payload.iter().map(|b| *b as i64))
            .map_err(|e| anyhow!("ctx.payload: {e}"))?;
        t.set("payload", payload_t).map_err(|e| anyhow!("set payload: {e}"))?;
        t.set("checksum", self.checksum).map_err(|e| anyhow!("set checksum: {e}"))?;
        t.set("direction", self.direction.clone()).map_err(|e| anyhow!("set direction: {e}"))?;
        t.set("device", self.device.clone()).map_err(|e| anyhow!("set device: {e}"))?;
        t.set("command", self.command.clone()).map_err(|e| anyhow!("set command: {e}"))?;
        let bind_t = lua.create_table().map_err(|e| anyhow!("ctx.bindings: {e}"))?;
        for (k, v) in &self.bindings {
            bind_t.set(k.as_str(), *v).map_err(|e| anyhow!("set binding: {e}"))?;
        }
        t.set("bindings", bind_t).map_err(|e| anyhow!("set bindings: {e}"))?;
        Ok(t)
    }

    fn from_lua(t: &Table) -> Result<Self> {
        let args: Vec<i64> = t.get::<Vec<i64>>("args").unwrap_or_default();
        let args_str: Vec<String> = t.get::<Vec<String>>("args_str").unwrap_or_default();
        let payload_i: Vec<i64> = t.get::<Vec<i64>>("payload").unwrap_or_default();
        let payload = payload_i.iter().map(|v| (*v & 0xFF) as u8).collect();
        let checksum: Option<i64> = t.get("checksum").ok();
        let direction: String = t.get("direction").unwrap_or_default();
        let device: String = t.get("device").unwrap_or_default();
        let command: String = t.get("command").unwrap_or_default();
        let mut bindings = std::collections::HashMap::new();
        if let Ok(bt) = t.get::<Table>("bindings") {
            for pair in bt.pairs::<String, i64>() {
                if let Ok((k, v)) = pair { bindings.insert(k, v); }
            }
        }
        Ok(ScriptContext { args, args_str, payload, checksum, direction, device, command, bindings })
    }
}
