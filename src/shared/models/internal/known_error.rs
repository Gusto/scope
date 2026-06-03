use std::path::Path;

use crate::models::HelpMetadata;
use crate::models::prelude::{ModelMetadata, V1AlphaKnownError};
use derivative::Derivative;
use regex::RegexSet;

use super::fix::DoctorFix;

#[derive(Derivative)]
#[derivative(PartialEq)]
#[derive(Debug, Clone)]
pub struct KnownError {
    pub full_name: String,
    pub metadata: ModelMetadata,
    #[derivative(PartialEq = "ignore")]
    pub regexes: RegexSet,
    pub help_text: String,
    pub fix: Option<DoctorFix>,
}

impl KnownError {
    /// Returns the original pattern strings in the order they were defined.
    pub fn patterns(&self) -> &[String] {
        self.regexes.patterns()
    }
}

impl HelpMetadata for KnownError {
    fn metadata(&self) -> &ModelMetadata {
        &self.metadata
    }

    fn full_name(&self) -> String {
        self.full_name.to_string()
    }
}

impl TryFrom<V1AlphaKnownError> for KnownError {
    type Error = anyhow::Error;

    fn try_from(value: V1AlphaKnownError) -> Result<Self, Self::Error> {
        let patterns = value.spec.pattern.patterns();
        if patterns.is_empty() {
            anyhow::bail!(
                "known error '{}' must have at least one pattern",
                value.full_name()
            );
        }
        // RegexSet::new accepts empty iterators, so the guard above is required.
        let regexes = RegexSet::new(&patterns)?;

        let binding = value.metadata.containing_dir();
        let containing_dir = Path::new(&binding);
        let working_dir = value
            .metadata
            .annotations
            .working_dir
            .as_ref()
            .unwrap()
            .clone();

        let maybe_fix = match value.spec.fix {
            Some(ref fix) => Some(DoctorFix::from_spec(
                containing_dir,
                &working_dir,
                fix.clone(),
            )?),
            None => None,
        };

        Ok(KnownError {
            full_name: value.full_name(),
            metadata: value.metadata,
            regexes,
            help_text: value.spec.help,
            fix: maybe_fix,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::{
        DoctorFixSpec, KnownErrorKind, KnownErrorPattern, KnownErrorSpec, ModelMetadataAnnotations,
        V1AlphaApiVersion, V1AlphaKnownError,
    };
    use crate::shared::models::parse_models_from_string;

    use std::collections::BTreeMap;
    use std::path::Path;

    fn make_metadata(
        name: &str,
        file_path: &str,
        file_dir: &str,
        working_dir: &str,
    ) -> ModelMetadata {
        ModelMetadata {
            name: name.to_string(),
            description: "some description".to_string(),
            annotations: ModelMetadataAnnotations {
                file_path: Some(file_path.to_string()),
                file_dir: Some(file_dir.to_string()),
                working_dir: Some(working_dir.to_string()),
                bin_path: None,
                extra: BTreeMap::new(),
            },
            labels: BTreeMap::new(),
        }
    }

    #[test]
    fn parses_single_pattern_string() {
        let text = "apiVersion: scope.github.com/v1alpha
kind: ScopeKnownError
metadata:
  name: error-exists
spec:
  description: Check if the word error is in the logs
  pattern: error
  help: The command had an error, try reading the logs around there to find out what happened.";

        let path = Path::new("/foo/bar/file.yaml");
        let work_dir = Path::new("/foo/bar");
        let configs = parse_models_from_string(work_dir, path, text).unwrap();
        assert_eq!(1, configs.len());
        let model = configs[0].get_known_error_spec().unwrap();

        assert_eq!("error-exists", model.metadata.name);
        assert_eq!("ScopeKnownError/error-exists", model.full_name);
        assert_eq!(
            "The command had an error, try reading the logs around there to find out what happened.",
            model.help_text
        );
        assert_eq!(["error"], model.patterns());
    }

    #[test]
    fn parses_list_of_patterns() {
        let text = "apiVersion: scope.github.com/v1alpha
kind: ScopeKnownError
metadata:
  name: multi-error
spec:
  pattern:
    - first error
    - second error
  help: Multiple patterns matched.";

        let path = Path::new("/foo/bar/file.yaml");
        let work_dir = Path::new("/foo/bar");
        let configs = parse_models_from_string(work_dir, path, text).unwrap();
        assert_eq!(1, configs.len());
        let model = configs[0].get_known_error_spec().unwrap();

        assert_eq!("multi-error", model.metadata.name);
        assert_eq!(["first error", "second error"], model.patterns());
        assert_eq!(2, model.regexes.len());
    }

    #[test]
    fn empty_pattern_list_is_rejected() {
        let metadata = make_metadata("bad-error", "/foo/bar/file.yaml", "/foo/bar", "/foo/bar");

        let input = V1AlphaKnownError {
            api_version: V1AlphaApiVersion::ScopeV1Alpha,
            kind: KnownErrorKind::ScopeKnownError,
            metadata,
            spec: KnownErrorSpec {
                help: "some help".to_string(),
                pattern: KnownErrorPattern::Many(vec![]),
                fix: None,
            },
        };

        let result = KnownError::try_from(input);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("at least one pattern"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn try_from_single_pattern_spec() {
        let model_metadata = make_metadata(
            "some test error",
            "/foo/bar/file.yaml",
            "/foo/bar",
            "/some/work/dir",
        );

        let input = V1AlphaKnownError {
            api_version: V1AlphaApiVersion::ScopeV1Alpha,
            kind: KnownErrorKind::ScopeKnownError,
            metadata: model_metadata.clone(),
            spec: KnownErrorSpec {
                help: "some help text".to_string(),
                pattern: KnownErrorPattern::Single("some regex pattern".to_string()),
                fix: Some(DoctorFixSpec {
                    commands: vec!["echo 'fix it!'".to_string()],
                    help_text: None,
                    help_url: None,
                    prompt: None,
                }),
            },
        };

        let actual = KnownError::try_from(input.clone()).unwrap();

        // regexes is PartialEq-ignored; check patterns explicitly.
        assert_eq!(["some regex pattern"], actual.patterns());
        assert_eq!(
            KnownError {
                full_name: "ScopeKnownError/some test error".to_string(),
                metadata: input.metadata,
                regexes: RegexSet::new(["some regex pattern"]).unwrap(),
                help_text: input.spec.help,
                fix: Some(
                    DoctorFix::from_spec(
                        Path::new(&model_metadata.containing_dir()),
                        &model_metadata.annotations.working_dir.unwrap(),
                        input.spec.fix.unwrap()
                    )
                    .unwrap()
                ),
            },
            actual
        )
    }

    #[test]
    fn try_from_multi_pattern_spec() {
        let model_metadata = make_metadata(
            "multi-error",
            "/foo/bar/file.yaml",
            "/foo/bar",
            "/some/work/dir",
        );

        let input = V1AlphaKnownError {
            api_version: V1AlphaApiVersion::ScopeV1Alpha,
            kind: KnownErrorKind::ScopeKnownError,
            metadata: model_metadata.clone(),
            spec: KnownErrorSpec {
                help: "some help text".to_string(),
                pattern: KnownErrorPattern::Many(vec![
                    "alpha error".to_string(),
                    "beta error".to_string(),
                ]),
                fix: None,
            },
        };

        let actual = KnownError::try_from(input).unwrap();

        assert_eq!(["alpha error", "beta error"], actual.patterns());
        assert_eq!(2, actual.regexes.len());
        assert!(actual.regexes.is_match("alpha error in output"));
        assert!(actual.regexes.is_match("beta error in output"));
        assert!(!actual.regexes.is_match("unrelated line"));
    }
}
