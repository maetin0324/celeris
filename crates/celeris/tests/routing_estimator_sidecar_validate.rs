//! ADR 2026-10-04 multi-objective model routing §10 Phase 5（shadow-check）: 実プロセスの sidecar
//! （`scripts/model-routing/routellm_sidecar.py --fake-classifier`）が返した `/estimate` v1 応答を
//! task-core の検証（[`EstimateResponseV1::validate`]）に通す。`scripts/model-routing/fake-shadow-check.sh`
//! が sidecar を起動して request/response を書き出し、この試験を `--ignored` で呼ぶ。通常の試験では
//! 走らない（入力の dir が無いため `#[ignore]`）。入力が無いまま呼ばれたら失敗する（skip を合格にしない）。
//!
//! 入力 dir（`CELERIS_SIDECAR_CHECK_DIR`）: `descriptor.json`（`/healthz` から組んだ descriptor）と
//! `request-<n>.json` / `response-<n>.json` の組（1 組以上）。

use std::path::Path;

use task_core::model_router::estimator::sidecar::{
    EstimateRequestV1, EstimateResponseV1, EstimatorDescriptor, SidecarProtocolError,
};

const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

#[test]
#[ignore = "run by scripts/model-routing/fake-shadow-check.sh with CELERIS_SIDECAR_CHECK_DIR"]
fn fake_sidecar_response_passes_task_core_validation() {
    let dir = std::env::var_os("CELERIS_SIDECAR_CHECK_DIR")
        .expect("CELERIS_SIDECAR_CHECK_DIR is required (not a passing skip)");
    let dir = Path::new(&dir);
    let descriptor: EstimatorDescriptor = read(&dir.join("descriptor.json"));
    descriptor.validate().expect("descriptor from /healthz");

    let mut pairs = 0;
    for n in 0.. {
        let request_path = dir.join(format!("request-{n}.json"));
        if !request_path.is_file() {
            break;
        }
        let request: EstimateRequestV1 = read(&request_path);
        let response: EstimateResponseV1 = read(&dir.join(format!("response-{n}.json")));
        response
            .validate(&request, &descriptor, MAX_PAYLOAD_BYTES)
            .unwrap_or_else(|e| panic!("pair {n}: sidecar response rejected: {e}"));
        assert_eq!(
            response.estimates.len(),
            request.candidates.len(),
            "pair {n}: every candidate is answered"
        );

        // 検証が空振りでないこと: 同じ応答の request_id を変えると拒否される。
        let mut tampered = response.clone();
        tampered.request_id = format!("{}-other", request.request_id);
        assert!(
            matches!(
                tampered.validate(&request, &descriptor, MAX_PAYLOAD_BYTES),
                Err(SidecarProtocolError::RequestId)
            ),
            "pair {n}: tampered request_id must be rejected"
        );
        pairs += 1;
    }
    assert!(pairs > 0, "no request/response pairs in {}", dir.display());
    println!("validated {pairs} sidecar response(s)");
}
