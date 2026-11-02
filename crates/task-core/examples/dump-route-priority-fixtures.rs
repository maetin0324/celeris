//! S2: route_priority の比較 fixture を stdout に出す。
//! `cargo run -q -p task-core --example dump-route-priority-fixtures > control-ts/fixtures/route_priority/cases.jsonl`

use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let body = task_core::script_compare::route_priority_fixture_jsonl()?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(body.as_bytes())?;
    stdout.flush()?;
    Ok(())
}
