use colored::Colorize;

use crate::models::HelpMetadata;
use crate::report_stdout;
use std::cmp::max;
use std::path::Path;

mod capture;
mod config_load;
mod logging;

pub mod analyze;
pub mod directories;
mod models;
mod redact;
mod report;

pub const CONFIG_FILE_PATH_ENV: &str = "SCOPE_CONFIG_JSON";
pub const RUN_ID_ENV_VAR: &str = "SCOPE_RUN_ID";

pub mod prelude {
    pub use super::capture::{
        CaptureError, CaptureOpts, DefaultExecutionProvider, ExecutionProvider,
        MockExecutionProvider, OutputCapture, OutputCaptureBuilder, OutputDisplay,
    };
    pub use super::config_load::{ConfigOptions, FoundConfig, build_config_path};
    pub use super::logging::{LoggingOpts, STDERR_WRITER, STDOUT_WRITER, progress_bar_without_pos};
    pub use super::models::prelude::*;
    pub use super::report::{
        ActionReport, ActionReportBuilder, ActionTaskReport, ActionTaskReportBuilder,
        DefaultGroupedReportBuilder, DefaultUnstructuredReportBuilder, GroupReport,
        GroupedReportBuilder, Report, ReportRenderer, UnstructuredReportBuilder,
    };
    pub use super::{CONFIG_FILE_PATH_ENV, RUN_ID_ENV_VAR};
    pub use super::{ExtraColumn, print_details, print_details_with_column};
}

pub(crate) fn convert_to_string(input: Vec<&str>) -> Vec<String> {
    input.iter().map(|x| x.to_string()).collect()
}

/// Header and per-row value extractor for an optional extra column in [`print_details_with_column`].
pub type ExtraColumn<'a, T> = (&'a str, &'a dyn Fn(&T) -> String);

pub async fn print_details<T>(working_dir: &Path, config: &[T])
where
    T: HelpMetadata,
{
    print_details_with_column(working_dir, config, None::<ExtraColumn<'_, T>>).await;
}

/// Same rendering as [`print_details`], with an optional extra column (header + per-row value)
/// inserted between `Description` and `Path`.
pub async fn print_details_with_column<T>(
    working_dir: &Path,
    config: &[T],
    extra_column: Option<ExtraColumn<'_, T>>,
) where
    T: HelpMetadata,
{
    let max_name_length = config
        .iter()
        .map(|x| x.full_name().len())
        .max()
        .unwrap_or(20);
    let max_name_length = max(max_name_length, 20) + 2;

    match extra_column {
        Some((header, _)) => {
            report_stdout!(
                "  {:max_name_length$}{:60}{:10}{}",
                "Name".white().bold(),
                "Description".white().bold(),
                header.white().bold(),
                "Path".white().bold()
            );
        }
        None => {
            report_stdout!(
                "  {:max_name_length$}{:60}{}",
                "Name".white().bold(),
                "Description".white().bold(),
                "Path".white().bold()
            );
        }
    }

    for resource in config {
        let description = format_description(&resource.description());
        let loc = format_location(&resource.metadata().file_path(), working_dir);

        match &extra_column {
            Some((_, extractor)) => {
                report_stdout!(
                    "- {:max_name_length$}{:60}{:10}{}",
                    resource.full_name(),
                    description,
                    extractor(resource),
                    loc
                );
            }
            None => {
                report_stdout!(
                    "- {:max_name_length$}{:60}{}",
                    resource.full_name(),
                    description,
                    loc
                );
            }
        }
    }
}

/// Truncates `description` to 55 bytes (on a char boundary) with a trailing `...` when it's
/// longer than that; returned as-is otherwise.
fn format_description(description: &str) -> String {
    if description.len() > 55 {
        format!("{}...", truncate_at_char_boundary(description, 55))
    } else {
        description.to_string()
    }
}

/// Renders a resource's file path relative to `working_dir` when possible. When
/// `pathdiff::diff_paths` can't relativize it (e.g. `file_path` is relative but `working_dir`
/// is absolute), falls back to the last 35 bytes (on a char boundary) prefixed with `...` if
/// longer than that, or the path unchanged otherwise.
fn format_location(file_path: &str, working_dir: &Path) -> String {
    match pathdiff::diff_paths(file_path, working_dir) {
        Some(diff) => diff.display().to_string(),
        None if file_path.len() > 35 => {
            format!("...{}", suffix_at_char_boundary(file_path, 35))
        }
        None => file_path.to_string(),
    }
}

/// Returns the longest prefix of `s` that is at most `max_bytes` long and ends on a char
/// boundary, so multi-byte UTF-8 characters are never split.
fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Returns the shortest suffix of `s` that is at least `s.len() - max_bytes` bytes long and
/// starts on a char boundary, so multi-byte UTF-8 characters are never split.
fn suffix_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut start = s.len() - max_bytes;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::prelude::{ModelMetadata, ModelMetadataAnnotations, ModelMetadataBuilder};
    use std::collections::BTreeMap;

    struct FakeResource {
        metadata: ModelMetadata,
    }

    impl HelpMetadata for FakeResource {
        fn metadata(&self) -> &ModelMetadata {
            &self.metadata
        }

        fn full_name(&self) -> String {
            "Fake/resource".to_string()
        }
    }

    fn fake_resource(description: &str, file_path: &str) -> FakeResource {
        let metadata = ModelMetadataBuilder::default()
            .name("fake")
            .description(description)
            .annotations(ModelMetadataAnnotations {
                file_path: Some(file_path.to_string()),
                ..Default::default()
            })
            .labels(BTreeMap::default())
            .build()
            .unwrap();
        FakeResource { metadata }
    }

    #[tokio::test]
    async fn print_details_truncates_multibyte_description_without_panicking() {
        let description = format!("{}é{}", "a".repeat(54), "b".repeat(20));
        let resource = fake_resource(&description, "/short/path.yaml");

        print_details(Path::new("/tmp"), &[resource]).await;
    }

    #[tokio::test]
    async fn print_details_truncates_multibyte_path_without_panicking() {
        let relative_path = format!("{}é{}", "a".repeat(4), "b".repeat(34));
        let resource = fake_resource("a description", &relative_path);

        print_details(Path::new("/tmp/working"), &[resource]).await;
    }

    #[tokio::test]
    async fn print_details_keeps_short_relative_path_as_is() {
        // pathdiff::diff_paths returns None when `path` is relative but `base` is absolute, so
        // this exercises the fallback branch that leaves `loc` untouched (it's already short
        // enough to not need the "..." truncation either).
        let resource = fake_resource("a description", "short.yaml");

        print_details(Path::new("/tmp/working"), &[resource]).await;
    }

    #[test]
    fn format_description_leaves_55_bytes_unchanged() {
        let description = "a".repeat(55);

        assert_eq!(format_description(&description), description);
    }

    #[test]
    fn format_description_truncates_56_bytes_with_ellipsis() {
        let description = "a".repeat(56);

        assert_eq!(
            format_description(&description),
            format!("{}...", "a".repeat(55))
        );
    }

    #[test]
    fn format_location_relativizes_when_possible() {
        assert_eq!(
            format_location("/tmp/working/child/group.yaml", Path::new("/tmp/working")),
            "child/group.yaml"
        );
    }

    #[test]
    fn format_location_leaves_35_byte_unrelativizable_path_unchanged() {
        let file_path = "a".repeat(35);

        assert_eq!(
            format_location(&file_path, Path::new("/tmp/working")),
            file_path
        );
    }

    #[test]
    fn format_location_truncates_36_byte_unrelativizable_path_with_ellipsis() {
        let file_path = "a".repeat(36);

        assert_eq!(
            format_location(&file_path, Path::new("/tmp/working")),
            format!("...{}", "a".repeat(35))
        );
    }

    #[test]
    fn truncate_at_char_boundary_never_splits_a_multibyte_char() {
        let description = format!("{}é{}", "a".repeat(54), "b".repeat(20));

        assert_eq!(truncate_at_char_boundary(&description, 55), "a".repeat(54));
    }

    #[test]
    fn truncate_at_char_boundary_returns_input_unchanged_when_within_limit() {
        assert_eq!(truncate_at_char_boundary("short", 55), "short");
    }

    #[test]
    fn suffix_at_char_boundary_never_splits_a_multibyte_char() {
        let path = format!("{}é{}", "a".repeat(4), "b".repeat(34));

        // The raw byte offset (path.len() - 35 = 5) lands inside "é" (bytes 4..6), so the
        // returned suffix rounds forward to the next boundary rather than splitting the char.
        assert_eq!(suffix_at_char_boundary(&path, 35), "b".repeat(34));
    }

    #[test]
    fn suffix_at_char_boundary_returns_input_unchanged_when_within_limit() {
        assert_eq!(suffix_at_char_boundary("short", 35), "short");
    }
}
