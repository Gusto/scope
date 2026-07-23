use std::path::Path;

use crate::models::HelpMetadata;
use crate::models::prelude::{DoctorFixSpec, ModelMetadata, V1AlphaKnownError};
use derivative::Derivative;
use regex::Regex;

use super::RegexCaptures;
use super::fix::DoctorFix;

/// Template variable names reserved for `working_dir` and positional captures; a named capture
/// group using one of these would otherwise silently shadow the reserved variable when
/// substituted into a fix's commands/help/prompt.
const RESERVED_CAPTURE_NAMES: [&str; 2] = ["working_dir", "captures"];

#[derive(Derivative)]
#[derivative(PartialEq)]
#[derive(Debug, Clone)]
pub struct KnownError {
    pub full_name: String,
    pub metadata: ModelMetadata,
    #[derivative(PartialEq = "ignore")]
    pub regexes: Vec<Regex>,
    pub help_text: String,
    pub fix: Option<DoctorFixSpec>,
}

impl KnownError {
    /// Returns the original pattern strings in the order they were defined.
    pub fn patterns(&self) -> Vec<String> {
        self.regexes
            .iter()
            .map(|r| r.as_str().to_string())
            .collect()
    }

    /// Returns the capture groups (positional and named) of the first pattern that matches
    /// `line`, if any, ready to substitute into fix commands / help text templates.
    /// When multiple patterns are defined, the first one (in definition order) that matches wins.
    pub fn find_match(&self, line: &str) -> Option<RegexCaptures> {
        self.regexes.iter().find_map(|r| {
            let caps = r.captures(line)?;
            let positional = caps
                .iter()
                .map(|m| m.map(|m| m.as_str().to_string()).unwrap_or_default())
                .collect();
            let named = r
                .capture_names()
                .flatten()
                .filter_map(|name| {
                    caps.name(name)
                        .map(|m| (name.to_string(), m.as_str().to_string()))
                })
                .collect();
            Some(RegexCaptures { positional, named })
        })
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
        let regexes = patterns
            .iter()
            .map(|p| Regex::new(p))
            .collect::<Result<Vec<_>, _>>()?;

        for regex in &regexes {
            for name in regex.capture_names().flatten() {
                if RESERVED_CAPTURE_NAMES.contains(&name) {
                    anyhow::bail!(
                        "known error '{}' has a pattern with a capture group named '{}', which is reserved for template substitution",
                        value.full_name(),
                        name
                    );
                }
            }
        }

        let binding = value.metadata.containing_dir();
        let containing_dir = Path::new(&binding);
        let working_dir = value
            .metadata
            .annotations
            .working_dir
            .as_ref()
            .expect("model metadata should always have a working_dir set during config loading")
            .clone();

        // The fix (if any) isn't built here: its commands, help text, and prompt may reference
        // capture groups from `pattern`, which aren't known until a line actually matches. The
        // raw spec is turned into a `DoctorFix` at match time instead (see
        // `DoctorFix::from_spec_with_captures`). It's still validated here (with no captures
        // available) so a broken fix template fails fast at config-load time rather than only
        // surfacing once a matching line is intercepted.
        if let Some(ref fix) = value.spec.fix {
            DoctorFix::from_spec_with_captures(
                containing_dir,
                &working_dir,
                fix.clone(),
                &RegexCaptures::default(),
            )?;
        }

        Ok(KnownError {
            full_name: value.full_name(),
            metadata: value.metadata,
            regexes,
            help_text: value.spec.help,
            fix: value.spec.fix,
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
        assert_eq!(model.patterns(), ["error"]);
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
        assert_eq!(model.patterns(), ["first error", "second error"]);
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
        assert_eq!(actual.patterns(), ["some regex pattern"]);
        assert_eq!(
            KnownError {
                full_name: "ScopeKnownError/some test error".to_string(),
                metadata: input.metadata,
                regexes: vec![Regex::new("some regex pattern").unwrap()],
                help_text: input.spec.help,
                fix: input.spec.fix,
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

        assert_eq!(actual.patterns(), ["alpha error", "beta error"]);
        assert_eq!(2, actual.regexes.len());
        assert!(actual.find_match("alpha error in output").is_some());
        assert!(actual.find_match("beta error in output").is_some());
        assert!(actual.find_match("unrelated line").is_none());
    }

    #[test]
    fn find_match_exposes_positional_and_named_capture_groups() {
        let model_metadata = make_metadata(
            "not-executable",
            "/foo/bar/file.yaml",
            "/foo/bar",
            "/some/work/dir",
        );

        let input = V1AlphaKnownError {
            api_version: V1AlphaApiVersion::ScopeV1Alpha,
            kind: KnownErrorKind::ScopeKnownError,
            metadata: model_metadata,
            spec: KnownErrorSpec {
                help: "The script is not executable.".to_string(),
                pattern: KnownErrorPattern::Single(
                    r"permission denied: \./(?<file>.*)".to_string(),
                ),
                fix: None,
            },
        };

        let actual = KnownError::try_from(input).unwrap();

        let caps = actual
            .find_match("zsh: permission denied: ./foo.sh")
            .expect("expected a match");

        assert_eq!(caps.positional[0], "permission denied: ./foo.sh");
        assert_eq!(caps.positional[1], "foo.sh");
        assert_eq!(caps.named.get("file").unwrap(), "foo.sh");
        assert!(actual.find_match("no match here").is_none());
    }

    #[test]
    fn reserved_capture_group_names_are_rejected() {
        for reserved in ["working_dir", "captures"] {
            let model_metadata = make_metadata(
                "bad-capture-name",
                "/foo/bar/file.yaml",
                "/foo/bar",
                "/some/work/dir",
            );

            let input = V1AlphaKnownError {
                api_version: V1AlphaApiVersion::ScopeV1Alpha,
                kind: KnownErrorKind::ScopeKnownError,
                metadata: model_metadata,
                spec: KnownErrorSpec {
                    help: "some help".to_string(),
                    pattern: KnownErrorPattern::Single(format!("error: (?<{reserved}>.*)")),
                    fix: None,
                },
            };

            let result = KnownError::try_from(input);
            let msg = result.unwrap_err().to_string();
            assert!(
                msg.contains("reserved"),
                "expected a 'reserved' error for capture name '{reserved}', got: {msg}"
            );
        }
    }

    #[test]
    fn broken_fix_template_is_rejected_at_load_time() {
        let model_metadata = make_metadata(
            "broken-fix",
            "/foo/bar/file.yaml",
            "/foo/bar",
            "/some/work/dir",
        );

        let input = V1AlphaKnownError {
            api_version: V1AlphaApiVersion::ScopeV1Alpha,
            kind: KnownErrorKind::ScopeKnownError,
            metadata: model_metadata,
            spec: KnownErrorSpec {
                help: "some help".to_string(),
                pattern: KnownErrorPattern::Single("some regex pattern".to_string()),
                fix: Some(DoctorFixSpec {
                    commands: vec!["echo {{ unterminated".to_string()],
                    help_text: None,
                    help_url: None,
                    prompt: None,
                }),
            },
        };

        // The broken template must fail here, at config-load time, rather than only
        // surfacing later when a matching line is actually intercepted.
        assert!(KnownError::try_from(input).is_err());
    }
}
