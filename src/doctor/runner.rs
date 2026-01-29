use super::check::{ActionRunResult, ActionRunStatus, DoctorActionRun};
use crate::doctor::check::RuntimeError;
use crate::internal::prompts::UserInteraction;
use crate::models::HelpMetadata;
use crate::models::prelude::SkipSpec;
use crate::shared::prelude::{
    ActionReport, ActionTaskReport, CaptureOpts, DoctorGroup, ExecutionProvider, GroupReport,
    OutputDestination, generate_env_vars, progress_bar_without_pos,
};
use anyhow::Result;
use colored::Colorize;
use opentelemetry::trace::Status;
use petgraph::dot::{Config, Dot};
use petgraph::prelude::*;
use petgraph::visit::{DfsPostOrder, Walker};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{Instrument, Span, debug, error, info, info_span, warn};
use tracing_indicatif::span_ext::IndicatifSpanExt;
use tracing_opentelemetry::OpenTelemetrySpanExt;

#[derive(Debug)]
pub struct PathRunResult {
    pub did_succeed: bool,
    pub succeeded_groups: BTreeSet<String>,
    pub failed_group: BTreeSet<String>,
    pub skipped_group: BTreeSet<String>,
    pub group_reports: Vec<GroupReport>,
}

impl Display for PathRunResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut output = Vec::new();
        output.push(format!(
            "{} groups {}",
            self.succeeded_groups.len(),
            "succeeded".bold()
        ));
        if !self.failed_group.is_empty() {
            output.push(format!(
                "{} groups {}",
                self.failed_group.len(),
                "failed".bold().red()
            ));
        }
        if !self.skipped_group.is_empty() {
            output.push(format!(
                "{} groups {}",
                self.skipped_group.len(),
                "skipped".bold().yellow()
            ));
        }

        write!(f, "{}", output.join(", "))
    }
}

impl PathRunResult {
    fn process(&mut self, group: &GroupExecutionResult) {
        let group_name = group.group_name.to_string();

        match group.status {
            GroupExecutionStatus::Succeeded => {
                self.succeeded_groups.insert(group_name);
            }
            GroupExecutionStatus::Failed => {
                self.failed_group.insert(group_name);
                self.did_succeed = false;
            }
            GroupExecutionStatus::Skipped => {
                self.skipped_group.insert(group_name);
                self.did_succeed = false; // User-denied fixes should cause failure
            }
            GroupExecutionStatus::GroupSkipped => {
                self.skipped_group.insert(group_name);
                // Note: Group skips via configuration do not cause the overall command to fail
            }
        };

        self.group_reports.push(group.group_report.clone());
    }
}

#[derive(Debug)]
enum GroupExecutionStatus {
    Succeeded,
    Failed,
    Skipped,
    GroupSkipped,
}

#[derive(Debug)]
struct GroupExecutionResult {
    group_name: String,
    status: GroupExecutionStatus,
    skip_remaining: bool,
    group_report: GroupReport,
}

pub struct GroupActionContainer<T>
where
    T: DoctorActionRun,
{
    pub group: DoctorGroup,
    pub actions: Vec<T>,
    pub exec_provider: Arc<dyn ExecutionProvider>,
    pub exec_working_dir: PathBuf,
    pub sys_path: String,
}

impl<T> GroupActionContainer<T>
where
    T: DoctorActionRun,
{
    pub fn new(
        group: DoctorGroup,
        actions: Vec<T>,
        exec_provider: Arc<dyn ExecutionProvider>,
        exec_working_dir: PathBuf,
        sys_path: String,
    ) -> Self {
        Self {
            group: group.clone(),
            actions,
            exec_provider,
            exec_working_dir,
            sys_path,
        }
    }

    pub fn group_name(&self) -> &str {
        &self.group.metadata.name
    }

    pub fn additional_report_details(&self) -> &BTreeMap<String, String> {
        &self.group.extra_report_args
    }

    pub async fn execute_command(&self, command: &str) -> Result<String> {
        Ok(self
            .exec_provider
            .run_for_output(&self.sys_path, &self.exec_working_dir, command)
            .await)
    }

    pub async fn should_skip_group(&self) -> Result<bool, RuntimeError> {
        match &self.group.skip {
            SkipSpec::Skip(should_skip) => Ok(*should_skip),
            SkipSpec::Command { command } => {
                let args = vec![command.clone()];
                let path = format!(
                    "{}:{}",
                    self.group.metadata().containing_dir(),
                    self.group.metadata().exec_path()
                );

                let output = self
                    .exec_provider
                    .run_command(CaptureOpts {
                        working_dir: &self.exec_working_dir,
                        args: &args,
                        output_dest: OutputDestination::Logging,
                        path: &path,
                        env_vars: generate_env_vars(),
                    })
                    .await?;

                // Skip if command returns success (exit code 0)
                Ok(output.exit_code == Some(0))
            }
        }
    }
}

pub struct RunGroups<T>
where
    T: DoctorActionRun,
{
    pub(crate) group_actions: BTreeMap<String, GroupActionContainer<T>>,
    pub(crate) all_paths: Vec<String>,
    pub(crate) yolo: bool,
}

impl<T> RunGroups<T>
where
    T: DoctorActionRun,
{
    pub async fn execute(&self) -> Result<PathRunResult> {
        let mut full_path = Vec::new();
        for path in &self.all_paths {
            if let Some(group_container) = self.group_actions.get(path) {
                full_path.push(group_container);
            }
        }

        self.run_path(full_path).await
    }

    async fn run_path(&self, groups: Vec<&GroupActionContainer<T>>) -> Result<PathRunResult> {
        let header_span = info_span!("doctor run", "indicatif.pb_show" = true);
        header_span.pb_set_length(self.all_paths.len() as u64);
        header_span.pb_set_message("scope doctor run");

        let _span = header_span.enter();

        let mut skip_remaining = false;
        let mut run_result = PathRunResult {
            did_succeed: true,
            succeeded_groups: BTreeSet::new(),
            failed_group: BTreeSet::new(),
            skipped_group: BTreeSet::new(),
            group_reports: Vec::new(),
        };

        for group_container in groups {
            let group_name = group_container.group_name();
            header_span.pb_inc(1);
            debug!(target: "user", "Running check {}", group_name);

            if skip_remaining {
                run_result.skipped_group.insert(group_name.to_string());
                continue;
            }

            let group_span = info_span!(
                parent: &header_span,
                "group",
                "indicatif.pb_show" = true,
                "group.name" = group_name,
                "otel.name" = format!("group {}", group_name)
            );
            group_span.pb_set_length(group_container.actions.len() as u64);
            group_span.pb_set_message(&format!("group {group_name}"));
            let _span = group_span.enter();

            let group_result = self.execute_group(&group_span, group_container).await?;
            if let GroupExecutionStatus::Failed = group_result.status {
                group_span.set_status(Status::Error {
                    description: std::borrow::Cow::Owned(format!(
                        "{} group failed",
                        group_result.group_name
                    )),
                });
            }

            run_result.process(&group_result);

            skip_remaining |= group_result.skip_remaining;
        }

        Ok(run_result)
    }

    async fn execute_group(
        &self,
        group_span: &Span,
        container: &GroupActionContainer<T>,
    ) -> Result<GroupExecutionResult> {
        let mut results = GroupExecutionResult {
            group_name: container.group_name().to_string(),
            skip_remaining: false,
            status: GroupExecutionStatus::Succeeded,
            group_report: GroupReport::new(container.group_name()),
        };

        // Check if the group should be skipped
        if container.should_skip_group().await? {
            warn!(target: "always", "Group skipped, group: \"{}\"", container.group_name());
            results.status = GroupExecutionStatus::GroupSkipped;
            return Ok(results);
        }

        for action in &container.actions {
            group_span.pb_inc(1);
            if results.skip_remaining {
                info!(target: "user", "Check `{}/{}` was skipped.", container.group_name().bold(), action.name());
                continue;
            }

            let action_span = info_span!(
                parent: group_span,
                "action",
                "indicatif.pb_show" = true,
                "group.name" = container.group_name(),
                "action.name" = action.name(),
                "otel.name" = format!("action {}", action.name())
            );
            action_span.pb_set_message(&format!(
                "action {} - {}",
                action.name(),
                action.description()
            ));
            action_span.pb_set_style(&progress_bar_without_pos());

            let prompt_fn = if self.yolo { auto_approve } else { prompt_user };
            let action_result = action
                .run_action(prompt_fn)
                .instrument(action_span.clone())
                .await?;

            if action_result.status.is_failure() {
                action_span.set_status(Status::Error {
                    description: std::borrow::Cow::Owned(format!(
                        "{} action failed",
                        action_result.action_name
                    )),
                });
            }

            results
                .group_report
                .add_action(&action_result.action_report);

            // ignore the result, because reporting shouldn't cause app to crash
            report_action_output(container.group_name(), action, &action_result)
                .await
                .ok();

            results.status = match action_result.status {
                ActionRunStatus::CheckSucceeded
                | ActionRunStatus::NoCheckFixSucceeded
                | ActionRunStatus::CheckFailedFixSucceedVerifySucceed => {
                    GroupExecutionStatus::Succeeded
                }
                ActionRunStatus::CheckFailedFixUserDenied => GroupExecutionStatus::Skipped,
                _ => GroupExecutionStatus::Failed,
            };

            results.skip_remaining = match action_result.status {
                ActionRunStatus::CheckSucceeded
                | ActionRunStatus::NoCheckFixSucceeded
                | ActionRunStatus::CheckFailedFixSucceedVerifySucceed => false,
                ActionRunStatus::CheckFailedFixFailedStop => true,
                _ => action.required(),
            };
        }

        for (name, command) in container.additional_report_details() {
            let output = container.execute_command(command).await.ok();
            results.group_report.add_additional_details(
                name,
                command,
                &output.unwrap_or_else(|| "Unable to capture output".to_string()),
            );
        }

        Ok(results)
    }
}

/// Prompt the user for confirmation using the inquire crate.
///
/// This function wraps inquire::Confirm and handles TTY detection.
/// It's used when `yolo` mode is disabled.
fn prompt_user(prompt_text: &str, maybe_help_text: &Option<String>) -> bool {
    tracing_indicatif::suspend_tracing_indicatif(|| {
        let prompt = {
            let base_prompt = inquire::Confirm::new(prompt_text).with_default(false);
            match maybe_help_text {
                Some(help_text) => base_prompt.with_help_message(help_text),
                None => base_prompt,
            }
        };

        prompt.prompt().unwrap_or(false)
    })
}

/// Auto-approve all prompts.
///
/// This function automatically approves all prompts, used in `yolo` mode
/// or when running non-interactively.
fn auto_approve(prompt_text: &str, maybe_help_text: &Option<String>) -> bool {
    info!(target: "progress", prompt = %prompt_text, "Auto-approved");
    if let Some(help_text) = maybe_help_text {
        debug!(target: "progress", help = %help_text, "Auto-approve help text");
    }
    true
}

/// Create a prompt function from a UserInteraction implementation.
///
/// This bridges the gap between the trait-based UserInteraction interface
/// and the function pointer interface used by DoctorActionRun.
#[allow(dead_code)]
pub fn make_prompt_fn<U: UserInteraction>(
    user_interaction: &U,
) -> impl Fn(&str, &Option<String>) -> bool + '_ {
    move |prompt_text: &str, maybe_help_text: &Option<String>| {
        tracing_indicatif::suspend_tracing_indicatif(|| {
            user_interaction.confirm(prompt_text, maybe_help_text.as_deref())
        })
    }
}

async fn report_action_output<T>(
    group_name: &str,
    action: &T,
    action_result: &ActionRunResult,
) -> Result<()>
where
    T: DoctorActionRun,
{
    match action_result.status {
        ActionRunStatus::CheckSucceeded => {
            info!(target: "progress", group = group_name, name = action.name(), "Check was successful");
        }
        ActionRunStatus::NoCheckFixSucceeded => {
            info!(target: "progress", group = group_name, name = action.name(), "Fix ran successfully");
        }
        ActionRunStatus::CheckFailedFixSucceedVerifySucceed => {
            info!(target: "progress", group = group_name, name = action.name(), "Check initially failed, fix was successful");
        }
        ActionRunStatus::CheckFailedFixFailed => {
            error!(target: "user", group = group_name, name = action.name(), "Check failed, fix ran and {}", "failed".red().bold());
            print_pretty_result(group_name, &action.name(), action_result)
                .await
                .ok();
        }
        ActionRunStatus::CheckFailedFixSucceedVerifyFailed => {
            error!(target: "user", group = group_name, name = action.name(), "Check initially failed, fix ran, verification {}", "failed".red().bold());
            print_pretty_result(group_name, &action.name(), action_result)
                .await
                .ok();
        }
        ActionRunStatus::CheckFailedNoRunFix => {
            info!(target: "progress", group = group_name, name = action.name(), "Check failed, fix was not run");
        }
        ActionRunStatus::CheckFailedNoFixProvided => {
            error!(target: "user", group = group_name, name = action.name(), "Check failed, no fix provided");
            print_pretty_result(group_name, &action.name(), action_result)
                .await
                .ok();
        }
        ActionRunStatus::CheckFailedFixFailedStop => {
            error!(target: "user", group = group_name, name = action.name(), "Check failed, fix ran and {} and aborted", "failed".red().bold());
            print_pretty_result(group_name, &action.name(), action_result)
                .await
                .ok();
        }
        ActionRunStatus::CheckFailedFixUserDenied => {
            warn!(target: "user", group = group_name, name = action.name(), "Checked failed, user opted not to run fix");
            print_pretty_result(group_name, &action.name(), action_result)
                .await
                .ok();
        }
    }

    if action_result.status.is_failure() {
        if let Some(help_text) = &action.help_text() {
            error!(target: "user", group = group_name, name = action.name(), "Action Help: {}", help_text);
        }
        if let Some(help_url) = &action.help_url() {
            error!(target: "user", group = group_name, name = action.name(), "For more help, please visit {}", help_url);
        }
    }

    Ok(())
}

async fn print_pretty_result(
    group_name: &str,
    action_name: &str,
    result: &ActionRunResult,
) -> Result<()> {
    let task_reports = action_task_reports_for_display(&result.action_report);
    for task in task_reports {
        if let Some(text) = task.output {
            for line in text.lines() {
                error!(target: "user", group = group_name, action = action_name, "{}", line);
            }
        }
    }

    Ok(())
}

/// Returns the action task reports for display based on the presumed action report status.
fn action_task_reports_for_display(action_report: &ActionReport) -> Vec<ActionTaskReport> {
    if !action_report.check.is_empty() {
        action_report.check.clone()
    } else if !action_report.fix.is_empty() {
        action_report.fix.clone()
    } else {
        action_report.validate.clone()
    }
}

pub fn compute_group_order(
    groups: &BTreeMap<String, DoctorGroup>,
    desired_groups: BTreeSet<String>,
) -> Vec<String> {
    let mut graph = DiGraph::<&str, i32>::new();
    let mut node_graph: BTreeMap<String, NodeIndex> = BTreeMap::new();

    for name in groups.keys() {
        node_graph.insert(name.to_string(), graph.add_node(name));
    }

    for (name, model) in groups {
        let this = node_graph.get(name).unwrap();
        for dep in &model.requires {
            if let Some(other) = node_graph.get(dep) {
                graph.add_edge(*other, *this, 1);
            } else {
                warn!(target: "user", "{} needs {} but no such dependency found, ignoring dependency", name, dep);
            }
        }
    }

    let start = graph.add_node("start");

    for name in &desired_groups {
        if let Some(this) = node_graph.get(name) {
            graph.add_edge(*this, start, 1);
        }
    }

    debug!(
        format = "graphviz",
        "{:?}",
        Dot::with_config(&graph, &[Config::NodeIndexLabel])
    );

    graph.reverse();

    let mut order = Vec::new();
    for node in DfsPostOrder::new(&graph, start).iter(&graph) {
        if node == start {
            continue;
        }
        let name = graph.node_weight(node).unwrap().to_string();
        order.push(name)
    }

    order
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::doctor::check::tests::build_run_fail_fix_succeed_action;
    use crate::doctor::check::{
        ActionRunResult, ActionRunStatus, DoctorActionRun, MockDoctorActionRun,
    };
    use crate::doctor::runner::{GroupActionContainer, RunGroups, compute_group_order};
    use crate::doctor::tests::{group_noop, make_root_model_additional};
    use crate::shared::prelude::{ActionReport, ActionTaskReport, MockExecutionProvider};
    use anyhow::Result;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;
    use std::vec;

    #[tokio::test]
    async fn test_compute_group_order_with_no_dep_will_have_no_tasks() -> Result<()> {
        let action = build_run_fail_fix_succeed_action();

        let mut groups = BTreeMap::new();

        let step_1 = make_root_model_additional(
            vec![action.clone()],
            |meta| meta.name("step_1"),
            group_noop,
        );
        groups.insert("step_1".to_string(), step_1);

        let step_2 = make_root_model_additional(
            vec![action.clone()],
            |meta| meta.name("step_2"),
            |group| group.requires(vec!["step_1".to_string()]),
        );
        groups.insert("step_2".to_string(), step_2);

        assert_eq!(0, compute_group_order(&groups, BTreeSet::new()).len());

        Ok(())
    }

    #[tokio::test]
    async fn test_compute_group_order_with_one_dep_will_include_dep() -> Result<()> {
        let action = build_run_fail_fix_succeed_action();

        let mut groups = BTreeMap::new();

        let step_1 = make_root_model_additional(
            vec![action.clone()],
            |meta| meta.name("step_1"),
            group_noop,
        );
        groups.insert("step_1".to_string(), step_1);

        let step_2 = make_root_model_additional(
            vec![action.clone()],
            |meta| meta.name("step_2"),
            |group| group.requires(vec!["step_1".to_string()]),
        );
        groups.insert("step_2".to_string(), step_2);

        assert_eq!(
            vec!["step_1", "step_2"],
            compute_group_order(&groups, BTreeSet::from(["step_2".to_string()]))
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_compute_group_order_with_reversed_definition_order() -> Result<()> {
        let action = build_run_fail_fix_succeed_action();

        let mut groups = BTreeMap::new();

        let step_1 = make_root_model_additional(
            vec![action.clone()],
            |meta| meta.name("step_1"),
            |group| group.requires(vec!["step_2".to_string()]),
        );
        groups.insert("step_1".to_string(), step_1);

        let step_2 = make_root_model_additional(
            vec![action.clone()],
            |meta| meta.name("step_2"),
            |group| group.requires(vec!["step_3".to_string()]),
        );
        groups.insert("step_2".to_string(), step_2);

        let step_3 = make_root_model_additional(
            vec![action.clone()],
            |meta| meta.name("step_3"),
            group_noop,
        );
        groups.insert("step_3".to_string(), step_3);

        assert_eq!(
            vec!["step_3", "step_2", "step_1"],
            compute_group_order(&groups, BTreeSet::from(["step_1".to_string()]))
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_compute_group_order_with_multiple_dependencies() -> Result<()> {
        let action = build_run_fail_fix_succeed_action();

        let mut groups = BTreeMap::new();

        let step_1 = make_root_model_additional(
            vec![action.clone()],
            |meta| meta.name("step_1"),
            group_noop,
        );
        groups.insert("step_1".to_string(), step_1);

        let step_2 = make_root_model_additional(
            vec![action.clone()],
            |meta| meta.name("step_2"),
            |group| group.requires(vec!["step_1".to_string()]),
        );
        groups.insert("step_2".to_string(), step_2);

        let step_3 = make_root_model_additional(
            vec![action.clone()],
            |meta| meta.name("step_3"),
            |group| group.requires(vec!["step_1".to_string(), "step_2".to_string()]),
        );
        groups.insert("step_3".to_string(), step_3);

        assert_eq!(
            vec!["step_1", "step_2", "step_3"],
            compute_group_order(&groups, BTreeSet::from(["step_3".to_string()]))
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_compute_group_order_with_single_shared_dependency() -> Result<()> {
        let action = build_run_fail_fix_succeed_action();

        let mut groups = BTreeMap::new();

        let step_1 = make_root_model_additional(
            vec![action.clone()],
            |meta| meta.name("step_1"),
            group_noop,
        );
        groups.insert("step_1".to_string(), step_1);

        let step_2 = make_root_model_additional(
            vec![action.clone()],
            |meta| meta.name("step_2"),
            |group| group.requires(vec!["step_1".to_string()]),
        );
        groups.insert("step_2".to_string(), step_2);

        let step_3 = make_root_model_additional(
            vec![action.clone()],
            |meta| meta.name("step_3"),
            |group| group.requires(vec!["step_1".to_string()]),
        );
        groups.insert("step_3".to_string(), step_3);

        assert_eq!(
            vec!["step_1", "step_3"],
            compute_group_order(&groups, BTreeSet::from(["step_3".to_string()]))
        );

        Ok(())
    }

    fn make_action_run(result: ActionRunStatus, required: bool) -> MockDoctorActionRun {
        let mut run = MockDoctorActionRun::new();
        run.expect_run_action().returning(move |_| {
            Ok(ActionRunResult::new(
                "a_name",
                result.clone(),
                None,
                None,
                None,
            ))
        });
        run.expect_help_text().return_const(None);
        run.expect_help_url().return_const(None);
        run.expect_name().returning(|| "step name".to_string());
        run.expect_required().return_const(required);
        run.expect_description()
            .returning(|| "description".to_string());

        run
    }

    fn make_action_runs(result: ActionRunStatus) -> Vec<MockDoctorActionRun> {
        vec![make_action_run(result, true)]
    }

    fn will_not_run() -> Vec<MockDoctorActionRun> {
        let mut run = MockDoctorActionRun::new();
        run.expect_run_action().never();
        run.expect_help_text().return_const(None);
        run.expect_help_url().return_const(None);
        run.expect_name()
            .returning(|| "step name not run".to_string());
        run.expect_required().return_const(true);
        run.expect_description()
            .returning(|| "description".to_string());
        vec![run]
    }

    fn make_group_action<T: DoctorActionRun>(
        name: &str,
        result: Vec<T>,
    ) -> (String, GroupActionContainer<T>) {
        // Create a minimal test group
        let test_group = make_root_model_additional(vec![], |meta| meta.name(name), |group| group);

        (
            name.to_string(),
            GroupActionContainer {
                group: test_group,
                actions: result,
                exec_provider: Arc::new(MockExecutionProvider::new()),
                exec_working_dir: Default::default(),
                sys_path: "".to_string(),
            },
        )
    }

    #[tokio::test]
    async fn test_execute_run_with_multiple_paths_only_run_group_once() -> Result<()> {
        let group_actions = BTreeMap::from([
            make_group_action("group_1", make_action_runs(ActionRunStatus::CheckSucceeded)),
            make_group_action("group_2", make_action_runs(ActionRunStatus::CheckSucceeded)),
            make_group_action("group_3", make_action_runs(ActionRunStatus::CheckSucceeded)),
        ]);

        let run_groups = RunGroups {
            group_actions,
            all_paths: vec![
                "group_1".to_string(),
                "group_2".to_string(),
                "group_3".to_string(),
            ],
            yolo: false,
        };

        let exit_code = run_groups.execute().await?;
        assert!(exit_code.did_succeed);
        assert_eq!(
            BTreeSet::from_iter(run_groups.all_paths),
            exit_code.succeeded_groups
        );
        assert_eq!(BTreeSet::new(), exit_code.failed_group);
        assert_eq!(BTreeSet::new(), exit_code.skipped_group);
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_dep_fails_wont_run_others() -> Result<()> {
        let group_actions = BTreeMap::from([
            make_group_action(
                "fails",
                make_action_runs(ActionRunStatus::CheckFailedFixSucceedVerifyFailed),
            ),
            make_group_action("skipped_1", will_not_run()),
            make_group_action("skipped_2", will_not_run()),
        ]);

        let run_groups = RunGroups {
            group_actions,
            all_paths: vec![
                "fails".to_string(),
                "skipped_1".to_string(),
                "skipped_2".to_string(),
            ],
            yolo: false,
        };

        let exit_code = run_groups.execute().await?;
        assert!(!exit_code.did_succeed);
        assert_eq!(BTreeSet::new(), exit_code.succeeded_groups);
        assert_eq!(
            BTreeSet::from(["fails"].map(str::to_string)),
            exit_code.failed_group
        );
        assert_eq!(
            BTreeSet::from(["skipped_1", "skipped_2"].map(str::to_string)),
            exit_code.skipped_group
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_when_user_denies_fix_others_wont_run() -> Result<()> {
        let group_actions = BTreeMap::from([
            make_group_action(
                "succeeds",
                make_action_runs(ActionRunStatus::CheckFailedFixSucceedVerifySucceed),
            ),
            make_group_action(
                "user_denies",
                make_action_runs(ActionRunStatus::CheckFailedFixUserDenied),
            ),
            make_group_action("skipped", will_not_run()),
        ]);

        let run_groups = RunGroups {
            group_actions,
            all_paths: vec![
                "succeeds".to_string(),
                "user_denies".to_string(),
                "skipped".to_string(),
            ],
            yolo: false,
        };

        let exit_code = run_groups.execute().await?;
        assert!(!exit_code.did_succeed);

        assert_eq!(
            BTreeSet::from(["succeeds"].map(str::to_string)),
            exit_code.succeeded_groups
        );
        // the user denied one counts as skipped
        // and we should not try running anything that depends on it
        assert_eq!(
            BTreeSet::from(["user_denies", "skipped"].map(str::to_string)),
            exit_code.skipped_group
        );
        assert_eq!(BTreeSet::new(), exit_code.failed_group);

        Ok(())
    }

    #[tokio::test]
    async fn test_execute_when_user_denies_optional_fix_others_run() -> Result<()> {
        let group_actions = BTreeMap::from([
            make_group_action(
                "succeeds_1",
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
            make_group_action(
                "user_denies",
                vec![make_action_run(
                    ActionRunStatus::CheckFailedFixUserDenied,
                    false,
                )],
            ),
            make_group_action(
                "succeeds_2",
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
        ]);

        let run_groups = RunGroups {
            group_actions,
            all_paths: vec![
                "succeeds_1".to_string(),
                "user_denies".to_string(),
                "succeeds_2".to_string(),
            ],
            yolo: false,
        };

        let exit_code = run_groups.execute().await?;
        assert!(!exit_code.did_succeed);
        assert_eq!(
            BTreeSet::from(["succeeds_1", "succeeds_2"].map(str::to_string)),
            exit_code.succeeded_groups
        );
        assert_eq!(BTreeSet::new(), exit_code.failed_group);
        assert_eq!(
            BTreeSet::from(["user_denies"].map(str::to_string)),
            exit_code.skipped_group
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_branch_fails_but_other_branch_continues() -> Result<()> {
        let group_actions = BTreeMap::from([
            make_group_action(
                "succeeds_1",
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
            make_group_action(
                "fails",
                vec![make_action_run(
                    ActionRunStatus::CheckFailedFixSucceedVerifyFailed,
                    false,
                )],
            ),
            make_group_action(
                "succeeds_2",
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
        ]);

        let run_groups = RunGroups {
            group_actions,
            all_paths: vec![
                "succeeds_1".to_string(),
                "fails".to_string(),
                "succeeds_2".to_string(),
            ],
            yolo: false,
        };

        let exit_code = run_groups.execute().await?;
        assert!(!exit_code.did_succeed);
        assert_eq!(
            BTreeSet::from(["succeeds_1", "succeeds_2"].map(str::to_string)),
            exit_code.succeeded_groups
        );
        assert_eq!(
            BTreeSet::from(["fails"].map(str::to_string)),
            exit_code.failed_group
        );
        assert_eq!(BTreeSet::new(), exit_code.skipped_group);
        Ok(())
    }

    #[test]
    fn test_action_task_reports_for_display_when_check_nonempty() {
        let action_report = ActionReport {
            action_name: "test".to_string(),
            check: vec![ActionTaskReport {
                output: Some("check output".to_string()),
                ..Default::default()
            }],
            fix: vec![ActionTaskReport {
                output: Some("fix output".to_string()),
                ..Default::default()
            }],
            validate: vec![ActionTaskReport {
                output: Some("validate output".to_string()),
                ..Default::default()
            }],
        };

        let task_reports = action_task_reports_for_display(&action_report);
        let actual = task_reports.first().unwrap();
        assert_eq!(actual.output, Some("check output".to_string()));
    }

    #[test]
    fn test_action_task_reports_for_display_when_fix_nonempty() {
        let action_report = ActionReport {
            action_name: "test".to_string(),
            check: vec![],
            fix: vec![ActionTaskReport {
                output: Some("fix output".to_string()),
                ..Default::default()
            }],
            validate: vec![ActionTaskReport {
                output: Some("validate output".to_string()),
                ..Default::default()
            }],
        };

        let task_reports = action_task_reports_for_display(&action_report);
        let actual = task_reports.first().unwrap();
        assert_eq!(actual.output, Some("fix output".to_string()));
    }

    #[test]
    fn test_action_task_reports_for_display_when_validate_nonempty() {
        let action_report = ActionReport {
            action_name: "test".to_string(),
            check: vec![],
            fix: vec![],
            validate: vec![ActionTaskReport {
                output: Some("validate output".to_string()),
                ..Default::default()
            }],
        };

        let task_reports = action_task_reports_for_display(&action_report);
        let actual = task_reports.first().unwrap();
        assert_eq!(actual.output, Some("validate output".to_string()));
    }

    #[tokio::test]
    async fn test_execute_group_skips_when_should_skip_group_returns_true() -> Result<()> {
        let mut mock_action = MockDoctorActionRun::new();
        mock_action.expect_run_action().never(); // Should not be called
        mock_action.expect_help_text().return_const(None);
        mock_action.expect_help_url().return_const(None);
        mock_action
            .expect_name()
            .returning(|| "test action".to_string());
        mock_action.expect_required().return_const(false);
        mock_action
            .expect_description()
            .returning(|| "test description".to_string());

        // Create a test group with skip = true
        let test_group = make_root_model_additional(
            vec![],
            |meta| meta.name("test-group"),
            |group| group.skip(SkipSpec::Skip(true)),
        );

        let container = GroupActionContainer {
            group: test_group,
            actions: vec![mock_action],
            exec_provider: Arc::new(MockExecutionProvider::new()),
            exec_working_dir: Default::default(),
            sys_path: "".to_string(),
        };

        let run_groups = RunGroups {
            group_actions: BTreeMap::new(),
            all_paths: Vec::new(),
            yolo: false,
        };

        let group_span = info_span!("test_group", "indicatif.pb_show" = true);
        let result = run_groups.execute_group(&group_span, &container).await?;

        // Verify the group was skipped
        assert_eq!(result.group_name, "test-group");
        assert!(matches!(result.status, GroupExecutionStatus::GroupSkipped));
        assert!(!result.skip_remaining);

        Ok(())
    }

    #[tokio::test]
    async fn test_execute_group_runs_actions_when_should_skip_group_returns_false() -> Result<()> {
        let mut mock_action = MockDoctorActionRun::new();
        mock_action.expect_run_action().returning(|_| {
            Ok(ActionRunResult::new(
                "test action",
                ActionRunStatus::CheckSucceeded,
                None,
                None,
                None,
            ))
        });
        mock_action.expect_help_text().return_const(None);
        mock_action.expect_help_url().return_const(None);
        mock_action
            .expect_name()
            .returning(|| "test action".to_string());
        mock_action.expect_required().return_const(false);
        mock_action
            .expect_description()
            .returning(|| "test description".to_string());

        // Create a test group with skip = false
        let test_group = make_root_model_additional(
            vec![],
            |meta| meta.name("test-group"),
            |group| group.skip(SkipSpec::Skip(false)),
        );

        let container = GroupActionContainer {
            group: test_group,
            actions: vec![mock_action],
            exec_provider: Arc::new(MockExecutionProvider::new()),
            exec_working_dir: Default::default(),
            sys_path: "".to_string(),
        };

        let run_groups = RunGroups {
            group_actions: BTreeMap::new(),
            all_paths: Vec::new(),
            yolo: false,
        };

        let group_span = info_span!("test_group", "indicatif.pb_show" = true);
        let result = run_groups.execute_group(&group_span, &container).await?;

        // Verify the group was executed normally
        assert_eq!(result.group_name, "test-group");
        assert!(matches!(result.status, GroupExecutionStatus::Succeeded));
        assert!(!result.skip_remaining);

        Ok(())
    }
}
