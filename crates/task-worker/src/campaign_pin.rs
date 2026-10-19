//! variant pin の run 証拠への保存と、continuation・再開時の照合（propagation §8.5 / D7）。
//!
//! 保存先は 2 つ: `runs/<run_id>/campaign-pin.json`（その run の証拠）と
//! `<artifacts_dir>/campaign-pin.json`（task の保存 pin。最初の variant run が作る）。
//! 保存 pin がある task の後続 run（continuation・retry・再開）は、同じ pin を運ぶことが条件で、
//! pin 欠落も食い違いも明示失敗にする。保存 pin の無い plain な run は何も書かず何も検査しない。

use std::path::{Path, PathBuf};

use crate::adapter::AdapterError;
use crate::protocol::{CampaignRunPin, RunRequest};

pub const PIN_FILE: &str = "campaign-pin.json";

fn stored_path(req: &RunRequest) -> PathBuf {
    req.artifacts_dir.join(PIN_FILE)
}

fn read_stored(path: &Path) -> Result<Option<CampaignRunPin>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("stored campaign pin {} is unreadable: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("stored campaign pin {}: {e}", path.display())),
    }
}

/// run の pin と保存 pin を照合し、証拠を書く。失敗は run を始めない理由（`AdapterError::Other`）。
pub(crate) async fn enforce(run_dir: &Path, req: &RunRequest) -> Result<(), AdapterError> {
    check(run_dir, req).await.map_err(AdapterError::Other)
}

async fn check(run_dir: &Path, req: &RunRequest) -> Result<(), String> {
    let path = stored_path(req);
    let stored = read_stored(&path)?;
    match (&req.context.campaign, stored) {
        (None, None) => Ok(()),
        (None, Some(_)) => Err(format!(
            "campaign_pin_missing: task {} has a stored campaign pin but this run carries none",
            req.task.id
        )),
        (Some(pin), stored) => {
            pin.validate()
                .map_err(|e| format!("campaign_pin_invalid: {e}"))?;
            if let Some(s) = &stored
                && s != pin
            {
                return Err(format!(
                    "campaign_pin_mismatch: task {} run pin differs from the stored pin",
                    req.task.id
                ));
            }
            let mut data = serde_json::to_vec_pretty(pin).map_err(|e| e.to_string())?;
            data.push(b'\n');
            tokio::fs::create_dir_all(&req.artifacts_dir)
                .await
                .map_err(|e| format!("campaign pin dir: {e}"))?;
            if stored.is_none() {
                tokio::fs::write(&path, &data)
                    .await
                    .map_err(|e| format!("campaign pin could not be stored: {e}"))?;
            }
            tokio::fs::write(run_dir.join(PIN_FILE), &data)
                .await
                .map_err(|e| format!("campaign pin evidence could not be written: {e}"))?;
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "campaign_pin_tests.rs"]
mod tests;
