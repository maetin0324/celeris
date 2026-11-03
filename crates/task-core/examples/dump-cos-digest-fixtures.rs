//! S4: cos_digest の比較 fixture を stdout に出す。
//! `cargo run -q -p task-core --example dump-cos-digest-fixtures > control-ts/fixtures/cos_digest/cases.jsonl`

use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let body = task_core::chat::cos_digest_fixture_jsonl()?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(body.as_bytes())?;
    stdout.flush()?;
    Ok(())
}
