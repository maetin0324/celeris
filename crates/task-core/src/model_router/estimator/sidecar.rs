//! Pure version 1 `/estimate` contract. The HTTP client owns transport and timeouts.
use std::collections::{BTreeMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{EstimatorDescriptor as KernelDescriptor, QualityEstimate, QualityEstimator};
use crate::model_router::{context::RoutingContext, profiles::ModelProfile};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_CANDIDATES: usize = 128;
const MAX_ID_BYTES: usize = 128;
const MAX_REASON_BYTES: usize = 256;
const MAX_REASONS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EstimatorDependencies {
    pub needs_network: bool,
    pub external_embeddings: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EstimatorDescriptor {
    pub estimator_id: String,
    pub version: String,
    pub protocol_version: u32,
    pub needs_prompt: bool,
    pub dependencies: EstimatorDependencies,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EstimateCandidateV1 {
    pub model_profile_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EstimateRequestV1 {
    pub request_id: String,
    pub context_features: Map<String, Value>,
    pub candidates: Vec<EstimateCandidateV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional_prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EstimateValueV1 {
    pub model_profile_id: String,
    pub index: Option<f64>,
    pub confidence: Option<f64>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EstimateResponseV1 {
    pub request_id: String,
    pub estimator_id: String,
    pub version: String,
    pub estimates: Vec<EstimateValueV1>,
    pub dependencies: EstimatorDependencies,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SidecarProtocolError {
    #[error("{field} is empty, invalid, or too long")]
    InvalidIdentity { field: &'static str },
    #[error("unsupported estimator protocol version {0}")]
    ProtocolVersion(u32),
    #[error("{field} exceeds {limit} bytes")]
    Size { field: &'static str, limit: usize },
    #[error("invalid candidate count")]
    CandidateCount,
    #[error("unknown model profile id: {0}")]
    UnknownModel(String),
    #[error("duplicate model profile id: {0}")]
    DuplicateModel(String),
    #[error("missing model profile id: {0}")]
    MissingModel(String),
    #[error("request id mismatch")]
    RequestId,
    #[error("estimator id or version mismatch")]
    EstimatorVersion,
    #[error("estimator dependencies differ from descriptor")]
    Dependencies,
    #[error("{field} for {model} must be finite and within [0, 1]")]
    Range { field: &'static str, model: String },
    #[error("too many or oversized reasons for {0}")]
    Reasons(String),
    #[error("cannot serialize {field}: {detail}")]
    Serialization { field: &'static str, detail: String },
}

fn identity(value: &str, field: &'static str) -> Result<(), SidecarProtocolError> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-/".contains(&b))
    {
        return Err(SidecarProtocolError::InvalidIdentity { field });
    }
    Ok(())
}

fn bounded_json<T: Serialize>(
    value: &T,
    limit: usize,
    field: &'static str,
) -> Result<(), SidecarProtocolError> {
    let bytes = serde_json::to_vec(value).map_err(|error| SidecarProtocolError::Serialization {
        field,
        detail: error.to_string(),
    })?;
    if bytes.len() > limit {
        return Err(SidecarProtocolError::Size { field, limit });
    }
    Ok(())
}

impl EstimatorDescriptor {
    pub fn validate(&self) -> Result<(), SidecarProtocolError> {
        identity(&self.estimator_id, "estimator_id")?;
        identity(&self.version, "version")?;
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(SidecarProtocolError::ProtocolVersion(self.protocol_version));
        }
        Ok(())
    }
}

impl EstimateRequestV1 {
    pub fn validate(&self, max_payload_bytes: usize) -> Result<(), SidecarProtocolError> {
        identity(&self.request_id, "request_id")?;
        if self.candidates.is_empty() || self.candidates.len() > MAX_CANDIDATES {
            return Err(SidecarProtocolError::CandidateCount);
        }
        let mut ids = HashSet::new();
        for candidate in &self.candidates {
            identity(&candidate.model_profile_id, "model_profile_id")?;
            if !ids.insert(candidate.model_profile_id.as_str()) {
                return Err(SidecarProtocolError::DuplicateModel(
                    candidate.model_profile_id.clone(),
                ));
            }
        }
        bounded_json(self, max_payload_bytes, "request")
    }
}

impl EstimateResponseV1 {
    pub fn validate(
        &self,
        request: &EstimateRequestV1,
        descriptor: &EstimatorDescriptor,
        max_payload_bytes: usize,
    ) -> Result<(), SidecarProtocolError> {
        descriptor.validate()?;
        request.validate(max_payload_bytes)?;
        bounded_json(self, max_payload_bytes, "response")?;
        if self.request_id != request.request_id {
            return Err(SidecarProtocolError::RequestId);
        }
        if self.estimator_id != descriptor.estimator_id || self.version != descriptor.version {
            return Err(SidecarProtocolError::EstimatorVersion);
        }
        if self.dependencies != descriptor.dependencies {
            return Err(SidecarProtocolError::Dependencies);
        }
        let expected: HashSet<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.model_profile_id.as_str())
            .collect();
        let mut seen = HashSet::new();
        for estimate in &self.estimates {
            let id = estimate.model_profile_id.as_str();
            if !expected.contains(id) {
                return Err(SidecarProtocolError::UnknownModel(id.to_owned()));
            }
            if !seen.insert(id) {
                return Err(SidecarProtocolError::DuplicateModel(id.to_owned()));
            }
            for (field, value) in [
                ("index", estimate.index),
                ("confidence", estimate.confidence),
            ] {
                if value.is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v)) {
                    return Err(SidecarProtocolError::Range {
                        field,
                        model: id.to_owned(),
                    });
                }
            }
            if estimate.reasons.len() > MAX_REASONS
                || estimate
                    .reasons
                    .iter()
                    .any(|reason| reason.is_empty() || reason.len() > MAX_REASON_BYTES)
            {
                return Err(SidecarProtocolError::Reasons(id.to_owned()));
            }
        }
        for id in expected {
            if !seen.contains(id) {
                return Err(SidecarProtocolError::MissingModel(id.to_owned()));
            }
        }
        Ok(())
    }
}

/// Constructed only from a validated response. No transport or selection state is retained.
#[derive(Debug, Clone)]
pub struct SidecarEstimateSnapshot {
    descriptor: KernelDescriptor,
    estimates: BTreeMap<String, QualityEstimate>,
}

impl SidecarEstimateSnapshot {
    pub fn from_response(
        request: &EstimateRequestV1,
        response: &EstimateResponseV1,
        descriptor: &EstimatorDescriptor,
        max_payload_bytes: usize,
    ) -> Result<Self, SidecarProtocolError> {
        response.validate(request, descriptor, max_payload_bytes)?;
        let estimates = response
            .estimates
            .iter()
            .map(|value| {
                (
                    value.model_profile_id.clone(),
                    QualityEstimate {
                        index: value.index,
                        confidence: value.confidence,
                        reasons: value.reasons.clone(),
                        feature_version: format!("sidecar-v{}", PROTOCOL_VERSION),
                    },
                )
            })
            .collect();
        Ok(Self {
            descriptor: KernelDescriptor {
                id: descriptor.estimator_id.clone(),
                version: descriptor.version.clone(),
                maturity: "external".into(),
                external_dependencies: [
                    descriptor.dependencies.needs_network.then_some("network"),
                    descriptor
                        .dependencies
                        .external_embeddings
                        .then_some("external_embeddings"),
                ]
                .into_iter()
                .flatten()
                .map(str::to_owned)
                .collect(),
                needs_network: descriptor.dependencies.needs_network,
                needs_prompt: descriptor.needs_prompt,
                supported_domains: vec![],
            },
            estimates,
        })
    }
}

impl QualityEstimator for SidecarEstimateSnapshot {
    fn descriptor(&self) -> KernelDescriptor {
        self.descriptor.clone()
    }

    fn estimate(&self, model: &ModelProfile, _context: &RoutingContext) -> QualityEstimate {
        self.estimates
            .get(&model.id)
            .cloned()
            .unwrap_or_else(|| QualityEstimate {
                index: None,
                confidence: None,
                reasons: vec!["quality_unknown".into()],
                feature_version: format!("sidecar-v{}", PROTOCOL_VERSION),
            })
    }
}
