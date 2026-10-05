use task_core::model_router::{
    context::RoutingContext,
    estimator::{QualityEstimator, sidecar::*},
    profiles::{Capabilities, ContextLimits, ModelProfile, Support},
    shadow::ShadowKind,
};

fn fixture(name: &str) -> String {
    let root = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/estimator_sidecar_v1/"
    );
    std::fs::read_to_string(format!("{root}{name}.json")).expect("fixture is checked in")
}

fn model(id: &str) -> ModelProfile {
    ModelProfile {
        id: id.into(),
        revision: "1".into(),
        family: "test".into(),
        capabilities: Capabilities {
            tools: Support::Unknown,
            structured_output: Support::Unknown,
            vision: Support::Unknown,
            streaming: Support::Unknown,
            reasoning_efforts: vec![],
        },
        context_limits: ContextLimits {
            input: None,
            output: None,
            total: None,
        },
        quality: vec![],
        pricing: None,
        provenance: "fixture".into(),
    }
}

#[test]
fn routing_sidecar_protocol_validates_identity_range_and_size() {
    let descriptor: EstimatorDescriptor =
        serde_json::from_str(&fixture("descriptor_valid")).unwrap();
    let request: EstimateRequestV1 = serde_json::from_str(&fixture("request_valid")).unwrap();
    let response: EstimateResponseV1 = serde_json::from_str(&fixture("response_valid")).unwrap();
    let limit = 4096;
    request.validate(limit).unwrap();
    response.validate(&request, &descriptor, limit).unwrap();
    let snapshot =
        SidecarEstimateSnapshot::from_response(&request, &response, &descriptor, limit).unwrap();
    assert_eq!(snapshot.descriptor().id, "route-test");
    assert_eq!(
        snapshot
            .estimate(&model("model-a"), &RoutingContext::default())
            .index,
        Some(0.8)
    );
    assert_eq!(
        snapshot
            .estimate(&model("model-b"), &RoutingContext::default())
            .index,
        None
    );
    assert_eq!(
        snapshot
            .estimate(&model("model-x"), &RoutingContext::default())
            .index,
        None
    );
    assert_eq!(
        serde_json::to_string(&ShadowKind::Estimator).unwrap(),
        "\"estimator\""
    );

    for (name, expected) in [
        (
            "response_unknown_id",
            SidecarProtocolError::UnknownModel("model-x".into()),
        ),
        (
            "response_duplicate_id",
            SidecarProtocolError::DuplicateModel("model-a".into()),
        ),
        ("response_wrong_request", SidecarProtocolError::RequestId),
        (
            "response_wrong_version",
            SidecarProtocolError::EstimatorVersion,
        ),
        (
            "response_out_of_range",
            SidecarProtocolError::Range {
                field: "index",
                model: "model-a".into(),
            },
        ),
        (
            "response_missing_id",
            SidecarProtocolError::MissingModel("model-b".into()),
        ),
    ] {
        let invalid: EstimateResponseV1 = serde_json::from_str(&fixture(name)).unwrap();
        assert_eq!(
            invalid.validate(&request, &descriptor, limit),
            Err(expected),
            "{name}"
        );
    }

    let mut duplicate_request = request.clone();
    duplicate_request.candidates[1].model_profile_id = "model-a".into();
    assert_eq!(
        duplicate_request.validate(limit),
        Err(SidecarProtocolError::DuplicateModel("model-a".into()))
    );
    let mut invalid_descriptor = descriptor.clone();
    invalid_descriptor.protocol_version = 2;
    assert_eq!(
        response.validate(&request, &invalid_descriptor, limit),
        Err(SidecarProtocolError::ProtocolVersion(2))
    );
    let mut nan = response.clone();
    nan.estimates[0].index = Some(f64::NAN);
    assert!(matches!(
        nan.validate(&request, &descriptor, limit),
        Err(SidecarProtocolError::Range { field: "index", .. })
    ));
    let mut wrong_dependencies = response.clone();
    wrong_dependencies.dependencies.external_embeddings = true;
    assert_eq!(
        wrong_dependencies.validate(&request, &descriptor, limit),
        Err(SidecarProtocolError::Dependencies)
    );
    assert!(matches!(
        request.validate(32),
        Err(SidecarProtocolError::Size {
            field: "request",
            ..
        })
    ));
    assert!(matches!(
        response.validate(&request, &descriptor, 200),
        Err(SidecarProtocolError::Size { .. })
    ));
}
