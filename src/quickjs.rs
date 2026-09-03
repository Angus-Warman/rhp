use anyhow::{Result, anyhow};
use rquickjs::function::{Async, Rest};
use rquickjs::markers::ParallelSend;
use rquickjs::prelude::{IntoJs, Opt};
use rquickjs::{Array, AsyncContext, Ctx, Exception, Function, Object, Promise, Type, Value};
use std::ops::AsyncFnOnce;

use crate::db::{DbConn, ExecStmt, QueryStmt, TableStmt};
use crate::process::{Context, HttpResponseState};

/// Build a `rquickjs::Error` that throws a JS `Error` with the given message.
fn js_err<'js>(ctx: &Ctx<'js>, message: impl AsRef<str>) -> rquickjs::Error {
    match Exception::from_message(ctx.clone(), message.as_ref()) {
        Ok(exc) => ctx.throw(exc.into_object().into()),
        Err(e) => e,
    }
}

// ---- HTML escaping (mirrors the old escape_html) ----

fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            '/' => out.push_str("&#x2F;"),
            '`' => out.push_str("&grave;"),
            '=' => out.push_str("&#x3D;"),
            other => out.push(other),
        }
    }
    out
}

// ---- Value conversion: JS <-> serde_json ----

/// Append a value to a JS array (rquickjs has no `Array::push`).
fn push_array<'js>(arr: &Array<'js>, value: impl IntoJs<'js>) -> rquickjs::Result<()> {
    arr.set(arr.len(), value)?;
    Ok(())
}

fn console_log<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<Function<'js>> {
    Function::new(
        ctx.clone(),
        |ctx: Ctx<'js>, values: Rest<Value<'js>>| -> rquickjs::Result<()> {
            let parts: Vec<String> = values
                .iter()
                .map(|v| stringify_value(&ctx, v).unwrap_or_default())
                .collect();
            println!("{}", parts.join(" "));
            Ok(())
        },
    )
}

/// Convert a JS value to a serde_json value (limited to JSON-representable
/// types: null, bool, number, string, array, object).
pub fn js_to_json<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> rquickjs::Result<serde_json::Value> {
    match v.type_of() {
        Type::Null | Type::Undefined => Ok(serde_json::Value::Null),
        Type::Bool => Ok(serde_json::Value::Bool(v.as_bool().unwrap_or(false))),
        Type::Int => Ok(serde_json::Value::from(v.as_int().unwrap_or(0) as i64)),
        Type::Float => {
            let f = v.as_float().unwrap_or(0.0);
            if f.fract() == 0.0 && f.abs() < 1e15 {
                Ok(serde_json::Value::from(f as i64))
            } else {
                serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| js_err(ctx, format!("cannot represent number {f} as JSON")))
            }
        }
        Type::String => {
            let s = v
                .as_string()
                .ok_or_else(|| js_err(ctx, "string value missing"))?
                .to_string()?;
            Ok(serde_json::Value::String(s))
        }
        Type::Array => {
            let arr = Array::from_value(v.clone())?;
            let mut items = Vec::with_capacity(arr.len());
            for item in arr.iter::<Value<'js>>() {
                items.push(js_to_json(ctx, &item?)?);
            }
            Ok(serde_json::Value::Array(items))
        }
        Type::Object => {
            let obj = Object::from_value(v.clone())?;
            let mut map = serde_json::Map::new();
            for entry in obj.props::<String, Value<'js>>() {
                let (k, val) = entry?;
                map.insert(k, js_to_json(ctx, &val)?);
            }
            Ok(serde_json::Value::Object(map))
        }
        other => Err(js_err(
            ctx,
            format!("cannot convert JS {} to JSON", other.as_str()),
        )),
    }
}

/// Convert a serde_json value into a JS value.
fn json_to_js<'js>(ctx: &Ctx<'js>, value: &serde_json::Value) -> rquickjs::Result<Value<'js>> {
    match value {
        serde_json::Value::Null => Ok(Value::new_null(ctx.clone())),
        serde_json::Value::Bool(b) => Ok((*b).into_js(ctx)?),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok((i as f64).into_js(ctx)?)
            } else {
                Ok(n.as_f64().unwrap_or(0.0).into_js(ctx)?)
            }
        }
        serde_json::Value::String(s) => Ok(s.as_str().into_js(ctx)?),
        serde_json::Value::Array(items) => {
            let arr = Array::new(ctx.clone())?;
            for item in items {
                let v = json_to_js(ctx, item)?;
                push_array(&arr, v)?;
            }
            Ok(Value::from(arr))
        }
        serde_json::Value::Object(map) => {
            let obj = Object::new(ctx.clone())?;
            for (k, v) in map {
                let jv = json_to_js(ctx, v)?;
                obj.set(k.as_str(), jv)?;
            }
            Ok(Value::from(obj))
        }
    }
}

// ---- DB statement bridging ----

/// If `o` is a DB error object (`{ok: false, error: msg}`), treat it as a
/// thrown error.
fn check_error<'js>(
    ctx: &Ctx<'js>,
    o: &serde_json::Map<String, serde_json::Value>,
) -> rquickjs::Result<()> {
    if o.get("ok") == Some(&serde_json::Value::Bool(false)) {
        let msg = o
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("query failed");
        return Err(js_err(ctx, msg));
    }
    Ok(())
}

/// Build the JS object returned by `DB.Query(sql)`.
fn query_stmt_object<'js>(ctx: &Ctx<'js>, stmt: QueryStmt) -> rquickjs::Result<Object<'js>> {
    let obj = Object::new(ctx.clone())?;

    let all_stmt = stmt.clone();
    let all = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>| {
            let stmt = all_stmt.clone();
            async move {
                let objects = stmt.all().await;
                if let Some(first) = objects.first() {
                    check_error(&ctx, first)?;
                }
                let arr = Array::new(ctx.clone())?;
                for o in objects {
                    let v = json_to_js(&ctx, &serde_json::Value::Object(o))?;
                    push_array(&arr, v)?;
                }
                Ok::<_, rquickjs::Error>(Value::from(arr))
            }
        }),
    )?;
    obj.set("All", all)?;

    let one_stmt = stmt.clone();
    let one = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>| {
            let stmt = one_stmt.clone();
            async move {
                let obj = stmt.one().await;
                check_error(&ctx, &obj)?;
                json_to_js(&ctx, &serde_json::Value::Object(obj))
            }
        }),
    )?;
    obj.set("One", one)?;

    let bind_stmt = stmt.clone();
    let bind = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>, v: Value<'js>| {
            let stmt = bind_stmt.clone();
            async move {
                let json = js_to_json(&ctx, &v)?;
                let next = query_stmt_object(&ctx, stmt.bind(&json))?;
                Ok::<_, rquickjs::Error>(Value::from(next))
            }
        }),
    )?;
    obj.set("Bind", bind)?;

    Ok(obj)
}

/// Build the JS object returned by `DB.Exec(sql)`.
fn exec_stmt_object<'js>(ctx: &Ctx<'js>, stmt: ExecStmt) -> rquickjs::Result<Object<'js>> {
    let obj = Object::new(ctx.clone())?;
    let run = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>| {
            let stmt = stmt.clone();
            async move {
                let o = stmt.run().await;
                check_error(&ctx, &o)?;
                json_to_js(&ctx, &serde_json::Value::Object(o))
            }
        }),
    )?;
    obj.set("Run", run)?;
    Ok(obj)
}

/// Build the JS object returned by `DB.Table(name)`.
fn table_stmt_object<'js>(ctx: &Ctx<'js>, stmt: TableStmt) -> rquickjs::Result<Object<'js>> {
    let obj = Object::new(ctx.clone())?;

    let all_stmt = stmt.clone();
    let all = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>| {
            let stmt = all_stmt.clone();
            async move {
                let objects = stmt.all().all().await;
                if let Some(first) = objects.first() {
                    check_error(&ctx, first)?;
                }
                let arr = Array::new(ctx.clone())?;
                for o in objects {
                    let v = json_to_js(&ctx, &serde_json::Value::Object(o))?;
                    push_array(&arr, v)?;
                }
                Ok::<_, rquickjs::Error>(Value::from(arr))
            }
        }),
    )?;
    obj.set("All", all)?;

    let one_stmt = stmt.clone();
    let one = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>| {
            let stmt = one_stmt.clone();
            async move {
                let o = stmt.one().one().await;
                check_error(&ctx, &o)?;
                json_to_js(&ctx, &serde_json::Value::Object(o))
            }
        }),
    )?;
    obj.set("One", one)?;

    let count_stmt = stmt.clone();
    let count = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>| {
            let stmt = count_stmt.clone();
            async move {
                let n = stmt.count().await;
                (n as f64).into_js(&ctx)
            }
        }),
    )?;
    obj.set("Count", count)?;

    let columns_stmt = stmt.clone();
    let columns = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>| {
            let stmt = columns_stmt.clone();
            async move {
                let objects = stmt.columns().await;
                let arr = Array::new(ctx.clone())?;
                for o in objects {
                    let v = json_to_js(&ctx, &serde_json::Value::Object(o))?;
                    push_array(&arr, v)?;
                }
                Ok::<_, rquickjs::Error>(Value::from(arr))
            }
        }),
    )?;
    obj.set("Columns", columns)?;

    let insert_stmt = stmt.clone();
    let insert = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>, v: Value<'js>| {
            let stmt = insert_stmt.clone();
            async move {
                let o = js_to_json(&ctx, &v)?;
                let obj = serde_json::from_value(o).map_err(|e| {
                    js_err(&ctx, format!("TableStmt.Insert: expected an object: {e}"))
                })?;
                Ok::<_, rquickjs::Error>(Value::from(exec_stmt_object(&ctx, stmt.insert(&obj))?))
            }
        }),
    )?;
    obj.set("Insert", insert)?;

    let update_stmt = stmt.clone();
    let update = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>, v: Value<'js>| {
            let stmt = update_stmt.clone();
            async move {
                let o = js_to_json(&ctx, &v)?;
                let obj = serde_json::from_value(o).map_err(|e| {
                    js_err(&ctx, format!("TableStmt.Update: expected an object: {e}"))
                })?;
                Ok::<_, rquickjs::Error>(Value::from(exec_stmt_object(&ctx, stmt.update(&obj))?))
            }
        }),
    )?;
    obj.set("Update", update)?;

    let where_stmt = stmt.clone();
    let where_fn = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>, v: Value<'js>| {
            let stmt = where_stmt.clone();
            async move {
                let o = js_to_json(&ctx, &v)?;
                let obj = serde_json::from_value(o).map_err(|e| {
                    js_err(&ctx, format!("TableStmt.Where: expected an object: {e}"))
                })?;
                Ok::<_, rquickjs::Error>(Value::from(table_stmt_object(&ctx, stmt.where_(&obj))?))
            }
        }),
    )?;
    obj.set("Where", where_fn)?;

    let delete_stmt = stmt.clone();
    let delete = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>| {
            let stmt = delete_stmt.clone();
            async move { Ok::<_, rquickjs::Error>(Value::from(exec_stmt_object(&ctx, stmt.delete())?)) }
        }),
    )?;
    obj.set("Delete", delete)?;

    Ok(obj)
}

// ---- Engine ----

/// A QuickJS-backed engine for running `.rhp` script sections. One engine is
/// created per HTTP request (or per websocket connection) so that globals and
/// accumulated `write()` output persist across sections within that request.
pub struct Engine {
    internal: AsyncContext,
    conn: DbConn,
}

impl Engine {
    pub async fn new(conn: DbConn) -> Result<Engine> {
        let runtime = rquickjs::AsyncRuntime::new()?;
        let internal = AsyncContext::full(&runtime).await?;
        Ok(Engine { internal, conn })
    }

    /// Register request globals into a fresh context. Sets up `RES`,
    /// `QUERY`, `BODY`, `REQ`, `DB`, `console`, `VERSION`, `SOCKET` (if a
    /// socket is present for the request), plus `write`/`writeRaw`.
    pub async fn setup(&self, context: &Context) -> Result<()> {
        let conn = self.conn.clone();
        let socket = context.socket.clone();

        self.internal
            .async_with(async |ctx| {
                let globals = ctx.globals();

                globals.set("VERSION", "0.0.2")?;

                // __rhp_out accumulator array (persists across sections)
                let out_arr = Array::new(ctx.clone())?;
                globals.set("__rhp_out", out_arr)?;

                // write / writeRaw push escaped/raw text into __rhp_out
                let write = Function::new(ctx.clone(), |ctx, v| -> rquickjs::Result<()> {
                    let text = stringify_value(&ctx, &v)?;
                    let out: Array = ctx.globals().get("__rhp_out")?;
                    push_array(&out, escape_html(&text).into_js(&ctx)?)?;
                    Ok(())
                })?;
                globals.set("write", write)?;

                let write_raw = Function::new(ctx.clone(), |ctx, v| -> rquickjs::Result<()> {
                    let text = stringify_value(&ctx, &v)?;
                    let out: Array = ctx.globals().get("__rhp_out")?;
                    push_array(&out, text.into_js(&ctx)?)?;
                    Ok(())
                })?;
                globals.set("writeRaw", write_raw)?;

                // console.log -> server stdout
                let log = console_log(&ctx)?;
                let console = Object::new(ctx.clone())?;
                console.set("log", log)?;
                globals.set("console", console)?;

                // RES object
                let res = Object::new(ctx.clone())?;
                res.set("Headers", Object::new(ctx.clone())?)?;
                res.set("Cookies", Object::new(ctx.clone())?)?;

                let res_json = Function::new(ctx.clone(), |ctx, v| -> rquickjs::Result<()> {
                    let json = js_to_json(&ctx, &v)?;
                    let text = serde_json::to_string(&json)
                        .map_err(|e| js_err(&ctx, format!("json error: {e}")))?;
                    let res: Object = ctx.globals().get("RES")?;
                    res.set("_body", text.into_js(&ctx)?)?;
                    res.set("_content_type", "application/json".into_js(&ctx)?)?;
                    Ok(())
                })?;
                res.set("Json", res_json)?;

                let res_html = Function::new(
                    ctx.clone(),
                    |ctx: Ctx<'_>, html: String| -> rquickjs::Result<()> {
                        let res: Object = ctx.globals().get("RES")?;
                        res.set("_body", html.clone().into_js(&ctx)?)?;
                        res.set("_content_type", "text/html".into_js(&ctx)?)?;
                        Ok(())
                    },
                )?;
                res.set("Html", res_html)?;

                let res_redirect = Function::new(
                    ctx.clone(),
                    |ctx: Ctx<'_>, url: String| -> rquickjs::Result<()> {
                        let res: Object = ctx.globals().get("RES")?;
                        res.set("_redirect", url.into_js(&ctx)?)?;
                        // default 302 unless a status was already set
                        let status: rquickjs::Value = res
                            .get("Status")
                            .unwrap_or(rquickjs::Value::new_undefined(ctx.clone()));
                        if status.is_undefined() || status.is_null() {
                            res.set("Status", 302)?;
                        }
                        Ok(())
                    },
                )?;
                res.set("Redirect", res_redirect)?;

                let res_set_cookie = Function::new(
                    ctx.clone(),
                    |ctx: Ctx<'_>,
                     name: String,
                     value: String,
                     opts: Opt<Object<'_>>|
                     -> rquickjs::Result<()> {
                        let mut parts = vec![format!("{name}={value}")];
                        if let Some(o) = opts.0 {
                            let _ = o
                                .get::<_, String>("Path")
                                .map(|p| parts.push(format!("Path={p}")))
                                .ok();
                            let _ = o
                                .get::<_, String>("MaxAge")
                                .map(|m| parts.push(format!("Max-Age={m}")))
                                .ok();
                            let _ = o
                                .get::<_, bool>("HttpOnly")
                                .map(|b| {
                                    if b {
                                        parts.push("HttpOnly".to_string())
                                    }
                                })
                                .ok();
                            let _ = o
                                .get::<_, bool>("Secure")
                                .map(|b| {
                                    if b {
                                        parts.push("Secure".to_string())
                                    }
                                })
                                .ok();
                            let _ = o
                                .get::<_, String>("SameSite")
                                .map(|s| parts.push(format!("SameSite={s}")))
                                .ok();
                        }
                        let res: Object = ctx.globals().get("RES")?;
                        let cookies: Array = match res.get("_cookies") {
                            Ok(a) => a,
                            Err(_) => {
                                let a = Array::new(ctx.clone())?;
                                res.set("_cookies", a.clone())?;
                                a
                            }
                        };
                        push_array(&cookies, parts.join("; ").into_js(&ctx)?)?;
                        Ok(())
                    },
                )?;
                res.set("SetCookie", res_set_cookie)?;

                globals.set("RES", res)?;

                // QUERY / BODY / REQ
                let query_json = serde_json::to_value(&context.query)?;
                globals.set("QUERY", json_to_js(&ctx, &query_json)?)?;
                globals.set("BODY", json_to_js(&ctx, &context.body)?)?;

                let mut req_map = serde_json::Map::new();
                req_map.insert(
                    "Headers".to_string(),
                    serde_json::to_value(&context.headers)?,
                );
                let cookies: serde_json::Value = serde_json::to_value(&context.cookies)?;
                req_map.insert("Cookies".to_string(), cookies);
                globals.set(
                    "REQ",
                    json_to_js(&ctx, &serde_json::Value::Object(req_map))?,
                )?;

                // DB object
                let db = Object::new(ctx.clone())?;
                let q_conn = conn.clone();
                let query_fn = Function::new(
                    ctx.clone(),
                    Async(move |ctx, sql: String| {
                        let conn = q_conn.clone();
                        async move {
                            Ok::<_, rquickjs::Error>(Value::from(query_stmt_object(
                                &ctx,
                                conn.query(&sql),
                            )?))
                        }
                    }),
                )?;
                db.set("Query", query_fn)?;

                let e_conn = conn.clone();
                let exec_fn = Function::new(
                    ctx.clone(),
                    Async(move |ctx, sql: String| {
                        let conn = e_conn.clone();
                        async move {
                            Ok::<_, rquickjs::Error>(Value::from(exec_stmt_object(
                                &ctx,
                                conn.exec(&sql),
                            )?))
                        }
                    }),
                )?;
                db.set("Exec", exec_fn)?;

                let t_conn = conn.clone();
                let table_fn = Function::new(
                    ctx.clone(),
                    Async(move |ctx, name: String| {
                        let conn = t_conn.clone();
                        async move {
                            Ok::<_, rquickjs::Error>(Value::from(table_stmt_object(
                                &ctx,
                                conn.table(&name),
                            )?))
                        }
                    }),
                )?;
                db.set("Table", table_fn)?;

                globals.set("DB", db)?;

                // SOCKET (when this request is a websocket connection)
                if let Some(socket) = &socket {
                    crate::ws::register_socket(&ctx, socket)?;
                }

                Ok::<(), anyhow::Error>(())
            })
            .await?;

        Ok(())
    }

    /// Run a single script section (the contents of `<rhp>...</rhp>`).
    /// Returns the accumulated `write()`/`writeRaw()` output and the script's
    /// completion value (the result of the section's last expression).
    pub async fn run_section(&self, code: &str) -> Result<(String, serde_json::Value)> {
        let script = format!("(async () => {{\n{code}\n}})()");

        self.internal
            .async_with(async |ctx| {
                let promise: Promise = ctx.eval(script.as_str())?;
                let result: Value<'_> = promise
                    .into_future()
                    .await
                    .map_err(|e| anyhow!("script error: {e}"))?;
                let completion = js_to_json(&ctx, &result)?;
                let out: Array = ctx.globals().get("__rhp_out")?;
                let mut text = String::new();
                for item in out.iter::<Value<'_>>() {
                    let item = item?;
                    if let Some(s) = item.as_string() {
                        text.push_str(&s.to_string().unwrap_or_default());
                    }
                }
                Ok((text, completion))
            })
            .await
    }

    /// Run one SOCKET section and return its completion value.
    pub async fn run_socket_section(&self, code: &str) -> Result<serde_json::Value> {
        let (_, completion) = self.run_section(code).await?;
        Ok(completion)
    }

    /// Run an asynchronous closure over the JS context, allowing it to `await`
    /// a Promise (e.g. an async host function) inside the closure. Used to
    /// invoke stored JS callbacks that make `await` DB calls.
    pub async fn call_async<R>(
        &self,
        f: impl for<'js> AsyncFnOnce(Ctx<'js>) -> rquickjs::Result<R> + ParallelSend,
    ) -> rquickjs::Result<R>
    where
        R: ParallelSend + 'static,
    {
        self.internal.async_with(f).await
    }

    /// Read the script's response intent from the `RES` object.
    pub async fn read_response(&self) -> HttpResponseState {
        let state = self
            .internal
            .with(|ctx| {
                let res: Object = ctx.globals().get("RES")?;

                let status: Option<u16> = res.get("Status").unwrap_or(None);

                let mut headers: Vec<(String, String)> = Vec::new();
                if let Ok(h) = res.get::<_, Object>("Headers") {
                    for entry in h.props::<String, Value>() {
                        let (k, val) = entry?;
                        if let Some(s) = val.as_string() {
                            headers.push((k, s.to_string().unwrap_or_default()));
                        }
                    }
                    headers.sort_by(|a, b| a.0.cmp(&b.0));
                }

                let body: Option<String> = res.get("_body").unwrap_or(None);
                let content_type: Option<String> = res.get("_content_type").unwrap_or(None);
                let redirect: Option<String> = res.get("_redirect").unwrap_or(None);

                let mut cookies: Vec<String> = Vec::new();
                if let Ok(a) = res.get::<_, Array>("_cookies") {
                    for item in a.iter::<Value>() {
                        let item = item?;
                        if let Some(s) = item.as_string() {
                            cookies.push(s.to_string().unwrap_or_default());
                        }
                    }
                }

                let owns_response = body.is_some() || redirect.is_some();

                Ok::<_, rquickjs::Error>(HttpResponseState {
                    status,
                    headers,
                    body,
                    content_type,
                    redirect,
                    cookies,
                    owns_response,
                })
            })
            .await;

        state.unwrap_or_default()
    }
}

/// Produce a string form of any JS value (uses String() semantics).
fn stringify_value<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> rquickjs::Result<String> {
    let f: Function = ctx.globals().get("String")?;
    let s: String = f.call((v.clone(),))?;
    Ok(s)
}
