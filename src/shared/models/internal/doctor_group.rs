use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use derive_builder::Builder;

use crate::models::HelpMetadata;
use crate::models::prelude::{ModelMetadata, V1AlphaDoctorGroup};
use crate::prelude::{DoctorGroupActionSpec, DoctorInclude, SkipSpec};
use crate::shared::models::internal::{DoctorCommands, DoctorFix};

use super::substitute_templates;

#[derive(Debug, PartialEq, Clone, Builder)]
#[builder(setter(into))]
pub struct DoctorGroupAction {
    pub name: String,
    pub description: String,
    pub fix: DoctorFix,
    pub check: DoctorGroupActionCheck,
    pub required: bool,
}

#[derive(Debug, PartialEq, Clone, Builder)]
#[builder(setter(into))]
pub struct DoctorGroupCachePath {
    pub paths: Vec<String>,
    pub base_path: PathBuf,
}

impl From<(&str, Vec<&str>)> for DoctorGroupCachePath {
    fn from(value: (&str, Vec<&str>)) -> Self {
        let pb = PathBuf::from(value.0);
        let paths = crate::shared::convert_to_string(value.1);

        Self {
            paths,
            base_path: pb,
        }
    }
}

#[derive(Debug, PartialEq, Clone, Builder)]
#[builder(setter(into))]
pub struct DoctorGroupActionCheck {
    pub command: Option<DoctorCommands>,
    pub files: Option<DoctorGroupCachePath>,
}

#[derive(Debug, PartialEq, Clone, Builder)]
#[builder(setter(into))]
pub struct DoctorGroup {
    pub full_name: String,
    pub metadata: ModelMetadata,
    pub requires: Vec<String>,
    pub run_by_default: bool,
    pub actions: Vec<DoctorGroupAction>,
    pub extra_report_args: BTreeMap<String, String>,
    #[builder(default)]
    pub skip: SkipSpec,
}

impl HelpMetadata for DoctorGroup {
    fn metadata(&self) -> &ModelMetadata {
        &self.metadata
    }

    fn full_name(&self) -> String {
        self.full_name.to_string()
    }
}

impl TryFrom<V1AlphaDoctorGroup> for DoctorGroup {
    type Error = anyhow::Error;

    fn try_from(model: V1AlphaDoctorGroup) -> Result<Self, Self::Error> {
        let mut actions: Vec<_> = Default::default();
        for spec_action in model.spec.actions.iter() {
            actions.push(parse_action(&model, spec_action)?);
        }

        Ok(DoctorGroup {
            full_name: model.full_name(),
            metadata: model.metadata,
            actions,
            requires: model.spec.needs,
            run_by_default: model.spec.include == DoctorInclude::ByDefault,
            extra_report_args: model.spec.report_extra_details,
            skip: model.spec.skip,
        })
    }
}

fn parse_action(
    group_model: &V1AlphaDoctorGroup,
    action: &DoctorGroupActionSpec,
) -> Result<DoctorGroupAction> {
    let binding = group_model.containing_dir();
    let containing_dir = Path::new(&binding);
    let working_dir = group_model
        .metadata
        .annotations
        .working_dir
        .as_ref()
        .unwrap()
        .clone();

    let spec_action = action.clone();

    let fix = match &spec_action.fix {
        Some(fix_spec) => DoctorFix::from_spec(containing_dir, &working_dir, fix_spec.clone())?,
        None => DoctorFix::empty(),
    };

    let check_spec = spec_action.check.unwrap_or_default();

    let check_command = match check_spec.commands {
        Some(ref check) => Some(DoctorCommands::from_commands(
            containing_dir,
            &working_dir,
            check,
        )?),
        None => None,
    };

    Ok(DoctorGroupAction {
        name: spec_action.name,
        required: spec_action.required,
        description: spec_action
            .description
            .unwrap_or_else(|| "default".to_string()),
        fix,
        check: DoctorGroupActionCheck {
            command: check_command,
            files: check_spec.paths.map(|paths| DoctorGroupCachePath {
                paths: paths
                    .iter() // TODO: should this be as_ref() still? Changed because type inference error
                    .map(|p| substitute_templates(working_dir.as_str(), p).unwrap()) // TODO: implement a function here, make it an early exit
                    .collect(),
                base_path: containing_dir.parent().unwrap().to_path_buf(),
            }),
        },
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::shared::models::parse_models_from_string;
    use crate::shared::models::prelude::{
        DoctorCommands, DoctorFix, DoctorGroupAction, DoctorGroupActionCheck,
    };
    use crate::shared::prelude::DoctorGroupCachePath;

    #[test]
    fn parse_group_1() {
        let test_file = format!("{}/examples/group-1.yaml", env!("CARGO_MANIFEST_DIR"));
        let work_dir = Path::new("/foo/bar");
        let text = std::fs::read_to_string(test_file).unwrap();
        let path = Path::new("/foo/bar/.scope/file.yaml");
        let configs = parse_models_from_string(work_dir, path, &text).unwrap();
        assert_eq!(1, configs.len());

        let dg = configs[0].get_doctor_group().unwrap();
        assert_eq!("foo", dg.metadata.name);
        assert_eq!(3, dg.actions.len());
        assert_eq!(
            "/foo/bar/.scope/file.yaml",
            dg.metadata.annotations.file_path.unwrap()
        );
        assert_eq!("/foo/bar/.scope", dg.metadata.annotations.file_dir.unwrap());
        assert_eq!("ScopeDoctorGroup/foo", dg.full_name);
        assert_eq!(vec!["bar"], dg.requires);

        assert_eq!(
            dg.actions[0],
            DoctorGroupAction {
                name: "action1".to_string(),
                required: false,
                description: "foo1".to_string(),
                fix: DoctorFix {
                    command: Some(DoctorCommands::from(vec!["/foo/bar/.scope/fix1.sh"])),
                    prompt: None,
                    help_text: Some("There is a good way to fix this, maybe...".to_string()),
                    help_url: Some("https://go.example.com/fixit".to_string()),
                },
                check: DoctorGroupActionCheck {
                    command: Some(DoctorCommands::from(vec!["/foo/bar/.scope/foo1.sh"])),
                    files: Some(DoctorGroupCachePath::from((
                        "/foo/bar",
                        vec!["flig/bar/**/*"]
                    )))
                }
            }
        );
        assert_eq!(
            dg.actions[1],
            DoctorGroupAction {
                name: "action2".to_string(),
                required: true,
                description: "foo2".to_string(),
                fix: DoctorFix {
                    command: None,
                    help_text: None,
                    help_url: None,
                    prompt: None,
                },
                check: DoctorGroupActionCheck {
                    command: Some(DoctorCommands::from(vec!["sleep infinity"])),
                    files: Some(DoctorGroupCachePath::from(("/foo/bar", vec!["*/*.txt"])))
                }
            }
        );
        assert_eq!(
            dg.actions[2],
            DoctorGroupAction {
                name: "action3".to_string(),
                required: true,
                description: "foo3".to_string(),
                fix: DoctorFix {
                    command: Some(DoctorCommands::from(vec!["/foo/bar/.scope/fix3.sh"])),
                    prompt: None,
                    help_text: None,
                    help_url: None,
                },
                check: DoctorGroupActionCheck {
                    command: None,
                    files: None,
                }
            }
        );
    }

    /// Regression test for https://github.com/Gusto/scope/issues/153: `check` becoming
    /// `Option<DoctorCheckSpec>` must not change how an explicit, empty `check: {}` (the
    /// pre-existing "always run the fix" idiom) is parsed.
    #[test]
    fn omitted_check_and_empty_check_parse_identically() {
        let work_dir = Path::new("/foo/bar");
        let path = Path::new("/foo/bar/.scope/file.yaml");

        let render = |check_yaml: &str| {
            let text = format!(
                r#"
apiVersion: scope.github.com/v1alpha
kind: ScopeDoctorGroup
metadata:
  name: foo
spec:
  actions:
    - name: action1
{check_yaml}      fix:
        commands:
          - ./fix1.sh
"#
            );
            let configs = parse_models_from_string(work_dir, path, &text).unwrap();
            configs[0].get_doctor_group().unwrap().actions[0]
                .check
                .clone()
        };

        let omitted = render("");
        let empty = render("      check: {}\n");
        let expected = DoctorGroupActionCheck {
            command: None,
            files: None,
        };

        assert_eq!(expected, omitted);
        assert_eq!(expected, empty);
    }
}
