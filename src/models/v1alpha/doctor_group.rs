use derive_builder::Builder;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::models::core::ModelMetadata;
use crate::models::v1alpha::V1AlphaApiVersion;
use crate::models::{HelpMetadata, InternalScopeModel, ScopeModel};

/// What needs to be checked before the action will run. `paths` will be checked first, then
/// `commands`. If a `path` matches no files or the matching files have changed, the `command` will run.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct DoctorCheckSpec {
    /// A list of globs to check for changes. When the glob matches a new file, or the contents
    /// of the file change, the check will require a fix.
    ///
    /// Relative paths are relative to the scope config directory containing the config file.
    ///
    /// Shared configs can use the template string `{{ working_dir }}` to access the working
    /// directory.
    #[serde(default)]
    pub paths: Option<Vec<String>>,

    /// A list of commands to execute to check the environment.
    #[serde(default)]
    pub commands: Option<Vec<String>>,
}

/// Definition for fixing the environment.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct DoctorFixSpec {
    /// List of commands to run to fix the env.
    #[serde(default)]
    pub commands: Vec<String>,

    /// Text to display when no command is provided / fails to fix the env.
    #[serde(default)]
    pub help_text: Option<String>,

    /// Link to documentation to fix the issue.
    #[serde(default)]
    pub help_url: Option<String>,

    /// When present, user will be prompted for approval before running the fix
    #[serde(default)]
    pub prompt: Option<DoctorFixPromptSpec>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct DoctorFixPromptSpec {
    /// Yes/No question presented to the user
    #[serde(default)]
    pub text: String,

    /// Additional context for why they're being prompted for approval
    #[serde(default)]
    pub extra_context: Option<String>,
}

/// An action is a single step used to check in a group. This is most commonly used to build a
/// series of tasks for a system, like `ruby`, `python`, and databases.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct DoctorGroupActionSpec {
    /// Name of the "action" used when reporting status to the users.
    pub name: String,

    /// A description of this specific action, used for information to the users.
    pub description: Option<String>,

    /// The `check` run before `fix` (if provided). A check is used to determine if the fix needs
    /// to be executed, or fail the action if no fix is provided. If a fix is specified, the check
    /// will re-execute to ensure that the fix applied correctly.
    ///
    /// When omitted, there is no check and the fix will always run.
    pub check: Option<DoctorCheckSpec>,

    /// A fix defines how to fix the issue that a `check` is validating. When provided, will only
    /// run when the `check` "fails".
    pub fix: Option<DoctorFixSpec>,

    /// If false, the action is allowed to fail and let other actions in the group execute. Defaults
    /// to `true`.
    #[serde(default = "doctor_group_action_required_default")]
    pub required: bool,
}

fn doctor_group_action_required_default() -> bool {
    true
}

/// Often used to describe how to fix a "system", like `ruby`, `python`, or databases. Able to
/// depend on other "system".
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct DoctorGroupSpec {
    /// A list of `ScopeDoctorGroup` that are required for this group to execute. If not all finish
    /// successfully, this group will not execute.
    #[serde(default)]
    pub needs: Vec<String>,

    /// A series of steps to check and fix for the group.
    pub actions: Vec<DoctorGroupActionSpec>,

    /// Change how a group is handled when building the dependency task graph.
    /// When set to `when-required`, the group will be ignored unless it's required by another
    /// dependency.
    #[serde(default)]
    pub include: DoctorInclude,

    /// Defines conditions under which the group should be skipped.
    /// `skip` can be a boolean, in which case the group will be skipped if `true`.
    /// Alternatively, it can be a command that will be run to determine if the group should
    /// be skipped. If the command returns a zero exit code, the group will be skipped.
    /// If the command returns a non-zero exit code, the group will not be skipped.
    #[serde(default)]
    pub skip: SkipSpec,

    /// defines additional data that needs to be pulled from the system when reporting a bug.
    /// `reportExtraDetails` is a map of `string:string`, the value is a command that should be run.
    /// When a report is built, the commands will be run and automatically included in the report.
    #[serde(default)]
    pub report_extra_details: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(untagged)]
pub enum SkipSpec {
    Skip(bool),
    Command { command: String },
}

impl Default for SkipSpec {
    fn default() -> Self {
        SkipSpec::Skip(false)
    }
}

/// Configure how a groups will be used when determining the task graph.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DoctorInclude {
    /// Default option, the group will be included by default when determining which groups should
    /// run.
    #[default]
    ByDefault,
    /// Useful for shared configuration. The group will not run unless another group depends on it.
    WhenRequired,
}

#[derive(Serialize, Deserialize, Debug, strum::Display, Clone, PartialEq, JsonSchema)]
pub enum DoctorGroupKind {
    #[strum(serialize = "ScopeDoctorGroup")]
    ScopeDoctorGroup,
}

/// Resource used to define a `ScopeDoctorGroup`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Builder, JsonSchema)]
#[builder(setter(into))]
#[serde(rename_all = "camelCase")]
#[schemars(deny_unknown_fields)]
pub struct V1AlphaDoctorGroup {
    /// API version of the resource
    pub api_version: V1AlphaApiVersion,
    /// The type of resource.
    pub kind: DoctorGroupKind,
    /// Standard set of options including name, description for the resource.
    /// Together `kind` and `metadata.name` are required to be unique. If there are duplicate, the
    /// resources "closest" to the execution dir will take precedence.
    pub metadata: ModelMetadata,
    /// Options for the resource.
    pub spec: DoctorGroupSpec,
}

impl HelpMetadata for V1AlphaDoctorGroup {
    fn metadata(&self) -> &ModelMetadata {
        &self.metadata
    }

    fn full_name(&self) -> String {
        format!("{}/{}", self.kind(), self.name())
    }
}

impl ScopeModel<DoctorGroupSpec> for V1AlphaDoctorGroup {
    fn api_version(&self) -> String {
        Self::int_api_version()
    }

    fn kind(&self) -> String {
        Self::int_kind()
    }

    fn spec(&self) -> &DoctorGroupSpec {
        &self.spec
    }
}

impl InternalScopeModel<DoctorGroupSpec, V1AlphaDoctorGroup> for V1AlphaDoctorGroup {
    fn int_api_version() -> String {
        V1AlphaApiVersion::ScopeV1Alpha.to_string()
    }

    fn int_kind() -> String {
        DoctorGroupKind::ScopeDoctorGroup.to_string()
    }

    #[cfg(test)]
    fn examples() -> Vec<String> {
        vec!["v1alpha/DoctorGroup.yaml".to_string()]
    }
}
