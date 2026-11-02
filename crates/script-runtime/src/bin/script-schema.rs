//! Host API `celeris:host` v1 の DTO の JSON Schema を stdout へ出す。
//!
//! 生成物は `docs/api/script-host/host-v1.schema.json`。更新は
//! `cargo run -q -p celeris-script --bin script-schema > docs/api/script-host/host-v1.schema.json`。

use std::io::Write;

fn main() -> std::process::ExitCode {
    let out = celeris_script::dto::host_api_schema_json();
    match std::io::stdout().write_all(out.as_bytes()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => std::process::ExitCode::FAILURE,
    }
}
