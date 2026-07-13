use super::check::{ActionRunResult, ActionRunStatus, DoctorActionRun};
use crate::doctor::check::RuntimeError;
use crate::models::HelpMetadata;
use crate::prelude::{
    ActionReport, ActionTaskReport, CaptureOpts, ExecutionProvider, GroupReport, OutputDisplay,
    SkipSpec, generate_env_vars, progress_bar_without_pos,
};
use crate::shared::prelude::DoctorGroup;
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
        };

        self.group_reports.push(group.group_report.clone());
    }
}

#[derive(Debug)]
enum GroupExecutionStatus {
    Succeeded,
    Failed,
    Skipped,
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
                        output_dest: OutputDisplay::Silent,
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
    /// Groups trimmed from `all_paths` during planning (via `--skip`, `--skip-only`, or a
    /// group's own `skip` config) that would otherwise have run. Reported as skipped without
    /// ever being executed.
    pub(crate) skipped_groups: BTreeSet<String>,
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

        let mut run_result = self.run_path(full_path).await?;

        for group_name in &self.skipped_groups {
            debug_assert!(
                !run_result.succeeded_groups.contains(group_name)
                    && !run_result.failed_group.contains(group_name)
                    && !run_result.skipped_group.contains(group_name),
                "group {group_name} was both executed and planned-skipped"
            );
            warn!(target: "always", "Group skipped, group: \"{}\"", group_name);
            run_result.skipped_group.insert(group_name.to_string());
            run_result.group_reports.push(GroupReport::new(group_name));
        }

        Ok(run_result)
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

fn auto_approve(prompt_text: &str, maybe_help_text: &Option<String>) -> bool {
    println!("{} Yes (auto-approved)", prompt_text);
    if let Some(help_text) = maybe_help_text {
        println!("[{}]", help_text);
    }
    true
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
            let line_prefix = format!("{group_name}/{action_name}");
            for line in text.lines() {
                // Only write to stdout — tracing already happened during capture
                writeln!(
                    crate::prelude::STDOUT_WRITER.write().await,
                    "{}:  {}\r",
                    line_prefix.dimmed(),
                    line
                )
                .ok();
            }
        }
    }

    Ok(())
}

/// Returns the most relevant action task reports for display.
/// Priority: validate > fix > check — shows the latest phase that ran,
/// which is the most useful output when diagnosing failures.
fn action_task_reports_for_display(action_report: &ActionReport) -> Vec<ActionTaskReport> {
    if !action_report.validate.is_empty() {
        action_report.validate.clone()
    } else if !action_report.fix.is_empty() {
        action_report.fix.clone()
    } else {
        action_report.check.clone()
    }
}

/// A dependency graph over a set of groups, along with the lookup from group name to its node.
struct DependencyGraph<'a> {
    graph: DiGraph<&'a str, i32>,
    node_graph: BTreeMap<String, NodeIndex>,
}

/// Builds the dependency graph over every group not in `exclude`, with an edge from each
/// dependency to its dependent (per `requires`).
fn build_dependency_graph<'a>(
    groups: &'a BTreeMap<String, DoctorGroup>,
    exclude: &BTreeSet<String>,
) -> DependencyGraph<'a> {
    let included = |name: &String| !exclude.contains(name);

    let mut graph = DiGraph::<&str, i32>::new();
    let node_graph: BTreeMap<String, NodeIndex> = groups
        .keys()
        .filter(|name| included(name))
        .map(|name| (name.clone(), graph.add_node(name)))
        .collect();

    for (name, model) in groups.iter().filter(|(name, _)| included(name)) {
        let this = node_graph[name];
        for dep in model.requires.iter().filter(|dep| included(dep)) {
            match node_graph.get(dep) {
                Some(other) => {
                    graph.add_edge(*other, this, 1);
                }
                None => {
                    warn!(target: "user", "{} needs {} but no such dependency found, ignoring dependency", name, dep)
                }
            }
        }
    }

    DependencyGraph { graph, node_graph }
}

/// Adds a synthetic "start" node to `graph`, wired from every name in `roots`, and returns its
/// index.
fn wire_start_node(
    graph: &mut DiGraph<&str, i32>,
    node_graph: &BTreeMap<String, NodeIndex>,
    roots: &BTreeSet<String>,
) -> NodeIndex {
    let start = graph.add_node("start");
    for name in roots {
        if let Some(this) = node_graph.get(name) {
            graph.add_edge(*this, start, 1);
        }
    }
    start
}

/// Reverses `graph` and returns the dependency-first (post-order) traversal of everything
/// reachable from `start`, excluding `start` itself.
fn traversal_order_from(graph: &mut DiGraph<&str, i32>, start: NodeIndex) -> Vec<String> {
    graph.reverse();

    DfsPostOrder::new(&*graph, start)
        .iter(&*graph)
        .filter(|&node| node != start)
        .map(|node| graph.node_weight(node).unwrap().to_string())
        .collect()
}

/// Names of every group reachable from `desired_groups`, over the dependency graph with
/// `exclude` removed. Used to test "would this group have run at all" independent of any
/// `skip_only` decision.
fn reachable_group_names(
    groups: &BTreeMap<String, DoctorGroup>,
    desired_groups: &BTreeSet<String>,
    exclude: &BTreeSet<String>,
) -> BTreeSet<String> {
    let DependencyGraph {
        mut graph,
        node_graph,
    } = build_dependency_graph(groups, exclude);

    let start = wire_start_node(&mut graph, &node_graph, desired_groups);
    traversal_order_from(&mut graph, start)
        .into_iter()
        .collect()
}

/// Arguments for [`compute_group_order`].
pub struct GroupOrderParams<'a> {
    pub groups: &'a BTreeMap<String, DoctorGroup>,
    pub desired_groups: &'a BTreeSet<String>,
    pub skip_subtree: &'a BTreeSet<String>,
    pub skip_only: &'a BTreeSet<String>,
}

/// Computes the topologically-sorted set of groups to run, starting from `desired_groups` and
/// pulling in transitive dependencies (via `requires`).
///
/// `skip_subtree` groups are removed along with any dependency not also required by a
/// non-skipped group (a shared dependency survives). `skip_only` groups are removed but their
/// dependencies are always kept — unless that group was never going to run in the first place,
/// in which case `--skip-only` on it is a no-op rather than pulling in unrelated work.
pub fn compute_group_order(params: GroupOrderParams) -> Vec<String> {
    let GroupOrderParams {
        groups,
        desired_groups,
        skip_subtree,
        skip_only,
    } = params;

    let all_skipped: BTreeSet<String> = skip_subtree.union(skip_only).cloned().collect();

    // Only force-keep a `skip_only` group's dependencies if that group would actually have run
    // (reachable from `desired_groups` once force-removed `skip_subtree` groups are excluded).
    let would_have_run = if skip_only.is_empty() {
        BTreeSet::new()
    } else {
        reachable_group_names(groups, desired_groups, skip_subtree)
    };

    let DependencyGraph {
        mut graph,
        node_graph,
    } = build_dependency_graph(groups, &all_skipped);

    let roots: BTreeSet<String> = desired_groups
        .iter()
        .filter(|name| !all_skipped.contains(*name))
        .cloned()
        .chain(
            skip_only
                .iter()
                .filter(|name| would_have_run.contains(*name))
                .filter_map(|name| groups.get(name))
                .flat_map(|model| model.requires.iter().cloned())
                .filter(|dep| !all_skipped.contains(dep)),
        )
        .collect();

    let start = wire_start_node(&mut graph, &node_graph, &roots);

    debug!(
        format = "graphviz",
        "{:?}",
        Dot::with_config(&graph, &[Config::EdgeNoLabel])
    );

    let order = traversal_order_from(&mut graph, start);

    debug!(
        target: "user",
        "Resolved doctor run order: [{}]{}",
        order.join(", "),
        if all_skipped.is_empty() {
            String::new()
        } else {
            format!(
                "; skipped/trimmed: [{}]",
                all_skipped.iter().cloned().collect::<Vec<_>>().join(", ")
            )
        }
    );

    order
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::doctor::check::tests::build_run_fail_fix_succeed_action;
    use crate::doctor::check::{
        ActionRunResult, ActionRunStatus, DoctorActionRun, MockDoctorActionRun,
    };
    use crate::doctor::runner::{
        GroupActionContainer, GroupOrderParams, RunGroups, compute_group_order,
    };
    use crate::doctor::tests::{group_noop, make_root_model_additional};
    use crate::prelude::{ActionReport, ActionTaskReport, MockExecutionProvider};
    use anyhow::Result;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;
    use std::vec;

    fn no_skip() -> BTreeSet<String> {
        BTreeSet::new()
    }

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

        assert_eq!(
            0,
            compute_group_order(GroupOrderParams {
                groups: &groups,
                desired_groups: &BTreeSet::new(),
                skip_subtree: &no_skip(),
                skip_only: &no_skip(),
            })
            .len()
        );

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
            compute_group_order(GroupOrderParams {
                groups: &groups,
                desired_groups: &BTreeSet::from(["step_2".to_string()]),
                skip_subtree: &no_skip(),
                skip_only: &no_skip(),
            })
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
            compute_group_order(GroupOrderParams {
                groups: &groups,
                desired_groups: &BTreeSet::from(["step_1".to_string()]),
                skip_subtree: &no_skip(),
                skip_only: &no_skip(),
            })
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
            compute_group_order(GroupOrderParams {
                groups: &groups,
                desired_groups: &BTreeSet::from(["step_3".to_string()]),
                skip_subtree: &no_skip(),
                skip_only: &no_skip(),
            })
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
            compute_group_order(GroupOrderParams {
                groups: &groups,
                desired_groups: &BTreeSet::from(["step_3".to_string()]),
                skip_subtree: &no_skip(),
                skip_only: &no_skip(),
            })
        );

        Ok(())
    }

    fn make_dependency_chain() -> BTreeMap<String, DoctorGroup> {
        let action = build_run_fail_fix_succeed_action();

        let mut groups = BTreeMap::new();
        groups.insert(
            "a".to_string(),
            make_root_model_additional(
                vec![action.clone()],
                |meta| meta.name("a"),
                |group| group.requires(vec!["b".to_string()]),
            ),
        );
        groups.insert(
            "b".to_string(),
            make_root_model_additional(
                vec![action.clone()],
                |meta| meta.name("b"),
                |group| group.requires(vec!["c".to_string()]),
            ),
        );
        groups.insert(
            "c".to_string(),
            make_root_model_additional(vec![action.clone()], |meta| meta.name("c"), group_noop),
        );
        groups.insert(
            "d".to_string(),
            make_root_model_additional(
                vec![action.clone()],
                |meta| meta.name("d"),
                |group| group.requires(vec!["shared".to_string()]),
            ),
        );
        groups.insert(
            "shared".to_string(),
            make_root_model_additional(
                vec![action.clone()],
                |meta| meta.name("shared"),
                group_noop,
            ),
        );
        groups.insert(
            "a-and-shared".to_string(),
            make_root_model_additional(
                vec![action.clone()],
                |meta| meta.name("a-and-shared"),
                |group| group.requires(vec!["a".to_string(), "shared".to_string()]),
            ),
        );

        groups
    }

    #[tokio::test]
    async fn test_compute_group_order_skip_trims_exclusive_dependencies() -> Result<()> {
        let groups = make_dependency_chain();

        let order = compute_group_order(GroupOrderParams {
            groups: &groups,
            desired_groups: &BTreeSet::from(["a".to_string(), "d".to_string()]),
            skip_subtree: &BTreeSet::from(["a".to_string()]),
            skip_only: &no_skip(),
        });

        // `a`, and its exclusive deps `b`/`c`, are trimmed; `d` and its dep are untouched.
        assert_eq!(vec!["shared", "d"], order);

        Ok(())
    }

    #[tokio::test]
    async fn test_compute_group_order_skip_force_removes_group_but_keeps_shared_dependency()
    -> Result<()> {
        let groups = make_dependency_chain();

        // `a-and-shared` requires both `a` and `shared`. Skipping `a` force-removes it (and its
        // exclusive deps `b`/`c`) even though `a-and-shared` — a non-skipped root — depends on
        // it. `shared` survives because `a-and-shared` also depends on it directly.
        let order = compute_group_order(GroupOrderParams {
            groups: &groups,
            desired_groups: &BTreeSet::from(["a-and-shared".to_string()]),
            skip_subtree: &BTreeSet::from(["a".to_string()]),
            skip_only: &no_skip(),
        });

        assert_eq!(vec!["shared", "a-and-shared"], order);

        Ok(())
    }

    #[tokio::test]
    async fn test_compute_group_order_skip_only_keeps_dependencies() -> Result<()> {
        let groups = make_dependency_chain();

        // `--skip-only=a` removes just `a`; its exclusive deps `b`/`c` still run.
        let order = compute_group_order(GroupOrderParams {
            groups: &groups,
            desired_groups: &BTreeSet::from(["a".to_string()]),
            skip_subtree: &no_skip(),
            skip_only: &BTreeSet::from(["a".to_string()]),
        });

        assert_eq!(vec!["c", "b"], order);

        Ok(())
    }

    #[tokio::test]
    async fn test_compute_group_order_skip_only_on_unreached_group_is_a_noop() -> Result<()> {
        let groups = make_dependency_chain();

        // Only `d` is desired; `a` (and its exclusive deps `b`/`c`) were never going to run in
        // the first place. `--skip-only=a` must not pull `b`/`c` in as unrelated new work.
        let order = compute_group_order(GroupOrderParams {
            groups: &groups,
            desired_groups: &BTreeSet::from(["d".to_string()]),
            skip_subtree: &no_skip(),
            skip_only: &BTreeSet::from(["a".to_string()]),
        });

        assert_eq!(vec!["shared", "d"], order);

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
            skipped_groups: BTreeSet::new(),
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
            skipped_groups: BTreeSet::new(),
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
            skipped_groups: BTreeSet::new(),
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
            skipped_groups: BTreeSet::new(),
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
            skipped_groups: BTreeSet::new(),
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
    fn test_action_task_reports_for_display_prefers_validate_over_all() {
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
        assert_eq!(actual.output, Some("validate output".to_string()));
    }

    #[test]
    fn test_action_task_reports_for_display_prefers_fix_over_check() {
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
            validate: vec![],
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
    async fn test_execute_command_returns_command_output() -> Result<()> {
        let test_group =
            make_root_model_additional(vec![], |meta| meta.name("test-group"), group_noop);

        let mut mock_exec = MockExecutionProvider::new();
        mock_exec
            .expect_run_for_output()
            .times(1)
            .withf(|_, _, command| command == "echo hi")
            .returning(|_, _, _| "hi".to_string());

        let container: GroupActionContainer<MockDoctorActionRun> = GroupActionContainer {
            group: test_group,
            actions: vec![],
            exec_provider: Arc::new(mock_exec),
            exec_working_dir: Default::default(),
            sys_path: "".to_string(),
        };

        assert_eq!("hi", container.execute_command("echo hi").await?);

        Ok(())
    }

    #[tokio::test]
    async fn test_should_skip_group_true_for_boolean_skip() -> Result<()> {
        let test_group = make_root_model_additional(
            vec![],
            |meta| meta.name("test-group"),
            |group| group.skip(SkipSpec::Skip(true)),
        );

        let container: GroupActionContainer<MockDoctorActionRun> = GroupActionContainer {
            group: test_group,
            actions: vec![],
            exec_provider: Arc::new(MockExecutionProvider::new()),
            exec_working_dir: Default::default(),
            sys_path: "".to_string(),
        };

        assert!(container.should_skip_group().await?);

        Ok(())
    }

    #[tokio::test]
    async fn test_should_skip_group_false_for_boolean_no_skip() -> Result<()> {
        let test_group = make_root_model_additional(
            vec![],
            |meta| meta.name("test-group"),
            |group| group.skip(SkipSpec::Skip(false)),
        );

        let container: GroupActionContainer<MockDoctorActionRun> = GroupActionContainer {
            group: test_group,
            actions: vec![],
            exec_provider: Arc::new(MockExecutionProvider::new()),
            exec_working_dir: Default::default(),
            sys_path: "".to_string(),
        };

        assert!(!container.should_skip_group().await?);

        Ok(())
    }

    #[tokio::test]
    async fn test_execute_runs_group_actions_regardless_of_group_skip_field() -> Result<()> {
        // `execute_group` no longer consults `group.skip` — skip is resolved during planning
        // (see doctor_run) and reflected in `RunGroups.skipped_groups` instead. A group reaching
        // `execute_group` always runs its actions.
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
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let group_span = info_span!("test_group", "indicatif.pb_show" = true);
        let result = run_groups.execute_group(&group_span, &container).await?;

        assert_eq!(result.group_name, "test-group");
        assert!(matches!(result.status, GroupExecutionStatus::Succeeded));
        assert!(!result.skip_remaining);

        Ok(())
    }

    #[tokio::test]
    async fn test_execute_reports_planned_skips_without_running_them() -> Result<()> {
        let group_actions = BTreeMap::from([make_group_action(
            "runs",
            make_action_runs(ActionRunStatus::CheckSucceeded),
        )]);

        let run_groups = RunGroups {
            group_actions,
            all_paths: vec!["runs".to_string()],
            skipped_groups: BTreeSet::from(["trimmed".to_string()]),
            yolo: false,
        };

        let exit_code = run_groups.execute().await?;

        // Groups trimmed during planning are reported as skipped without failing the run.
        assert!(exit_code.did_succeed);
        assert_eq!(
            BTreeSet::from(["runs".to_string()]),
            exit_code.succeeded_groups
        );
        assert_eq!(
            BTreeSet::from(["trimmed".to_string()]),
            exit_code.skipped_group
        );

        Ok(())
    }
}
