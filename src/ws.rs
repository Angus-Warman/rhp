use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::Query;
use axum::extract::ws::{Message, WebSocket};
use axum::http::Uri;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::db::DbConn;
use crate::eval::call_value;
use crate::process::{Context, Method, process_socket_src};
use crate::value::{Env, Function, FunctionBody, Value};

const CHANNEL_CAPACITY: usize = 256;

// ---- Registry ----

#[derive(Debug)]
pub struct SocketRegistry {
    clients: Mutex<HashMap<Uuid, Arc<Mutex<ClientState>>>>,
}

#[derive(Debug)]
struct ClientInner {
    group: String,
    closing: bool,
    on_message: Option<Value>,
    on_close: Option<Value>,
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
        self.clients.lock().unwrap().insert(uuid, state);
        uuid
    }

    fn remove(&self, uuid: Uuid) {
        self.clients.lock().unwrap().remove(&uuid);
    }

    fn get(&self, uuid: Uuid) -> Option<Arc<Mutex<ClientState>>> {
        self.clients.lock().unwrap().get(&uuid).cloned()
    }

    fn members(&self, selection: &Selection, self_uuid: Uuid) -> Vec<Uuid> {
        let clients = self.clients.lock().unwrap();
        let self_group = clients
            .get(&self_uuid)
            .map(|s| s.lock().unwrap().inner.lock().unwrap().group.clone());

        let mut out = Vec::new();
        for (uuid, state) in clients.iter() {
            let state_guard = state.lock().unwrap();
            let inner = state_guard.inner.lock().unwrap();
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

// ---- Script-facing values ----

pub fn socket_value(socket: &SocketRef) -> Value {
    let registry = socket.registry.clone();
    let uuid = socket.uuid;

    let client = Value::Function(Function {
        params: vec![],
        body: FunctionBody::Native(Arc::new({
            let registry = registry.clone();
            move |_args| {
                let registry = registry.clone();
                Box::pin(async move { Ok(group_value(registry, uuid, Selection::This)) })
            }
        })),
        captured: Env::new_root(),
    });

    let peers = Value::Function(Function {
        params: vec![],
        body: FunctionBody::Native(Arc::new({
            let registry = registry.clone();
            move |_args| {
                let registry = registry.clone();
                Box::pin(async move { Ok(group_value(registry, uuid, Selection::Peers)) })
            }
        })),
        captured: Env::new_root(),
    });

    let everyone = Value::Function(Function {
        params: vec![],
        body: FunctionBody::Native(Arc::new({
            let registry = registry.clone();
            move |_args| {
                let registry = registry.clone();
                Box::pin(async move { Ok(group_value(registry, uuid, Selection::Everyone)) })
            }
        })),
        captured: Env::new_root(),
    });

    let group = Value::Function(Function {
        params: vec!["name".to_string()],
        body: FunctionBody::Native(Arc::new({
            let registry = registry.clone();
            move |args| {
                let registry = registry.clone();
                Box::pin(async move {
                    let name = match args.first() {
                        Some(Value::String(s)) => s.clone(),
                        _ => {
                            return Ok(Value::String(
                                "SOCKET.Group: expected a string bucket name".to_string(),
                            ));
                        }
                    };
                    Ok(group_value(registry, uuid, Selection::Group(name)))
                })
            }
        })),
        captured: Env::new_root(),
    });

    let join = Value::Function(Function {
        params: vec!["name".to_string()],
        body: FunctionBody::Native(Arc::new({
            let registry = registry.clone();
            move |args| {
                let registry = registry.clone();
                Box::pin(async move {
                    let name = match args.first() {
                        Some(Value::String(s)) => s.clone(),
                        _ => {
                            return Ok(Value::String(
                                "SOCKET.Join: expected a string bucket name".to_string(),
                            ));
                        }
                    };
                    if let Some(state) = registry.get(uuid) {
                        state.lock().unwrap().inner.lock().unwrap().group = name;
                    }
                    Ok(Value::Null)
                })
            }
        })),
        captured: Env::new_root(),
    });

    let mut map = HashMap::new();
    map.insert("Client".to_string(), client);
    map.insert("Peers".to_string(), peers);
    map.insert("Everyone".to_string(), everyone);
    map.insert("Group".to_string(), group);
    map.insert("Join".to_string(), join);
    Value::Object(Arc::new(Mutex::new(map)))
}

fn group_value(registry: Arc<SocketRegistry>, uuid: Uuid, selection: Selection) -> Value {
    let is_this = selection == Selection::This;
    let mut map = HashMap::new();

    let send = Value::Function(Function {
        params: vec!["value".to_string()],
        body: FunctionBody::Native(Arc::new({
            let registry = registry.clone();
            let selection = selection.clone();
            move |args| {
                let registry = registry.clone();
                let selection = selection.clone();
                Box::pin(async move {
                    let text = match args.first().and_then(value_to_text) {
                        Some(t) => t,
                        None => return Ok(Value::Null),
                    };
                    for member in registry.members(&selection, uuid) {
                        if let Some(state) = registry.get(member) {
                            let _ = state
                                .lock()
                                .unwrap()
                                .tx
                                .try_send(Message::Text(text.clone().into()));
                        }
                    }
                    Ok(Value::Null)
                })
            }
        })),
        captured: Env::new_root(),
    });

    let count = Value::Function(Function {
        params: vec![],
        body: FunctionBody::Native(Arc::new({
            let registry = registry.clone();
            let selection = selection.clone();
            move |_args| {
                let registry = registry.clone();
                let selection = selection.clone();
                Box::pin(async move {
                    Ok(Value::Number(
                        registry.members(&selection, uuid).len() as f64
                    ))
                })
            }
        })),
        captured: Env::new_root(),
    });

    let ids = Value::Function(Function {
        params: vec![],
        body: FunctionBody::Native(Arc::new({
            let registry = registry.clone();
            let selection = selection.clone();
            move |_args| {
                let registry = registry.clone();
                let selection = selection.clone();
                Box::pin(async move {
                    let values = registry
                        .members(&selection, uuid)
                        .into_iter()
                        .map(|id| Value::String(id.to_string()))
                        .collect();
                    Ok(Value::Array(Arc::new(Mutex::new(values))))
                })
            }
        })),
        captured: Env::new_root(),
    });

    let get = Value::Function(Function {
        params: vec!["id".to_string()],
        body: FunctionBody::Native(Arc::new({
            let registry = registry.clone();
            let selection = selection.clone();
            move |args| {
                let registry = registry.clone();
                let selection = selection.clone();
                Box::pin(async move {
                    let id = match args.first() {
                        Some(Value::String(s)) => match Uuid::parse_str(s) {
                            Ok(id) => id,
                            Err(_) => return Ok(Value::Null),
                        },
                        _ => return Ok(Value::Null),
                    };
                    if registry.members(&selection, uuid).contains(&id) {
                        Ok(client_value(registry, id))
                    } else {
                        Ok(Value::Null)
                    }
                })
            }
        })),
        captured: Env::new_root(),
    });

    map.insert("Send".to_string(), send);
    map.insert("Count".to_string(), count);
    map.insert("Get".to_string(), get);
    map.insert("Ids".to_string(), ids);

    if is_this {
        let id = Value::Function(Function {
            params: vec![],
            body: FunctionBody::Native(Arc::new(move |_args| {
                Box::pin(async move { Ok(Value::String(uuid.to_string())) })
            })),
            captured: Env::new_root(),
        });

        let on_message = Value::Function(Function {
            params: vec!["callback".to_string()],
            body: FunctionBody::Native(Arc::new({
                let registry = registry.clone();
                move |args| {
                    let registry = registry.clone();
                    Box::pin(async move {
                        match args.first() {
                            Some(Value::Function(_)) => {}
                            Some(other) => {
                                return Ok(Value::String(format!(
                                    "SOCKET.OnMessage: expected a function, got {}",
                                    other.type_name()
                                )));
                            }
                            None => {
                                return Ok(Value::String(
                                    "SOCKET.OnMessage: expected a function".to_string(),
                                ));
                            }
                        }
                        if let Some(state) = registry.get(uuid) {
                            state.lock().unwrap().inner.lock().unwrap().on_message =
                                args.first().cloned();
                        }
                        Ok(Value::Null)
                    })
                }
            })),
            captured: Env::new_root(),
        });

        let on_close = Value::Function(Function {
            params: vec!["callback".to_string()],
            body: FunctionBody::Native(Arc::new({
                let registry = registry.clone();
                move |args| {
                    let registry = registry.clone();
                    Box::pin(async move {
                        match args.first() {
                            Some(Value::Function(_)) => {}
                            Some(other) => {
                                return Ok(Value::String(format!(
                                    "SOCKET.OnClose: expected a function, got {}",
                                    other.type_name()
                                )));
                            }
                            None => {
                                return Ok(Value::String(
                                    "SOCKET.OnClose: expected a function".to_string(),
                                ));
                            }
                        }
                        if let Some(state) = registry.get(uuid) {
                            state.lock().unwrap().inner.lock().unwrap().on_close =
                                args.first().cloned();
                        }
                        Ok(Value::Null)
                    })
                }
            })),
            captured: Env::new_root(),
        });

        map.insert("Id".to_string(), id);
        map.insert("OnMessage".to_string(), on_message);
        map.insert("OnClose".to_string(), on_close);
    }

    Value::Object(Arc::new(Mutex::new(map)))
}

fn client_value(registry: Arc<SocketRegistry>, target: Uuid) -> Value {
    let send = Value::Function(Function {
        params: vec!["value".to_string()],
        body: FunctionBody::Native(Arc::new({
            let registry = registry.clone();
            move |args| {
                let registry = registry.clone();
                Box::pin(async move {
                    let text = match args.first().and_then(value_to_text) {
                        Some(t) => t,
                        None => return Ok(Value::Null),
                    };
                    if let Some(state) = registry.get(target) {
                        let _ = state
                            .lock()
                            .unwrap()
                            .tx
                            .try_send(Message::Text(text.into()));
                    }
                    Ok(Value::Null)
                })
            }
        })),
        captured: Env::new_root(),
    });

    let id = Value::Function(Function {
        params: vec![],
        body: FunctionBody::Native(Arc::new(move |_args| {
            Box::pin(async move { Ok(Value::String(target.to_string())) })
        })),
        captured: Env::new_root(),
    });

    let mut map = HashMap::new();
    map.insert("Send".to_string(), send);
    map.insert("Id".to_string(), id);
    Value::Object(Arc::new(Mutex::new(map)))
}

fn value_to_text(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Object(_) | Value::Array(_) => {
            serde_json::to_string(&crate::process::value_to_json(v)).ok()
        }
        other => Some(other.display()),
    }
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
            on_message: None,
            on_close: None,
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
        body: Value::Object(Arc::new(Mutex::new(HashMap::new()))),
        socket: Some(SocketRef {
            registry: registry.clone(),
            uuid,
            selection: Selection::This,
        }),
    };

    let first = process_socket_src(src, context, conn).await;
    if let Some(text) = first.as_ref().and_then(value_to_text)
        && socket.send(Message::Text(text.into())).await.is_err()
    {
        teardown(&registry, &state, uuid).await;
        return;
    }

    loop {
        tokio::select! {
            maybe = socket.recv() => {
                match maybe {
                    Some(Ok(Message::Text(text))) => {
                        fire_on_message(&state, vec![Value::String(text.to_string())]).await;
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        let text = String::from_utf8_lossy(&bytes).to_string();
                        fire_on_message(&state, vec![Value::String(text)]).await;
                    }
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
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

    teardown(&registry, &state, uuid).await;
}

async fn fire_on_message(state: &Arc<Mutex<ClientState>>, args: Vec<Value>) {
    let callback = {
        let state_guard = state.lock().unwrap();
        state_guard.inner.lock().unwrap().on_message.clone()
    };
    if let Some(callback) = callback {
        let _ = call_value(callback, args).await;
    }
}

async fn teardown(registry: &Arc<SocketRegistry>, state: &Arc<Mutex<ClientState>>, uuid: Uuid) {
    let callback = {
        let state_guard = state.lock().unwrap();
        let mut inner = state_guard.inner.lock().unwrap();
        inner.closing = true;
        inner.on_close.take()
    };
    if let Some(callback) = callback {
        let _ = call_value(callback, vec![]).await;
    }
    registry.remove(uuid);
}
