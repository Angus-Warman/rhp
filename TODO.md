## Bugs
- <label for> breaks parsing

## HTTP / server layer

- [x] `[P0]` **Script-controlled HTTP responses.** Today every `.rhp` always
      returns `200` with `Content-Type: text/html`. Add a `RES` object so
      scripts can set status, set headers, emit JSON, and redirect:
      `RES.Status = 404`, `RES.Headers["X-Foo"] = "bar"`,
      `RES.Json({...})`, `RES.Redirect("/login")`. Short-circuit
      remaining sections after a redirect/return. — *Done: `RES` object
      with Status/Headers/SetCookie/Json/Html/Redirect; `HttpResponse`
      threaded through `process_src` → `into_axum`.*
- [x] `[P0]` **Cookies.** Read via `REQ.Cookies.name`, set via
      `RES.SetCookie(name, value, { ... })`. Needed for real sessions. —
      *Done: `REQ.Cookies` parsed from the request header; `SetCookie`
      supports Path/MaxAge/HttpOnly/Secure/SameSite.*
- [ ] `[P1]` **Path parameters** (`/users/:id` → `PARAM.id`). `resolve_rhp`
      only matches files/dirs; add route patterns that keep mapping to `.rhp`
      files (e.g. `users/:id.rhp`).
- [ ] `[P2]` **Multipart / file uploads** in `parse_body`.
- [ ] `[P2]` **Static assets**: cache headers, `HEAD` handling.

## Language core

- [x] `[P0]` **DB error-shape consistency.** `DB.Query("...")`,
      `DB.Exec(...)`, `DB.Table(...).Insert/Update/Where` return `Value::String`
      error messages on bad args, but everything else (JSON, MATH, methods)
      returns `{ ok: false, error }`. Unify on error objects so `try` works
      uniformly. Same for the `DB.Ping()` `unwrap()`. — *Done.*
- [x] `[P0]` **`const` enforcement.** Assigning to a `const` should error. —
      *Done: `Env` tracks consts; assignment raises a runtime error.*
- [x] `[P1]` **`typeof x`** / `x.type` — return `"string"`, `"number"`, etc.
      — *Done: prefix `typeof` operator and `.type` property on non-object
      values.*
- [ ] `[P1]` **`throw` + `try/catch`.** `try` today only early-returns falsy
      *values*; thrown runtime errors still abort the script. Add `catch (e)`
      handling of `Signal::Error`.
- [ ] `[P1]` **Backtick string interpolation** `` `hi {name}` `` (or reuse
      `{expr}`).
- [ ] `[P1]` **Unicode `.length`.** String `.length` uses byte length; make it
      character count.
- [ ] `[P2]` **`do...while`**, nullish coalescing `??`, optional
      chaining `?.`, exponent `**`.
- [x] `[P2]` **`switch` statements** — JS-style with fall-through, `case`/`default`,
      `break` to exit, `continue` to skip enclosing loop. — *Done.*
- [x] `[P2]` **Compound assignments** `%=`, `&=`, `|=`, `^=`, `<<=`, `>>=`
      and bitwise binary operators `&`, `|`, `^`, `<<`, `>>`. — *Done.*
- [ ] `[P2]` **`var` vs `let` scoping** semantics (currently identical).

## Array / object helpers

- [x] `[P1]` **Iteration/transformation methods** — the gap that makes
      server-side list rendering awkward: `map`, `filter`, `forEach`,
      `reduce`, `sort`, `slice`, `indexOf`, `includes`. — *Done.*
- [ ] `[P1]` **Object helpers**: `Object.keys/values/entries`.
- [ ] `[P2]` **`String.padStart`, `substr`, `startsWith/endsWith`**, number
      formatting (`toFixed`).

## Templating

- [ ] `[P1]` **Conditional / loop constructs inside templates** so lists can
      be rendered declaratively (e.g. `{#if ok}`, `{#each items}`) instead of
      hand-accumulating strings with `for..in`.
- [ ] `[P2]` **Unescaped slot escape hatch** — e.g. `{safe(html)}` or
      `{html!}` to inject pre-trusted markup (today every slot auto-escapes).

## DB layer

- [ ] `[P1]` **Transactions** exposed (`DB.Tx()` + commit/rollback).
- [ ] `[P1]` **Last insert id** from `Insert(...).Run()`.
- [ ] `[P2]` **`TableStmt` ordering/limits** (`OrderBy`, `Limit`, `Offset`).
- [ ] `[P2]` **Schema migrations** (a `migrations/` folder or `schema.rhp`).

## WebSocket

- [ ] `[P1]` **Ping/pong heartbeat** + stale connection reaping. (not needed, Axum does this already)
- [ ] `[P2]` **Message size limit** and structured error broadcast.
- [ ] `[P2]` **Room presence** (`SOCKET.Room(...).Members()` or broadcast
      events on join/leave).

## Project
- [ ] `[P1]` **Example app** — a small CRUD todo/notes app showing
      `DB.Table`, forms (`BODY`), templates, and redirects end-to-end.
- [ ] `[P2]` **CI** (cargo fmt / clippy / test on push).
- [ ] `[P2]` **Error page for script runtime errors** — today a thrown error
      becomes a 500-ish blank response; render a readable traceback.

