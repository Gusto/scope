use anyhow::Result;
use std::path::Path;

use crate::{
    prelude::{DoctorFixPromptSpec, DoctorFixSpec},
    shared::prelude::*,
};
use derive_builder::Builder;

use super::{RegexCaptures, substitute_templates_with_captures};

#[derive(Debug, PartialEq, Clone, Builder)]
#[builder(setter(into))]
pub struct DoctorFix {
    #[builder(default)]
    pub command: Option<DoctorCommands>,
    #[builder(default)]
    pub help_text: Option<String>,
    #[builder(default)]
    pub help_url: Option<String>,
    #[builder(default)]
    pub prompt: Option<DoctorFixPrompt>,
}

impl DoctorFix {
    pub fn empty() -> Self {
        DoctorFix {
            command: None,
            help_text: None,
            help_url: None,
            prompt: None,
        }
    }

    pub fn from_spec(containing_dir: &Path, working_dir: &str, fix: DoctorFixSpec) -> Result<Self> {
        let commands = DoctorCommands::from_commands(containing_dir, working_dir, &fix.commands)?;
        let help_text = fix
            .help_text
            .as_ref()
            .map(|st| st.trim().to_string())
            .clone();
        let help_url = fix.help_url.clone();
        let prompt = fix.prompt.map(DoctorFixPrompt::from);

        Ok(DoctorFix {
            command: Some(commands),
            help_text,
            help_url,
            prompt,
        })
    }

    /// Like `from_spec`, but also substitutes regex capture groups matched from a
    /// `ScopeKnownError` pattern into the fix's commands, help text, and prompt.
    pub fn from_spec_with_captures(
        containing_dir: &Path,
        working_dir: &str,
        fix: DoctorFixSpec,
        captures: &RegexCaptures,
    ) -> Result<Self> {
        let commands = DoctorCommands::from_commands_with_captures(
            containing_dir,
            working_dir,
            &fix.commands,
            captures,
        )?;
        let help_text = fix
            .help_text
            .as_ref()
            .map(|st| substitute_templates_with_captures(working_dir, st.trim(), captures))
            .transpose()?;
        let help_url = fix.help_url.clone();
        let prompt = fix
            .prompt
            .map(|p| DoctorFixPrompt::from_spec_with_captures(p, working_dir, captures))
            .transpose()?;

        Ok(DoctorFix {
            command: Some(commands),
            help_text,
            help_url,
            prompt,
        })
    }
}

#[derive(Debug, PartialEq, Clone, Builder)]
#[builder(setter(into))]
pub struct DoctorFixPrompt {
    #[builder(default)]
    pub text: String,
    #[builder(default)]
    pub extra_context: Option<String>,
}

impl From<DoctorFixPromptSpec> for DoctorFixPrompt {
    fn from(value: DoctorFixPromptSpec) -> Self {
        DoctorFixPrompt {
            text: value.text,
            extra_context: value.extra_context,
        }
    }
}

impl DoctorFixPrompt {
    fn from_spec_with_captures(
        value: DoctorFixPromptSpec,
        working_dir: &str,
        captures: &RegexCaptures,
    ) -> Result<Self> {
        let text = substitute_templates_with_captures(working_dir, &value.text, captures)?;
        let extra_context = value
            .extra_context
            .as_ref()
            .map(|ctx| substitute_templates_with_captures(working_dir, ctx, captures))
            .transpose()?;

        Ok(DoctorFixPrompt {
            text,
            extra_context,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_from_spec() {
        let spec = DoctorFixPromptSpec {
            text: "do you want to do the thing?".to_string(),
            extra_context: None,
        };

        let actual = DoctorFixPrompt::from(spec);

        assert_eq!(
            DoctorFixPrompt {
                text: "do you want to do the thing?".to_string(),
                extra_context: None
            },
            actual
        )
    }

    #[test]
    fn empty_returns_a_fix_full_of_none() {
        // I can argue that we should use Option<DoctorFix> instead,
        // but for now, this is where we're at.
        assert_eq!(
            DoctorFix {
                command: None,
                help_text: None,
                help_url: None,
                prompt: None,
            },
            DoctorFix::empty()
        )
    }

    #[test]
    fn from_spec_translates_to_fix() {
        let spec = DoctorFixSpec {
            commands: [
                "some/command",
                "./other_command",
                "{{ working_dir }}/.foo.sh",
            ]
            .iter()
            .map(|cmd| cmd.to_string())
            .collect(),
            help_text: Some("text".to_string()),
            help_url: Some("https.example.com".to_string()),
            prompt: Some(DoctorFixPromptSpec {
                text: "do you want to do the thing?".to_string(),
                extra_context: Some("additional context".to_string()),
            }),
        };

        let expected = DoctorFix {
            command: Some(
                DoctorCommands::from_commands(
                    Path::new("/some/dir"),
                    "/some/work/dir",
                    &spec.commands,
                )
                .unwrap(),
            ),
            help_text: spec.help_text.clone(),
            help_url: spec.help_url.clone(),
            prompt: Some(DoctorFixPrompt::from(spec.prompt.clone().unwrap())),
        };

        let actual = DoctorFix::from_spec(Path::new("/some/dir"), "/some/work/dir", spec).unwrap();

        assert_eq!(expected, actual)
    }

    #[test]
    fn from_spec_with_captures_substitutes_commands_help_and_prompt() {
        let mut named = std::collections::BTreeMap::new();
        named.insert("file".to_string(), "foo.sh".to_string());
        let captures = RegexCaptures {
            positional: vec![
                "permission denied: ./foo.sh".to_string(),
                "foo.sh".to_string(),
            ],
            named,
        };

        let spec = DoctorFixSpec {
            commands: vec!["sudo chmod +x {{ file }}".to_string()],
            help_text: Some("Run `chmod +x {{ captures[1] }}` and try again.".to_string()),
            help_url: None,
            prompt: Some(DoctorFixPromptSpec {
                text: "Fix {{ file }}?".to_string(),
                extra_context: Some("Matched: {{ captures[0] }}".to_string()),
            }),
        };

        let actual = DoctorFix::from_spec_with_captures(
            Path::new("/some/dir"),
            "/some/work/dir",
            spec,
            &captures,
        )
        .unwrap();

        assert_eq!(
            actual.command.unwrap().iter().next().unwrap().text(),
            "sudo chmod +x foo.sh"
        );
        assert_eq!(
            actual.help_text.unwrap(),
            "Run `chmod +x foo.sh` and try again."
        );
        let prompt = actual.prompt.unwrap();
        assert_eq!(prompt.text, "Fix foo.sh?");
        assert_eq!(
            prompt.extra_context.unwrap(),
            "Matched: permission denied: ./foo.sh"
        );
    }

    #[test]
    fn from_spec_with_captures_defaults_match_from_spec() {
        let spec = DoctorFixSpec {
            commands: vec!["./script.sh".to_string()],
            help_text: Some("some help".to_string()),
            help_url: None,
            prompt: None,
        };

        let expected =
            DoctorFix::from_spec(Path::new("/some/dir"), "/some/work/dir", spec.clone()).unwrap();
        let actual = DoctorFix::from_spec_with_captures(
            Path::new("/some/dir"),
            "/some/work/dir",
            spec,
            &RegexCaptures::default(),
        )
        .unwrap();

        assert_eq!(expected, actual)
    }

    #[test]
    fn from_spec_with_captures_handles_no_help_text_or_prompt() {
        let spec = DoctorFixSpec {
            commands: vec!["true".to_string()],
            help_text: None,
            help_url: None,
            prompt: None,
        };

        let actual = DoctorFix::from_spec_with_captures(
            Path::new("/some/dir"),
            "/some/work/dir",
            spec,
            &RegexCaptures::default(),
        )
        .unwrap();

        assert_eq!(actual.help_text, None);
        assert_eq!(actual.prompt, None);
    }

    #[test]
    fn from_spec_with_captures_handles_prompt_with_no_extra_context() {
        let spec = DoctorFixSpec {
            commands: vec!["true".to_string()],
            help_text: None,
            help_url: None,
            prompt: Some(DoctorFixPromptSpec {
                text: "do the thing?".to_string(),
                extra_context: None,
            }),
        };

        let actual = DoctorFix::from_spec_with_captures(
            Path::new("/some/dir"),
            "/some/work/dir",
            spec,
            &RegexCaptures::default(),
        )
        .unwrap();

        let prompt = actual.prompt.unwrap();
        assert_eq!(prompt.text, "do the thing?");
        assert_eq!(prompt.extra_context, None);
    }
}
