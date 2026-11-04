//! ADR 2026-11-04-project-controller D7: project_controller の不変条件の fixture を stdout に出す。
//! `cargo run -q -p task-core --example dump-project-controller-fixtures > control-ts/fixtures/project_controller/invariants.jsonl`

use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let body = task_core::project_proposal::invariants_fixture_jsonl()?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(body.as_bytes())?;
    stdout.flush()?;
    Ok(())
}
