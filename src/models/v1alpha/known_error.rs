use crate::models::core::ModelMetadata;
use crate::models::v1alpha::V1AlphaApiVersion;
use crate::models::{HelpMetadata, InternalScopeModel, ScopeModel};
use derive_builder::Builder;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::prelude::DoctorFixSpec;

/// One or more regexes that determine whether a log line matches this known error.
/// Accepts either a single pattern string or a list of pattern strings; a line matching
/// any one of the patterns triggers the error.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(untagged)]
pub enum KnownErrorPattern {
    /// A single regex pattern.
    Single(String),
    /// A list of regex patterns; the known error fires when any one matches.
    Many(Vec<String>),
}

impl KnownErrorPattern {
    /// Returns the list of pattern strings (a single value becomes a one-element vec).
    pub fn patterns(&self) -> Vec<String> {
        match self {
            KnownErrorPattern::Single(p) => vec![p.clone()],
            KnownErrorPattern::Many(p) => p.clone(),
        }
    }
}

/// Definition of the known error
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct KnownErrorSpec {
    /// Text that the user can use to fix the issue
    pub help: String,

    /// A regex (or list of regexes) used to determine if the line is an error.
    /// A single string or a list of strings are both accepted; the known error fires when any
    /// pattern matches.
    pub pattern: KnownErrorPattern,

    /// An optional fix the user will be prompted to run.
    pub fix: Option<DoctorFixSpec>,
}

#[derive(Serialize, Deserialize, Debug, strum::Display, Clone, PartialEq, JsonSchema)]
pub enum KnownErrorKind {
    #[strum(serialize = "ScopeKnownError")]
    ScopeKnownError,
}

/// Resource used to define a `ScopeKnownError`.
/// A known error is a specific error that a user may run into.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Builder, JsonSchema)]
#[builder(setter(into))]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct V1AlphaKnownError {
    /// API version of the resource
    pub api_version: V1AlphaApiVersion,
    /// The type of resource.
    pub kind: KnownErrorKind,
    /// Standard set of options including name, description for the resource.
    /// Together `kind` and `metadata.name` are required to be unique. If there are duplicate, the
    /// resources "closest" to the execution dir will take precedence.
    pub metadata: ModelMetadata,
    /// Options for the resource.
    pub spec: KnownErrorSpec,
}

impl HelpMetadata for V1AlphaKnownError {
    fn metadata(&self) -> &ModelMetadata {
        &self.metadata
    }

    fn full_name(&self) -> String {
        format!("{}/{}", self.kind(), self.name())
    }
}

impl ScopeModel<KnownErrorSpec> for V1AlphaKnownError {
    fn api_version(&self) -> String {
        Self::int_api_version()
    }

    fn kind(&self) -> String {
        Self::int_kind()
    }

    fn spec(&self) -> &KnownErrorSpec {
        &self.spec
    }
}

impl InternalScopeModel<KnownErrorSpec, V1AlphaKnownError> for V1AlphaKnownError {
    fn int_api_version() -> String {
        V1AlphaApiVersion::ScopeV1Alpha.to_string()
    }

    fn int_kind() -> String {
        KnownErrorKind::ScopeKnownError.to_string()
    }

    #[cfg(test)]
    fn examples() -> Vec<String> {
        vec![
            "v1alpha/KnownError.yaml".to_string(),
            "v1alpha/KnownErrorMultiPattern.yaml".to_string(),
        ]
    }
}
