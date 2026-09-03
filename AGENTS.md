# AGENTS.md

RHP ("Rust Hypertext Preprocessor") is a PHP-like server-side scripting language served over HTTP. It is a single Rust crate (edition 2024) built on `axum` + `sqlx` + `tokio`.

## What it is / architecture

- The HTTP server serves `.rhp` files from `./public` (config `FOLDER`). Plain static files are served via `ServeDir`; only `.rhp` files execute.
- A `.rhp` file is a template: literal text is HTML, and `<rhp method="GET">…</rhp>` sections run scripts. Method tag controls which HTTP method (also `HEAD`, `POST`, `PUT`, `DELETE`, and `SOCKET`) executes the section; a non-matching method skips it.
- The interpreter pipeline lives in `src/`: `quickjs.rs` is the JS engine (via `rquickjs` crate). `process.rs` parses the template into sections, runs code sections through the engine, and produces `HttpResponse`. `db.rs` is the `DB` object (sqlx, sqlite/postgres). `ws.rs` implements websockets (`SOCKET`).
- Entry points: `main.rs` (CLI via clap) → `lib.rs::run_server`/`build_router` → `process::process_src`.
- `lib.rs::resolve_rhp` maps a path to an `.rhp` file: a path ending in `.rhp`, or `index.rhp` when the path names a directory. No route-pattern matching (`/users/:id` is unimplemented).

## Commands

- `cargo run`, start server. Config is env-var driven (main.rs): `PORT` (3000), `FOLDER` (./public), `DEBUG`, `DB_CONN` (default `:memory:`), `HOT_RELOAD`. A `.env` file is loaded if present (missing `.env` is fine).
- Verify before finishing: `cargo fmt --all -- --check` → `cargo clippy` → `cargo test`. This is exactly what CI (`ci.yml`) runs on `main` pushes. The `Release` workflow also runs them before cross-target builds.
- Tests: `cargo test` (112 tests, fast, pure in-memory). Run one: `cargo test test_socket`, `cargo test --lib process::process_tests::...`, etc.

## Conventions & gotchas

- **No `unwrap()`**: `clippy.toml` disallows `Option::unwrap` / `Result::unwrap`. Use `expect("msg")` or handle the error. This is enforced by `cargo clippy` in CI.
- **Test files are split out** via `#[path]` module declarations, not inline `#[cfg(test)] mod tests`: `db.rs`→`db_tests.rs`, `lib.rs`→`lib_tests.rs` + `ws_tests.rs` + `quickjs_tests.rs`. Add/run tests in those sidecar files.
- Tests use a **unique named shared in-memory sqlite** DB per test (e.g. `sqlite://file%3Arhp_proc_test_{id}?mode=memory&cache=shared`) so pooled connections share one DB. Copy the `test_conn()`/`unique_conn()` helper pattern rather than inventing a new one.
- `cargo fmt` output is committed; match the existing formatting.
- Branches: `main` (default, CI-tested) and `dev` exist; feature branches (e.g. `websocket`, `number-integer-float`) are the norm. Work on a feature branch, not `main`.
- Script runtime errors historically produce a blank/500-ish response; a readable error page is a known gap.

## Language is PHP-like, not Rust

Scripts are JS-ish, not Rust: `return`, `if`, `for..in`, `try`, `switch`, arrow closures, `const`/`let`, compound and bitwise operators. Objects are `{...}` keyed by identifier. Don't assume the runtime validates like Rust, many helpers return `Value::String` error messages on bad args (intrinsics like `DB` return `{ ok:false, error }` objects). When adding language features, update `TODO.md`.
