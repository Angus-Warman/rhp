use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::Query;
use axum::extract::ws::{Message, WebSocket};
use axum::http::Uri;
use rquickjs::function::Rest;
use rquickjs::prelude::{FromJs, IntoJs};
use rquickjs::promise::MaybePromise;
use rquickjs::{Array, Ctx, Function, Object, Value};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::db::DbConn;
use crate::process::{Context, Method, split_src};
use crate::quickjs::Engine;

const CHANNEL_CAPACITY: usize = 256;

/// Globals in the engine's runtime that hold the per-connection callbacks.
///
/// Handlers are stored *in JS* (as globals on the engine's own context) instead
/// of as `Persistent` handles in Rust state. The Runtime alone owns and frees
/// them, so no JS value can outlive the engine that created it, dropping the
/// `Engine` at any point (including task cancellation) frees everything cleanly.
const ON_MESSAGE_SLOT: &str = "__rhp_on_message";
const ON_CLOSE_SLOT: &str = "__rhp_on_close";

// ---- Registry ----

#[derive(Debug)]
pub struct SocketRegistry {
    clients: Mutex<HashMap<Uuid, Arc<Mutex<ClientState>>>>,
}

#[derive(Debug)]
struct ClientInner {
    group: String,
    closing: bool,
}

#[derive(Debug)]
pub struct ClientState {
    inner: Mutex<ClientInner>,
    tx: mpsc::Sender<Message>,
}

impl SocketRegistry {
    pub fn new() -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
        }
    }

    fn register(&self, state: Arc<Mutex<ClientState>>) -> Uuid {
        let uuid = Uuid::new_v4();
        self.clients
            .lock()
            .expect("lock poisoned")
            .insert(uuid, state);
        uuid
    }

    fn remove(&self, uuid: Uuid) {
        self.clients.lock().expect("lock poisoned").remove(&uuid);
    }

    fn get(&self, uuid: Uuid) -> Option<Arc<Mutex<ClientState>>> {
        self.clients
            .lock()
            .expect("lock poisoned")
            .get(&uuid)
            .cloned()
    }

    fn members(&self, selection: &Selection, self_uuid: Uuid) -> Vec<Uuid> {
        let clients = self.clients.lock().expect("lock poisoned");
        let self_group = clients.get(&self_uuid).map(|s| {
            s.lock()
                .expect("lock poisoned")
                .inner
                .lock()
                .expect("lock poisoned")
                .group
                .clone()
        });

        let mut out = Vec::new();
        for (uuid, state) in clients.iter() {
            let state_guard = state.lock().expect("lock poisoned");
            let inner = state_guard.inner.lock().expect("lock poisoned");
            if inner.closing {
                continue;
            }
            let include = match selection {
                Selection::This => *uuid == self_uuid,
                Selection::Peers => *uuid != self_uuid && Some(&inner.group) == self_group.as_ref(),
                Selection::Everyone => Some(&inner.group) == self_group.as_ref(),
                Selection::Group(name) => *uuid != self_uuid && &inner.group == name,
            };
            if include {
                out.push(*uuid);
            }
        }
        out
    }
}

impl Default for SocketRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---- Selections ----

#[derive(Debug, Clone, PartialEq)]
pub enum Selection {
    This,
    Peers,
    Everyone,
    Group(String),
}

// ---- SocketRef (carried in Context) ----

#[derive(Debug, Clone)]
pub struct SocketRef {
    pub registry: Arc<SocketRegistry>,
    pub uuid: Uuid,
    pub selection: Selection,
}

impl PartialEq for SocketRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.registry, &other.registry)
            && self.uuid == other.uuid
            && self.selection == other.selection
    }
}

// ---- Script-facing SOCKET globals ----

/// Register the `SOCKET` global into a script engine's context.
pub fn register_socket<'js>(ctx: &Ctx<'js>, socket: &SocketRef) -> rquickjs::Result<()> {
    let registry = socket.registry.clone();
    let uuid = socket.uuid;

    let r1 = registry.clone();
    let client = Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
        group_object(&ctx, &r1, uuid, Selection::This).map(Value::from)
    })?;
    let r2 = registry.clone();
    let peers = Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
        group_object(&ctx, &r2, uuid, Selection::Peers).map(Value::from)
    })?;
    let r3 = registry.clone();
    let everyone = Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
        group_object(&ctx, &r3, uuid, Selection::Everyone).map(Value::from)
    })?;
    let r4 = registry.clone();
    let group = Function::new(ctx.clone(), move |ctx: Ctx<'js>, name: String| {
        group_object(&ctx, &r4, uuid, Selection::Group(name)).map(Value::from)
    })?;
    let r5 = registry.clone();
    let join = Function::new(ctx.clone(), move |ctx: Ctx<'js>, name: String| {
        if let Some(state) = r5.get(uuid) {
            state
                .lock()
                .expect("lock poisoned")
                .inner
                .lock()
                .expect("lock poisoned")
                .group = name;
        }
        Ok::<rquickjs::Value, rquickjs::Error>(Value::new_null(ctx.clone()))
    })?;
    let r6 = registry.clone();
    let socket_obj = Object::new(ctx.clone())?;
    socket_obj.set("Client", client)?;
    socket_obj.set("Peers", peers)?;
    socket_obj.set("Everyone", everyone)?;
    socket_obj.set("Group", group)?;
    socket_obj.set("Join", join)?;

    merge_client_object(ctx, &socket_obj, &r6, uuid, Selection::This, true)?;

    ctx.globals().set("SOCKET", socket_obj)?;
    Ok(())
}

fn group_object<'js>(
    ctx: &Ctx<'js>,
    registry: &Arc<SocketRegistry>,
    uuid: Uuid,
    selection: Selection,
) -> rquickjs::Result<Object<'js>> {
    let is_this = selection == Selection::This;
    let obj = Object::new(ctx.clone())?;
    merge_client_object(ctx, &obj, registry, uuid, selection, is_this)?;
    Ok(obj)
}

/// Populate a client-group object with Send/Count/Get/Ids (plus Id/OnMessage/
/// OnClose for the group containing the current client).
fn merge_client_object<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    registry: &Arc<SocketRegistry>,
    uuid: Uuid,
    selection: Selection,
    is_this: bool,
) -> rquickjs::Result<()> {
    let reg0 = registry.clone();
    let sel = selection.clone();
    let send = Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>, values: Rest<Value<'js>>| {
            if let Some(text) = values.first() {
                let text = stringify_value(&ctx, text)?;
                for member in reg0.members(&sel, uuid) {
                    if let Some(state) = reg0.get(member) {
                        let _ = state
                            .lock()
                            .expect("lock poisoned")
                            .tx
                            .try_send(Message::Text(text.clone().into()));
                    }
                }
            }
            Ok::<rquickjs::Value, rquickjs::Error>(Value::new_null(ctx.clone()))
        },
    )?;
    obj.set("Send", send)?;

    let reg1 = registry.clone();
    let sel = selection.clone();
    let count = Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
        let n = reg1.members(&sel, uuid).len() as f64;
        n.into_js(&ctx)
    })?;
    obj.set("Count", count)?;

    let reg2 = registry.clone();
    let sel = selection.clone();
    let ids = Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
        let arr = Array::new(ctx.clone())?;
        for id in reg2.members(&sel, uuid) {
            let idx = arr.len();
            arr.set(idx, id.to_string().into_js(&ctx)?)?;
        }
        Ok::<rquickjs::Value, rquickjs::Error>(Value::from(arr))
    })?;
    obj.set("Ids", ids)?;

    let reg3 = registry.clone();
    let sel = selection.clone();
    let get = Function::new(ctx.clone(), move |ctx: Ctx<'js>, id: String| {
        let target = match Uuid::parse_str(&id) {
            Ok(t) => t,
            Err(_) => return Ok::<rquickjs::Value, rquickjs::Error>(Value::new_null(ctx.clone())),
        };
        if reg3.members(&sel, uuid).contains(&target) {
            client_object(&ctx, &reg3, target).map(Value::from)
        } else {
            Ok::<rquickjs::Value, rquickjs::Error>(Value::new_null(ctx.clone()))
        }
    })?;
    obj.set("Get", get)?;

    if is_this {
        let id = Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
            uuid.to_string().into_js(&ctx)
        })?;
        obj.set("Id", id)?;

        let on_message = Function::new(ctx.clone(), move |ctx: Ctx<'js>, cb: Value<'js>| {
            ctx.globals().set(ON_MESSAGE_SLOT, cb)?;
            Ok::<rquickjs::Value, rquickjs::Error>(Value::new_null(ctx.clone()))
        })?;
        obj.set("OnMessage", on_message)?;

        let on_close = Function::new(ctx.clone(), move |ctx: Ctx<'js>, cb: Value<'js>| {
            ctx.globals().set(ON_CLOSE_SLOT, cb)?;
            Ok::<rquickjs::Value, rquickjs::Error>(Value::new_null(ctx.clone()))
        })?;
        obj.set("OnClose", on_close)?;
    }

    Ok(())
}

fn client_object<'js>(
    ctx: &Ctx<'js>,
    registry: &Arc<SocketRegistry>,
    target: Uuid,
) -> rquickjs::Result<Object<'js>> {
    let registry = registry.clone();
    let obj = Object::new(ctx.clone())?;
    let send = Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>, values: Rest<Value<'js>>| {
            if let Some(text) = values.first() {
                let text = stringify_value(&ctx, text)?;
                if let Some(state) = registry.get(target) {
                    let _ = state
                        .lock()
                        .expect("lock poisoned")
                        .tx
                        .try_send(Message::Text(text.into()));
                }
            }
            Ok::<rquickjs::Value, rquickjs::Error>(Value::new_null(ctx.clone()))
        },
    )?;
    obj.set("Send", send)?;
    let id = Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
        target.to_string().into_js(&ctx)
    })?;
    obj.set("Id", id)?;
    Ok(obj)
}

/// Produce a string form of any JS value to send to clients: strings verbatim,
/// objects/arrays as JSON, and other scalars via `String()` semantics.
fn stringify_value<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> rquickjs::Result<String> {
    if v.is_object() || v.is_array() {
        let json: Object = ctx.globals().get("JSON")?;
        let stringify: Function = json.get("stringify")?;
        let s: String = stringify.call((v.clone(),))?;
        return Ok(s);
    }
    let f: Function = ctx.globals().get("String")?;
    let s: String = f.call((v.clone(),))?;
    Ok(s)
}

// ---- Connection lifecycle ----

pub async fn run_socket(
    socket: WebSocket,
    uri: Uri,
    path: PathBuf,
    conn: DbConn,
    registry: Arc<SocketRegistry>,
) {
    let mut socket = socket;

    let src = match tokio::fs::read_to_string(&path).await {
        Ok(src) => src,
        Err(_) => {
            let _ = socket
                .send(Message::Text("not found".to_string().into()))
                .await;
            return;
        }
    };

    let (tx, mut rx) = mpsc::channel(CHANNEL_CAPACITY);
    let state = Arc::new(Mutex::new(ClientState {
        inner: Mutex::new(ClientInner {
            group: String::new(),
            closing: false,
        }),
        tx,
    }));
    let uuid = registry.register(state.clone());

    let query = Query::<HashMap<String, String>>::try_from_uri(&uri)
        .map(|q| q.0)
        .unwrap_or_default();
    let context = Context {
        method: Method::Socket,
        query,
        headers: HashMap::new(),
        cookies: HashMap::new(),
        body: serde_json::Value::Object(serde_json::Map::new()),
        socket: Some(SocketRef {
            registry: registry.clone(),
            uuid,
            selection: Selection::This,
        }),
    };

    let engine = match Engine::new(conn).await {
        Ok(e) => e,
        Err(_) => {
            teardown(&registry, &state, uuid, None).await;
            return;
        }
    };
    if engine.setup(&context).await.is_err() {
        teardown(&registry, &state, uuid, Some(&engine)).await;
        return;
    }

    run_socket_sections(&engine, &src, &context).await;

    loop {
        tokio::select! {
            maybe = socket.recv() => {
                match maybe {
                    Some(Ok(Message::Text(text))) => {
                        fire_on_message(&engine, text.to_string()).await;
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        let text = String::from_utf8_lossy(&bytes).to_string();
                        fire_on_message(&engine, text).await;
                    }
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                        break;
                    }
                }
            }
            msg = rx.recv() => {
                match msg {
                    Some(message) => {
                        if socket.send(message).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }

    teardown(&registry, &state, uuid, Some(&engine)).await;
}

/// Run all `SOCKET` sections for their side effects (registering OnMessage /
/// OnClose handlers, joining groups, etc.). Scripts that want to send an
/// initial message to the client should call `SOCKET.Client().Send(...)` explicitly.
async fn run_socket_sections(engine: &Engine, src: &str, context: &Context) {
    for section in split_src(src) {
        if let crate::process::Section::Code { code, method } = section
            && method.matches(&context.method)
        {
            let _ = engine.run_socket_section(&code).await;
        }
    }
}

/// Invoke the registered `OnMessage` handler for this connection.
///
/// The handler lives in the engine's runtime (the `__rhp_on_message` global),
/// so it is looked up and called in a `call_async` block. The handler may be
/// async; any returned Promise is awaited.
async fn fire_on_message(engine: &Engine, text: String) {
    let _ = engine
        .call_async(async |ctx| -> rquickjs::Result<()> {
            let cb: Value = ctx.globals().get(ON_MESSAGE_SLOT)?;
            if !cb.is_function() {
                return Ok(());
            }
            let cb = Function::from_js(&ctx, cb)?;
            let mut args = rquickjs::function::Args::new(ctx.clone(), 1);
            args.push_arg(text.into_js(&ctx)?)?;
            let result: Value = cb.call_arg(args)?;
            let _ = MaybePromise::from_value(result).into_future::<()>().await;
            Ok(())
        })
        .await;
}

/// Invoke the registered `OnClose` handler for this connection.
async fn fire_on_close(engine: &Engine) {
    let _ = engine
        .call_async(async |ctx| -> rquickjs::Result<()> {
            let cb: Value = ctx.globals().get(ON_CLOSE_SLOT)?;
            if !cb.is_function() {
                return Ok(());
            }
            let cb = Function::from_js(&ctx, cb)?;
            let args = rquickjs::function::Args::new(ctx.clone(), 0);
            let result: Value = cb.call_arg(args)?;
            let _ = MaybePromise::from_value(result).into_future::<()>().await;
            Ok(())
        })
        .await;
}

/// Close out a connection: mark it leaving (so it stops receiving broadcasts),
/// fire its `OnClose` handler inside its own runtime, then deregister it.
async fn teardown(
    registry: &Arc<SocketRegistry>,
    state: &Arc<Mutex<ClientState>>,
    uuid: Uuid,
    engine: Option<&Engine>,
) {
    state
        .lock()
        .expect("lock poisoned")
        .inner
        .lock()
        .expect("lock poisoned")
        .closing = true;

    if let Some(engine) = engine {
        fire_on_close(engine).await;
    }

    registry.remove(uuid);
}
