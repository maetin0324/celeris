//! ADR 2026-11-04-ts-execution-gate-domain: execution_gate の比較 fixture を stdout に出す。
//! `cargo run -q -p task-core --example dump-execution-gate-fixtures > control-ts/fixtures/execution_gate/cases.jsonl`

use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let body = task_core::script_compare::execution_gate_fixture_jsonl()?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(body.as_bytes())?;
    stdout.flush()?;
    Ok(())
}
