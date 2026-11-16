use super::*;

const EPS: f64 = 1e-9;

fn close(a: Option<f64>, b: f64) -> bool {
    a.is_some_and(|v| (v - b).abs() < EPS)
}

/// task-api の `GET /llm-sources` が表示する pressure の規則: 既知の成分の最大、各成分は [0,1]、
/// 既知の成分が無ければ None、不正な値と分母 0 は拒否（見積もりの関数を消したので、ここで直接守る）。
#[test]
fn pressure_is_max_of_known_components_and_rejects_bad_values() {
    let mut s = SourceState::unobserved("gpu-1");
    assert_eq!(pressure(&s, None).unwrap(), None, "no known component");

    s.queue_limit = Some(4);
    s.queue_depth = Some(1);
    assert!(close(pressure(&s, None).unwrap(), 0.25));
    s.queue_depth = Some(3);
    assert!(close(pressure(&s, None).unwrap(), 0.75));
    s.in_use = Some(1);
    assert!(close(pressure(&s, Some(4)).unwrap(), 0.75), "max wins");
    s.in_use = Some(9);
    assert!(close(pressure(&s, Some(4)).unwrap(), 1.0), "clamped");

    let mut gpu = SourceState::unobserved("gpu-2");
    gpu.gpu_utilization = Some(f64::NAN);
    assert_eq!(pressure(&gpu, None).unwrap_err(), "invalid_estimate");

    let mut zero_queue = SourceState::unobserved("gpu-1");
    zero_queue.queue_depth = Some(1);
    zero_queue.queue_limit = Some(0);
    assert_eq!(pressure(&zero_queue, None).unwrap_err(), "zero_denominator");
}
