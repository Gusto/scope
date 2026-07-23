use anyhow::Result;
use derive_builder::Builder;
use std::path::Path;

use super::{RegexCaptures, extract_command_path, substitute_templates_with_captures};

#[derive(Debug, PartialEq, Clone, Builder)]
pub struct DoctorCommand {
    text: String,
}

impl DoctorCommand {
    pub fn try_new(containing_dir: &Path, working_dir: &str, command: &str) -> Result<Self> {
        Self::try_new_with_captures(
            containing_dir,
            working_dir,
            command,
            &RegexCaptures::default(),
        )
    }

    /// Like `try_new`, but also substitutes regex capture groups (e.g. `{{ captures[1] }}` or
    /// `{{ name }}` for a named group) matched from a `ScopeKnownError` pattern.
    pub fn try_new_with_captures(
        containing_dir: &Path,
        working_dir: &str,
        command: &str,
        captures: &RegexCaptures,
    ) -> Result<Self> {
        let cmd = substitute_templates_with_captures(working_dir, command, captures)?;
        let cmd = extract_command_path(containing_dir, &cmd);
        let cmd = Self::expand_command(&cmd);
        Ok(DoctorCommand::from_str(&cmd))
    }

    /// Performs no template rendering, path qualification, or shell expansion.
    /// This is useful for when you want to use a command as-is, without any modifications
    ///
    // We don't want to implement FromStr for DoctorCommand because we're not parsing a string into an instance.
    // Calling this with cmd.parse() doesn't make sense here.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(cmd: &str) -> Self {
        DoctorCommand {
            text: cmd.to_string(),
        }
    }

    /// Returns the command text
    pub fn text(&self) -> &str {
        &self.text
    }

    // keeping this to make it easier to do in a constructor later
    fn expand_command(command: &str) -> String {
        command
            .split(' ')
            //consider doing a full expansion that includes env vars?
            .map(|word| shellexpand::tilde(word))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, PartialEq, Clone, Builder)]
#[builder(setter(into))]
pub struct DoctorCommands {
    commands: Vec<DoctorCommand>,
}

impl DoctorCommands {
    pub fn from_commands(
        containing_dir: &Path,
        working_dir: &str,
        commands: &[String],
    ) -> Result<DoctorCommands> {
        Self::from_commands_with_captures(
            containing_dir,
            working_dir,
            commands,
            &RegexCaptures::default(),
        )
    }

    /// Like `from_commands`, but also substitutes regex capture groups matched from a
    /// `ScopeKnownError` pattern into each command.
    pub fn from_commands_with_captures(
        containing_dir: &Path,
        working_dir: &str,
        commands: &[String],
        captures: &RegexCaptures,
    ) -> Result<DoctorCommands> {
        commands
            .iter()
            .map(|command| {
                DoctorCommand::try_new_with_captures(containing_dir, working_dir, command, captures)
            })
            .collect::<Result<Vec<_>>>()
            .map(|commands| DoctorCommands { commands })
    }

    /// Returns an iterator over the commands
    pub fn iter(&self) -> std::slice::Iter<'_, DoctorCommand> {
        self.commands.iter()
    }
}

impl<'a> IntoIterator for &'a DoctorCommands {
    type Item = &'a DoctorCommand;
    type IntoIter = std::slice::Iter<'a, DoctorCommand>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
impl From<Vec<&str>> for DoctorCommands {
    /// This is only used by some tests and should NOT be used in production code
    /// because it does not properly pre-pend the command with a base path.
    fn from(value: Vec<&str>) -> Self {
        let commands = value
            .iter()
            .map(|cmd| DoctorCommand::from_str(cmd))
            .collect();
        Self { commands }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_vec_str() {
        let input = vec!["echo 'foo'", "false"];

        let actual = DoctorCommands::from(input.clone());

        let expected = DoctorCommands::from(vec!["echo 'foo'", "false"]);
        assert_eq!(expected, actual)
    }

    #[test]
    fn from_commands_inserts_execution_path() {
        let base_path = Path::new("/foo/bar");
        let input = ["echo 'foo'", "baz/qux", "./qux"];

        let actual = DoctorCommands::from_commands(
            base_path,
            "/some/working_dir",
            &input
                .iter()
                .map(|cmd| cmd.to_string())
                .collect::<Vec<String>>(),
        )
        .expect("Expected Ok");

        let expected = DoctorCommands::from(vec!["echo 'foo'", "baz/qux", "/foo/bar/qux"]);
        assert_eq!(expected, actual)
    }

    #[test]
    fn from_commands_inserts_working_dir() {
        let containing_dir = Path::new("/foo/bar");
        let working_dir = "/some/working_dir";
        let commands = ["{{ working_dir }}/foo.sh", "./bar.sh"]
            .iter()
            .map(|cmd| cmd.to_string())
            .collect::<Vec<String>>();

        let actual = DoctorCommands::from_commands(containing_dir, working_dir, &commands)
            .expect("Expected Ok");

        let expected = DoctorCommands::from(vec!["/some/working_dir/foo.sh", "/foo/bar/bar.sh"]);

        assert_eq!(expected, actual)
    }

    mod try_new_tests {
        use super::*;

        #[test]
        fn try_new_with_simple_command() {
            let containing_dir = Path::new("/foo/bar");
            let working_dir = "/some/working_dir";
            let command = "echo hello";

            let actual =
                DoctorCommand::try_new(containing_dir, working_dir, command).expect("Expected Ok");

            let expected = DoctorCommand::from_str("echo hello");
            assert_eq!(expected, actual);
        }

        #[test]
        fn try_new_with_working_dir_template() {
            let containing_dir = Path::new("/foo/bar");
            let working_dir = "/some/working_dir";
            let command = "{{ working_dir }}/script.sh";

            let actual =
                DoctorCommand::try_new(containing_dir, working_dir, command).expect("Expected Ok");

            let expected = DoctorCommand::from_str("/some/working_dir/script.sh");
            assert_eq!(expected, actual);
        }

        #[test]
        fn try_new_with_relative_path() {
            let containing_dir = Path::new("/foo/bar");
            let working_dir = "/some/working_dir";
            let command = "./script.sh";

            let actual =
                DoctorCommand::try_new(containing_dir, working_dir, command).expect("Expected Ok");

            let expected = DoctorCommand::from_str("/foo/bar/script.sh");
            assert_eq!(expected, actual);
        }

        #[test]
        fn try_new_with_relative_path_and_args() {
            let containing_dir = Path::new("/foo/bar");
            let working_dir = "/some/working_dir";
            let command = "./script.sh --verbose";

            let actual =
                DoctorCommand::try_new(containing_dir, working_dir, command).expect("Expected Ok");

            let expected = DoctorCommand::from_str("/foo/bar/script.sh --verbose");
            assert_eq!(expected, actual);
        }

        #[test]
        fn try_new_with_template_and_relative_path() {
            let containing_dir = Path::new("/project/root");
            let working_dir = "/build/dir";
            let command = "{{ working_dir }}/check.sh && ./validate.sh";

            let actual =
                DoctorCommand::try_new(containing_dir, working_dir, command).expect("Expected Ok");

            // extract_command_path only processes the first command/token,
            // not all relative paths in the entire command string
            // This is likely a BUG in the extract_command_path() implementation
            let expected = DoctorCommand::from_str("/build/dir/check.sh && ./validate.sh");
            assert_eq!(expected, actual);
        }

        #[test]
        fn try_new_with_absolute_path() {
            let containing_dir = Path::new("/foo/bar");
            let working_dir = "/some/working_dir";
            let command = "/usr/bin/env python3 test.py";

            let actual =
                DoctorCommand::try_new(containing_dir, working_dir, command).expect("Expected Ok");

            let expected = DoctorCommand::from_str("/usr/bin/env python3 test.py");
            assert_eq!(expected, actual);
        }

        #[test]
        fn try_new_with_unknown_template() {
            let containing_dir = Path::new("/foo/bar");
            let working_dir = "/some/working_dir";
            let command = "{{ unknown_template }}/script.sh";

            let actual =
                DoctorCommand::try_new(containing_dir, working_dir, command).expect("Expected Ok");

            // Unknown templates are erased (current behavior)
            // I can argue that this should error instead, but for now we document its behavior
            let expected = DoctorCommand::from_str("/script.sh");
            assert_eq!(expected, actual);
        }

        #[test]
        fn try_new_with_nested_relative_path() {
            let containing_dir = Path::new("/project/root");
            let working_dir = "/build/dir";
            let command = "./scripts/build.sh";

            let actual =
                DoctorCommand::try_new(containing_dir, working_dir, command).expect("Expected Ok");

            let expected = DoctorCommand::from_str("/project/root/scripts/build.sh");
            assert_eq!(expected, actual);
        }

        #[test]
        fn try_new_with_empty_command() {
            let containing_dir = Path::new("/foo/bar");
            let working_dir = "/some/working_dir";
            let command = "";

            let actual =
                DoctorCommand::try_new(containing_dir, working_dir, command).expect("Expected Ok");

            // Empty string split produces a single empty string, not an empty iterator
            // This case should probably return an error instead of an empty command
            // but for now we document its behavior
            let expected = DoctorCommand::from_str("");
            assert_eq!(expected, actual);
        }

        #[test]
        fn try_new_with_complex_command() {
            let containing_dir = Path::new("/project");
            let working_dir = "/work";
            let command = "cd {{ working_dir }} && ./build.sh && echo 'done'";

            let actual =
                DoctorCommand::try_new(containing_dir, working_dir, command).expect("Expected Ok");

            // extract_command_path only processes the first command/token,
            // not all relative paths in the entire command string
            // This is likely a BUG in the extract_command_path() implementation
            let expected = DoctorCommand::from_str("cd /work && ./build.sh && echo 'done'");
            assert_eq!(expected, actual);
        }

        #[test]
        fn try_new_with_multiple_templates() {
            let containing_dir = Path::new("/project");
            let working_dir = "/build";
            let command = "{{ working_dir }}/setup.sh && {{ working_dir }}/build.sh";

            let actual =
                DoctorCommand::try_new(containing_dir, working_dir, command).expect("Expected Ok");

            // Multiple templates should all be substituted
            let expected = DoctorCommand::from_str("/build/setup.sh && /build/build.sh");
            assert_eq!(expected, actual);
        }

        #[test]
        fn try_new_with_whitespace_only_command() {
            let containing_dir = Path::new("/foo/bar");
            let working_dir = "/some/working_dir";
            let command = "   ";

            let actual =
                DoctorCommand::try_new(containing_dir, working_dir, command).expect("Expected Ok");

            // Whitespace should be preserved
            let expected = DoctorCommand::from_str("   ");
            assert_eq!(expected, actual);
        }

        #[test]
        fn try_new_with_tilde_expansion() {
            let containing_dir = Path::new("/project");
            let working_dir = "/work";
            let command = "~/script.sh";

            let actual =
                DoctorCommand::try_new(containing_dir, working_dir, command).expect("Expected Ok");

            // Currently, the tilde is NOT expanded, but we want it to be
            let home_dir = std::env::var("HOME").expect("HOME environment variable should be set");
            let expected = DoctorCommand::from_str(&format!("{home_dir}/script.sh"));
            assert_eq!(expected, actual);
        }

        #[test]
        fn try_new_with_tilde_in_arguments() {
            let containing_dir = Path::new("/project");
            let working_dir = "/work";
            let command = "cp ~/source.txt ~/dest.txt";

            let actual =
                DoctorCommand::try_new(containing_dir, working_dir, command).expect("Expected Ok");

            // Currently, the tildes are NOT expanded, but we want them to be
            let home_dir = std::env::var("HOME").expect("HOME environment variable should be set");
            let expected =
                DoctorCommand::from_str(&format!("cp {home_dir}/source.txt {home_dir}/dest.txt"));
            assert_eq!(expected, actual);
        }
    }

    mod iterator_tests {
        use super::*;

        #[test]
        fn iter_returns_command_references() {
            let commands = DoctorCommands::from(vec!["echo hello", "ls -la", "pwd"]);

            let command_refs: Vec<&DoctorCommand> = commands.iter().collect();

            assert_eq!(command_refs.len(), 3);
            assert_eq!(command_refs[0].text(), "echo hello");
            assert_eq!(command_refs[1].text(), "ls -la");
            assert_eq!(command_refs[2].text(), "pwd");
        }

        #[test]
        fn into_iter_returns_command_references() {
            let commands = DoctorCommands::from(vec!["echo hello", "ls -la", "pwd"]);

            let command_refs: Vec<&DoctorCommand> = (&commands).into_iter().collect();

            assert_eq!(command_refs.len(), 3);
            assert_eq!(command_refs[0].text(), "echo hello");
            assert_eq!(command_refs[1].text(), "ls -la");
            assert_eq!(command_refs[2].text(), "pwd");
        }

        #[test]
        fn iter_works_with_for_loop() {
            let commands = DoctorCommands::from(vec!["echo hello", "ls -la", "pwd"]);
            let mut collected = Vec::new();

            for command in &commands {
                collected.push(command.text());
            }

            assert_eq!(collected, vec!["echo hello", "ls -la", "pwd"]);
        }

        #[test]
        fn iter_works_with_empty_commands() {
            let commands = DoctorCommands::from(vec![]);

            let command_refs: Vec<&DoctorCommand> = commands.iter().collect();

            assert_eq!(command_refs.len(), 0);
        }

        #[test]
        fn iter_works_with_single_command() {
            let commands = DoctorCommands::from(vec!["single command"]);

            let command_refs: Vec<&DoctorCommand> = commands.iter().collect();

            assert_eq!(command_refs.len(), 1);
            assert_eq!(command_refs[0].text(), "single command");
        }
    }
}
