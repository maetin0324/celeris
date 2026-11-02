//! S2: lane 規則表の比較 fixture を stdout に出す。
//! `cargo run -q -p task-core --example dump-lane-fixtures > control-ts/fixtures/lane_rules/all-features.jsonl`

use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let body = task_core::model_policy::lane_fixture_jsonl()?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(body.as_bytes())?;
    stdout.flush()?;
    Ok(())
}
