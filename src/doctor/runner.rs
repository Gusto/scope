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
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum GroupExecutionStatus {
    Succeeded,
    Skipped,
    Failed,
}

impl GroupExecutionStatus {
    /// Combine with a newly observed action status, keeping whichever is more
    /// severe. Ensures a later successful action can't clear an earlier
    /// failure/skip within the same group.
    fn merge(self, other: Self) -> Self {
        std::cmp::max(self, other)
    }
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

/// The dependency DAG over a run's runnable groups (i.e. exactly the names in the topological
/// `order` passed to [`RunGraph::new`]). An edge points from a dependent to a group it
/// `requires`. Built once during planning and walked recursively during execution, so a
/// dependent group's span is opened before — and therefore encloses — the spans of the groups
/// it depends on (see issue #172: previously every group span was a sibling directly under
/// `doctor run`, regardless of `requires`).
///
/// A "root" is a group no other group in the graph requires — these are the groups that parent
/// directly to the top-level `doctor run` span. A group required by more than one other group
/// (a diamond dependency) still appears once, nested under whichever dependent's subtree reaches
/// it first during the walk.
///
/// Nodes are added in `order`, so `NodeIndex::index()` equals a node's position in `order` —
/// [`RunGraph::roots`] and [`RunGraph::dependencies`] sort by that position so the recursive
/// walk's group-by-group execution sequence matches `order` exactly (this is what preserves
/// fail-fast and reporting behavior; see [`RunGroups::execute`]).
pub(crate) struct RunGraph {
    graph: DiGraph<String, ()>,
    nodes: BTreeMap<String, NodeIndex>,
}

impl RunGraph {
    /// Builds the graph over every name in `order`, adding an edge from each group to a
    /// dependency named in its `requires` when that dependency is also present in `order`.
    /// Dependencies not present in `order` (e.g. trimmed during planning) are simply omitted —
    /// planning has already decided the final runnable set; this only reconstructs the shape of
    /// the dependencies among it.
    ///
    /// This deliberately doesn't share code with [`build_dependency_graph`], the other
    /// `requires`-to-`DiGraph` builder in this file: that one runs during planning, points
    /// edges dependency-to-dependent, borrows `&str` node weights from `groups`, and warns on a
    /// `requires` naming a group that doesn't exist at all. This one runs during execution,
    /// points edges dependent-to-dependency (so [`RunGraph::roots`]/[`RunGraph::dependencies`]
    /// read naturally), owns its `String` node weights, and treats a missing name as
    /// unremarkable (it was legitimately trimmed by planning, e.g. via `--skip`) rather than as
    /// a config error. If dependency-graph semantics change — most likely cycle detection, since
    /// none exists today (see the fallback pass in [`RunGroups::execute`]) — check whether it
    /// should apply to both.
    pub(crate) fn new(order: &[String], groups: &BTreeMap<String, DoctorGroup>) -> Self {
        let mut graph = DiGraph::<String, ()>::new();
        let nodes: BTreeMap<String, NodeIndex> = order
            .iter()
            .map(|name| (name.clone(), graph.add_node(name.clone())))
            .collect();

        for name in order {
            let Some(&this) = nodes.get(name) else {
                continue;
            };
            let Some(model) = groups.get(name) else {
                continue;
            };
            for dep in &model.requires {
                if let Some(&dep_node) = nodes.get(dep) {
                    graph.add_edge(this, dep_node, ());
                }
            }
        }

        Self { graph, nodes }
    }

    /// Groups no other group in the graph requires, ordered by their position in `order`.
    fn roots(&self) -> Vec<NodeIndex> {
        let mut roots: Vec<NodeIndex> = self
            .graph
            .node_indices()
            .filter(|&node| {
                self.graph
                    .neighbors_directed(node, Direction::Incoming)
                    .next()
                    .is_none()
            })
            .collect();
        roots.sort_by_key(|node| node.index());
        roots
    }

    /// A group's dependencies present in the graph, ordered by their position in `order`.
    fn dependencies(&self, node: NodeIndex) -> Vec<NodeIndex> {
        let mut deps: Vec<NodeIndex> = self
            .graph
            .neighbors_directed(node, Direction::Outgoing)
            .collect();
        deps.sort_by_key(|node| node.index());
        deps
    }

    fn name(&self, node: NodeIndex) -> &str {
        &self.graph[node]
    }

    fn node(&self, name: &str) -> Option<NodeIndex> {
        self.nodes.get(name).copied()
    }

    fn contains(&self, name: &str) -> bool {
        self.nodes.contains_key(name)
    }

    fn len(&self) -> usize {
        self.graph.node_count()
    }
}

pub struct RunGroups<T>
where
    T: DoctorActionRun,
{
    pub(crate) group_actions: BTreeMap<String, GroupActionContainer<T>>,
    pub(crate) graph: RunGraph,
    /// The full candidate order (topological, ignoring any skip decisions). Iterated instead of
    /// `graph`'s nodes purely so `skipped_groups` can be reported in their natural position in
    /// the run rather than lumped together at the end.
    pub(crate) full_order: Vec<String>,
    /// Groups trimmed from the run during planning (via `--skip`, `--skip-only`, or a group's
    /// own `skip` config) that would otherwise have run. Reported as skipped without ever being
    /// executed.
    pub(crate) skipped_groups: BTreeSet<String>,
    pub(crate) yolo: bool,
}

/// Mutable state threaded through the whole recursive walk in [`RunGroups::run_group_subtree`].
/// Unlike `parent_span` — which changes on every recursive call — these three keep the same
/// identity for the entire walk; bundling them keeps the recursive function's signature from
/// growing every time the walk needs to track something new, and avoids the
/// `clippy::too_many_arguments` a fully-flattened parameter list would hit.
struct WalkState {
    visited: BTreeSet<NodeIndex>,
    skip_remaining: bool,
    run_result: PathRunResult,
}

impl<T> RunGroups<T>
where
    T: DoctorActionRun,
{
    pub async fn execute(&self) -> Result<PathRunResult> {
        let header_span = info_span!("doctor run", "indicatif.pb_show" = true);
        header_span.pb_set_length((self.graph.len() + self.skipped_groups.len()) as u64);
        header_span.pb_set_message("scope doctor run");
        let _span = header_span.enter();

        let roots: BTreeSet<&str> = self
            .graph
            .roots()
            .into_iter()
            .map(|node| self.graph.name(node))
            .collect();

        let mut state = WalkState {
            visited: BTreeSet::new(),
            skip_remaining: false,
            run_result: PathRunResult {
                did_succeed: true,
                succeeded_groups: BTreeSet::new(),
                failed_group: BTreeSet::new(),
                skipped_group: BTreeSet::new(),
                group_reports: Vec::new(),
            },
        };

        for group_name in &self.full_order {
            if self.skipped_groups.contains(group_name) {
                debug_assert!(
                    !self.graph.contains(group_name),
                    "group {group_name} is both runnable and planned-skipped"
                );
                header_span.pb_inc(1);
                warn!(target: "always", "Group skipped, group: \"{}\"", group_name);
                state.run_result.skipped_group.insert(group_name.clone());
                state
                    .run_result
                    .group_reports
                    .push(GroupReport::new(group_name));
                continue;
            }

            let Some(node) = self.graph.node(group_name) else {
                // Transitively pruned as an exclusive dependency of a skipped group — nothing
                // to report, it simply never appears in the graph.
                continue;
            };

            if !roots.contains(group_name.as_str()) {
                // Not a root: it runs as part of a dependent's subtree, opened below.
                continue;
            }

            self.run_group_subtree(node, &header_span, &header_span, &mut state)
                .await?;
        }

        // A `requires` cycle — nothing validates against this anywhere in the codebase — gives
        // every member of the cycle an incoming edge, so `roots()` excludes all of them and the
        // loop above never reaches them via recursion either (nothing outside the cycle points
        // into it). Rather than silently dropping them from the run while still reporting
        // success, execute whatever's left unvisited directly, nested under `doctor run` like a
        // root — a cycle has no single "correct" dependent to nest it under anyway.
        for group_name in &self.full_order {
            let Some(node) = self.graph.node(group_name) else {
                continue;
            };
            if state.visited.contains(&node) {
                continue;
            }
            warn!(
                target: "user",
                "Group \"{}\" has a cyclic `requires` relationship and could not be nested under a dependent; running it directly",
                group_name
            );
            self.run_group_subtree(node, &header_span, &header_span, &mut state)
                .await?;
        }

        Ok(state.run_result)
    }

    /// Recursively runs `node`'s dependencies (nested under its own span, so that span
    /// temporally encloses them), then runs `node` itself. Dependencies are visited in the same
    /// relative order as `full_order`, and `state` is threaded through every call by mutable
    /// reference, so the flattened "own turn" sequence produced by this walk, and the
    /// fail-fast/reporting behavior driven by it, are identical to the previous flat,
    /// non-nested traversal.
    fn run_group_subtree<'a>(
        &'a self,
        node: NodeIndex,
        parent_span: &'a Span,
        header_span: &'a Span,
        state: &'a mut WalkState,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(async move {
            if state.visited.contains(&node) {
                // Already ran as a dependency of an earlier-visited group (diamond dependency).
                return Ok(());
            }
            state.visited.insert(node);

            let group_name = self.graph.name(node).to_string();
            let Some(container) = self.group_actions.get(&group_name) else {
                // No action container for this name. Not reachable today — `group_actions` and
                // `graph` are always built from the same source — but still visit this node's
                // dependencies (nested under `parent_span`, since there's no group span of its
                // own to nest them under) rather than silently orphaning them too.
                for dep in self.graph.dependencies(node) {
                    self.run_group_subtree(dep, parent_span, header_span, state)
                        .await?;
                }
                return Ok(());
            };

            let group_span = info_span!(
                parent: parent_span,
                "group",
                "indicatif.pb_show" = true,
                "group.name" = group_name.as_str(),
                "otel.name" = format!("group {}", group_name)
            );
            group_span.pb_set_length(container.actions.len() as u64);
            group_span.pb_set_message(&format!("group {group_name}"));
            let _span = group_span.enter();

            for dep in self.graph.dependencies(node) {
                self.run_group_subtree(dep, &group_span, header_span, state)
                    .await?;
            }

            header_span.pb_inc(1);
            debug!(target: "user", "Running check {}", group_name);

            if state.skip_remaining {
                state.run_result.skipped_group.insert(group_name);
                return Ok(());
            }

            let group_result = self.execute_group(&group_span, container).await?;
            if let GroupExecutionStatus::Failed = group_result.status {
                group_span.set_status(Status::Error {
                    description: std::borrow::Cow::Owned(format!(
                        "{} group failed",
                        group_result.group_name
                    )),
                });
            }

            state.run_result.process(&group_result);
            state.skip_remaining |= group_result.skip_remaining;

            Ok(())
        })
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

            let action_status = match action_result.status {
                ActionRunStatus::CheckSucceeded
                | ActionRunStatus::NoCheckFixSucceeded
                | ActionRunStatus::CheckFailedFixSucceedVerifySucceed => {
                    GroupExecutionStatus::Succeeded
                }
                ActionRunStatus::CheckFailedFixUserDenied => GroupExecutionStatus::Skipped,
                _ => GroupExecutionStatus::Failed,
            };
            // Merge (worst-wins) rather than overwrite, so a later succeeding
            // action can't clear an earlier failure/skip in the same group.
            results.status = results.status.merge(action_status);

            // Derived from `action_status` (rather than re-matching on
            // `action_result.status`) so a newly added `ActionRunStatus` variant
            // only needs classifying once, above, to affect both group status and
            // halting — the two decisions can't silently diverge. Only the two
            // exceptional cases below need to override that classification.
            results.skip_remaining = match action_result.status {
                // --fix=false: a not-run fix should never halt the rest of the
                // run, so every check gets a chance to report its status.
                ActionRunStatus::CheckFailedNoRunFix => false,
                ActionRunStatus::CheckFailedFixFailedStop => true,
                _ => match action_status {
                    GroupExecutionStatus::Succeeded => false,
                    GroupExecutionStatus::Skipped | GroupExecutionStatus::Failed => {
                        action.required()
                    }
                },
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
            warn!(target: "user", group = group_name, name = action.name(), "Check failed, fix was not run");
            print_pretty_result(group_name, &action.name(), action_result)
                .await
                .ok();
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
    use crate::prelude::{ActionReport, ActionTaskReport, CaptureError, MockExecutionProvider};
    use anyhow::Result;
    use std::collections::{BTreeMap, BTreeSet, HashMap};
    use std::sync::{Arc, Mutex};
    use std::vec;
    use tracing::field::{Field, Visit};
    use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

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

    /// A [`RunGraph`] over `names` with no dependency edges — every name is a root. Used by
    /// tests that don't care about nesting, so they exercise the same "every group is
    /// independent" shape the old flat `all_paths` traversal always produced.
    fn flat_graph(names: &[String]) -> RunGraph {
        RunGraph::new(names, &BTreeMap::new())
    }

    /// Builds a [`RunGraph`] over `order` using the `requires` already set on each entry in
    /// `group_actions` — lets a test declare dependencies once, on the container, and have both
    /// the graph and the execution use the same data instead of restating them separately.
    fn graph_from<T: DoctorActionRun>(
        order: &[&str],
        group_actions: &BTreeMap<String, GroupActionContainer<T>>,
    ) -> RunGraph {
        let order: Vec<String> = order.iter().map(|s| s.to_string()).collect();
        let groups: BTreeMap<String, DoctorGroup> = group_actions
            .iter()
            .map(|(name, container)| (name.clone(), container.group.clone()))
            .collect();
        RunGraph::new(&order, &groups)
    }

    fn make_group_action_requiring<T: DoctorActionRun>(
        name: &str,
        requires: Vec<&str>,
        result: Vec<T>,
    ) -> (String, GroupActionContainer<T>) {
        let requires: Vec<String> = requires.into_iter().map(str::to_string).collect();
        let test_group = make_root_model_additional(
            vec![],
            |meta| meta.name(name),
            |group| group.requires(requires),
        );

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

    #[test]
    fn test_run_graph_roots_and_dependencies_ordered_by_position() {
        let mut groups = BTreeMap::new();
        groups.insert(
            "b".to_string(),
            make_root_model_additional(vec![], |meta| meta.name("b"), group_noop),
        );
        groups.insert(
            "a".to_string(),
            make_root_model_additional(
                vec![],
                |meta| meta.name("a"),
                |group| group.requires(vec!["b".to_string()]),
            ),
        );

        let order = vec!["b".to_string(), "a".to_string()];
        let graph = RunGraph::new(&order, &groups);

        assert_eq!(2, graph.len());
        assert!(graph.contains("a"));
        assert!(!graph.contains("missing"));

        let roots: Vec<&str> = graph.roots().into_iter().map(|n| graph.name(n)).collect();
        assert_eq!(vec!["a"], roots, "only `a` isn't required by anything else");

        let a = graph.node("a").expect("a is in the graph");
        let deps: Vec<&str> = graph
            .dependencies(a)
            .into_iter()
            .map(|n| graph.name(n))
            .collect();
        assert_eq!(vec!["b"], deps);
    }

    #[test]
    fn test_run_graph_diamond_dependency_reachable_from_both_roots() {
        let mut groups = BTreeMap::new();
        groups.insert(
            "s".to_string(),
            make_root_model_additional(vec![], |meta| meta.name("s"), group_noop),
        );
        groups.insert(
            "a".to_string(),
            make_root_model_additional(
                vec![],
                |meta| meta.name("a"),
                |group| group.requires(vec!["s".to_string()]),
            ),
        );
        groups.insert(
            "b".to_string(),
            make_root_model_additional(
                vec![],
                |meta| meta.name("b"),
                |group| group.requires(vec!["s".to_string()]),
            ),
        );

        let order = vec!["s".to_string(), "a".to_string(), "b".to_string()];
        let graph = RunGraph::new(&order, &groups);

        let roots: Vec<&str> = graph.roots().into_iter().map(|n| graph.name(n)).collect();
        assert_eq!(
            vec!["a", "b"],
            roots,
            "`s` is required by both, so neither is a root"
        );

        let s = graph.node("s").expect("s is in the graph");
        let a = graph.node("a").expect("a is in the graph");
        let b = graph.node("b").expect("b is in the graph");
        assert_eq!(vec![s], graph.dependencies(a));
        assert_eq!(vec![s], graph.dependencies(b));
    }

    #[tokio::test]
    async fn test_execute_shared_dependency_runs_once() -> Result<()> {
        let mut shared_action = MockDoctorActionRun::new();
        shared_action.expect_run_action().times(1).returning(|_| {
            Ok(ActionRunResult::new(
                "shared_action",
                ActionRunStatus::CheckSucceeded,
                None,
                None,
                None,
            ))
        });
        shared_action.expect_help_text().return_const(None);
        shared_action.expect_help_url().return_const(None);
        shared_action
            .expect_name()
            .returning(|| "shared_action".to_string());
        shared_action.expect_required().return_const(true);
        shared_action
            .expect_description()
            .returning(|| "description".to_string());

        let group_actions = BTreeMap::from([
            make_group_action_requiring("shared", vec![], vec![shared_action]),
            make_group_action_requiring(
                "a",
                vec!["shared"],
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
            make_group_action_requiring(
                "b",
                vec!["shared"],
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
        ]);

        let order = ["shared", "a", "b"];
        let run_groups = RunGroups {
            graph: graph_from(&order, &group_actions),
            group_actions,
            full_order: order.iter().map(|s| s.to_string()).collect(),
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let exit_code = execute_locked(&run_groups).await?;
        assert!(exit_code.did_succeed);
        assert_eq!(
            BTreeSet::from(["shared", "a", "b"].map(str::to_string)),
            exit_code.succeeded_groups
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_dependency_failure_skips_dependent() -> Result<()> {
        let group_actions = BTreeMap::from([
            make_group_action_requiring(
                "dep",
                vec![],
                make_action_runs(ActionRunStatus::CheckFailedFixSucceedVerifyFailed),
            ),
            make_group_action_requiring("dependent", vec!["dep"], will_not_run()),
        ]);

        let order = ["dep", "dependent"];
        let run_groups = RunGroups {
            graph: graph_from(&order, &group_actions),
            group_actions,
            full_order: order.iter().map(|s| s.to_string()).collect(),
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let exit_code = execute_locked(&run_groups).await?;
        assert!(!exit_code.did_succeed);
        assert_eq!(BTreeSet::from(["dep".to_string()]), exit_code.failed_group);
        assert_eq!(
            BTreeSet::from(["dependent".to_string()]),
            exit_code.skipped_group
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_multi_root_forest_all_run() -> Result<()> {
        let group_actions = BTreeMap::from([
            make_group_action_requiring(
                "dep_1",
                vec![],
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
            make_group_action_requiring(
                "root_1",
                vec!["dep_1"],
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
            make_group_action_requiring(
                "dep_2",
                vec![],
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
            make_group_action_requiring(
                "root_2",
                vec!["dep_2"],
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
        ]);

        let order = ["dep_1", "root_1", "dep_2", "root_2"];
        let run_groups = RunGroups {
            graph: graph_from(&order, &group_actions),
            group_actions,
            full_order: order.iter().map(|s| s.to_string()).collect(),
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let exit_code = execute_locked(&run_groups).await?;
        assert!(exit_code.did_succeed);
        assert_eq!(
            BTreeSet::from(["dep_1", "root_1", "dep_2", "root_2"].map(str::to_string)),
            exit_code.succeeded_groups
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_fail_fast_across_subtrees() -> Result<()> {
        // A failure in `root_1`'s subtree must halt `root_2`'s subtree too — fail-fast is
        // global across the whole forest, not scoped to the failing root's own tree.
        let group_actions = BTreeMap::from([
            make_group_action_requiring(
                "dep_1",
                vec![],
                make_action_runs(ActionRunStatus::CheckFailedFixSucceedVerifyFailed),
            ),
            make_group_action_requiring("root_1", vec!["dep_1"], will_not_run()),
            make_group_action_requiring("dep_2", vec![], will_not_run()),
            make_group_action_requiring("root_2", vec!["dep_2"], will_not_run()),
        ]);

        let order = ["dep_1", "root_1", "dep_2", "root_2"];
        let run_groups = RunGroups {
            graph: graph_from(&order, &group_actions),
            group_actions,
            full_order: order.iter().map(|s| s.to_string()).collect(),
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let exit_code = execute_locked(&run_groups).await?;
        assert!(!exit_code.did_succeed);
        assert_eq!(
            BTreeSet::from(["dep_1".to_string()]),
            exit_code.failed_group
        );
        assert_eq!(
            BTreeSet::from(["root_1", "dep_2", "root_2"].map(str::to_string)),
            exit_code.skipped_group
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_cyclic_requires_still_runs_every_group() -> Result<()> {
        // A cyclic `requires` relationship (nothing validates against this anywhere in the
        // codebase) gives both `a` and `b` an incoming edge, so neither qualifies as a root and
        // nothing outside the cycle recurses into them either. Without the fallback pass in
        // `execute`, both would silently vanish from the run while `did_succeed` stayed true.
        let group_actions = BTreeMap::from([
            make_group_action_requiring(
                "a",
                vec!["b"],
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
            make_group_action_requiring(
                "b",
                vec!["a"],
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
        ]);

        let order = ["a", "b"];
        let run_groups = RunGroups {
            graph: graph_from(&order, &group_actions),
            group_actions,
            full_order: order.iter().map(|s| s.to_string()).collect(),
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let exit_code = execute_locked(&run_groups).await?;
        assert!(exit_code.did_succeed);
        assert_eq!(
            BTreeSet::from(["a", "b"].map(str::to_string)),
            exit_code.succeeded_groups,
            "both groups in the cycle must still run, not silently vanish"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_missing_container_does_not_orphan_its_dependencies() -> Result<()> {
        // `ghost` requires `dependency` and is present in the dependency graph, but has no
        // entry in `group_actions`. Not reachable in production today — the graph and
        // `group_actions` are always built from the same source — but `run_group_subtree` must
        // still visit a missing node's dependencies rather than orphaning them.
        let group_actions = BTreeMap::from([make_group_action_requiring(
            "dependency",
            vec![],
            make_action_runs(ActionRunStatus::CheckSucceeded),
        )]);

        let mut groups_for_graph: BTreeMap<String, DoctorGroup> = group_actions
            .iter()
            .map(|(name, container)| (name.clone(), container.group.clone()))
            .collect();
        groups_for_graph.insert(
            "ghost".to_string(),
            make_root_model_additional(
                vec![],
                |meta| meta.name("ghost"),
                |group| group.requires(vec!["dependency".to_string()]),
            ),
        );

        let order = vec!["dependency".to_string(), "ghost".to_string()];
        let graph = RunGraph::new(&order, &groups_for_graph);

        let run_groups = RunGroups {
            group_actions,
            graph,
            full_order: order,
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let exit_code = execute_locked(&run_groups).await?;
        assert!(exit_code.did_succeed);
        assert_eq!(
            BTreeSet::from(["dependency".to_string()]),
            exit_code.succeeded_groups,
            "`dependency` must still run even though its dependent `ghost` has no container"
        );
        Ok(())
    }

    /// `RunGroups::execute`/`execute_group` create `group`/`action`/`doctor run` spans at fixed
    /// call sites in `run_group_subtree`/`execute_group`, shared by every test in this module.
    /// `tracing`'s per-callsite interest cache is process-global and populated lazily: the first
    /// time any of these call sites is ever hit, in *any* concurrently-running test on *any*
    /// thread, its interest gets computed against whichever dispatcher that thread happens to
    /// have active (which, for a test with no custom subscriber, is the ambient no-op default)
    /// and then stays cached — including for tests that install their own subscriber
    /// afterward, like [`SpanCapture`]'s. So every test that exercises these call sites has to
    /// serialize against every other one, not just against tests that also capture spans;
    /// otherwise span capture is flaky depending on test scheduling. Call [`execute_locked`]
    /// / [`execute_group_locked`] instead of `RunGroups::execute`/`execute_group` directly.
    ///
    /// A `tokio::sync::Mutex` (rather than `std::sync::Mutex`) because the guard has to stay
    /// held across the `.await` — that's the whole point, the callsites are hit *during* it.
    static SPAN_CALLSITE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn execute_locked<T: DoctorActionRun>(
        run_groups: &RunGroups<T>,
    ) -> Result<PathRunResult> {
        let _serialize = SPAN_CALLSITE_LOCK.lock().await;
        run_groups.execute().await
    }

    async fn execute_group_locked<T: DoctorActionRun>(
        run_groups: &RunGroups<T>,
        group_span: &Span,
        container: &GroupActionContainer<T>,
    ) -> Result<GroupExecutionResult> {
        let _serialize = SPAN_CALLSITE_LOCK.lock().await;
        run_groups.execute_group(group_span, container).await
    }

    /// Records each span's identity (its static macro name, plus the `group.name` field when
    /// present — this distinguishes a `group` span from the `action` spans nested under it,
    /// which also carry `group.name`) and its parent's identity, in creation order. There's no
    /// existing tracing test harness in this codebase, and this is the only way to prove group
    /// spans nest under their dependent's span rather than always under `doctor run` (issue
    /// #172) — the property under test simply isn't observable from `RunGroups::execute`'s
    /// return value.
    /// (child identity, parent identity) pairs, in creation order.
    type SpanEdges = Vec<(String, Option<String>)>;

    #[derive(Default, Clone)]
    struct SpanCapture {
        names: Arc<Mutex<HashMap<u64, String>>>,
        edges: Arc<Mutex<SpanEdges>>,
    }

    impl SpanCapture {
        fn edges(&self) -> SpanEdges {
            self.edges.lock().unwrap().clone()
        }

        fn parent_of(&self, identity: &str) -> Option<String> {
            self.edges()
                .into_iter()
                .find(|(name, _)| name == identity)
                .and_then(|(_, parent)| parent)
        }
    }

    #[derive(Default)]
    struct GroupNameVisitor(Option<String>);

    impl Visit for GroupNameVisitor {
        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "group.name" {
                self.0 = Some(value.to_string());
            }
        }

        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            if field.name() == "group.name" && self.0.is_none() {
                self.0 = Some(format!("{value:?}"));
            }
        }
    }

    impl<S: tracing::Subscriber> Layer<S> for SpanCapture {
        fn on_new_span(
            &self,
            attrs: &tracing::span::Attributes<'_>,
            id: &tracing::span::Id,
            _ctx: Context<'_, S>,
        ) {
            let mut visitor = GroupNameVisitor::default();
            attrs.record(&mut visitor);
            let identity = match visitor.0 {
                Some(group_name) => format!("{}:{group_name}", attrs.metadata().name()),
                None => attrs.metadata().name().to_string(),
            };

            let parent_identity = attrs.parent().and_then(|parent_id| {
                self.names
                    .lock()
                    .unwrap()
                    .get(&parent_id.into_u64())
                    .cloned()
            });

            self.names
                .lock()
                .unwrap()
                .insert(id.into_u64(), identity.clone());
            self.edges.lock().unwrap().push((identity, parent_identity));
        }
    }

    #[tokio::test]
    async fn test_execute_nests_dependency_span_under_dependent_span() -> Result<()> {
        let group_actions = BTreeMap::from([
            make_group_action_requiring(
                "dep",
                vec![],
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
            make_group_action_requiring(
                "root",
                vec!["dep"],
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
        ]);

        let order = ["dep", "root"];
        let run_groups = RunGroups {
            graph: graph_from(&order, &group_actions),
            group_actions,
            full_order: order.iter().map(|s| s.to_string()).collect(),
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        // Hold the same lock `execute_locked`/`execute_group_locked` use, for the entire window
        // from installing the subscriber through the run completing — not just via
        // `execute_locked` — so no other test's dispatch can be active when these call sites
        // get their (process-global) interest computed. Call `execute()` directly rather than
        // through `execute_locked`, which would try to take this same non-reentrant lock again.
        let _serialize = SPAN_CALLSITE_LOCK.lock().await;
        let capture = SpanCapture::default();
        let subscriber = tracing_subscriber::registry().with(capture.clone());
        let _guard = tracing::subscriber::set_default(subscriber);
        tracing::callsite::rebuild_interest_cache();

        let exit_code = run_groups.execute().await?;
        assert!(exit_code.did_succeed);

        assert_eq!(
            Some("doctor run".to_string()),
            capture.parent_of("group:root"),
            "`root` has nothing depending on it, so it parents directly to the header span"
        );
        assert_eq!(
            Some("group:root".to_string()),
            capture.parent_of("group:dep"),
            "`dep`'s span must nest under its dependent `root`'s span, not the header span \
             (issue #172)"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_execute_diamond_dependency_span_nests_under_first_dependent_only() -> Result<()> {
        let group_actions = BTreeMap::from([
            make_group_action_requiring(
                "shared",
                vec![],
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
            make_group_action_requiring(
                "a",
                vec!["shared"],
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
            make_group_action_requiring(
                "b",
                vec!["shared"],
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
        ]);

        let order = ["shared", "a", "b"];
        let run_groups = RunGroups {
            graph: graph_from(&order, &group_actions),
            group_actions,
            full_order: order.iter().map(|s| s.to_string()).collect(),
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        // Hold the same lock `execute_locked`/`execute_group_locked` use, for the entire window
        // from installing the subscriber through the run completing — not just via
        // `execute_locked` — so no other test's dispatch can be active when these call sites
        // get their (process-global) interest computed. Call `execute()` directly rather than
        // through `execute_locked`, which would try to take this same non-reentrant lock again.
        let _serialize = SPAN_CALLSITE_LOCK.lock().await;
        let capture = SpanCapture::default();
        let subscriber = tracing_subscriber::registry().with(capture.clone());
        let _guard = tracing::subscriber::set_default(subscriber);
        tracing::callsite::rebuild_interest_cache();

        let exit_code = run_groups.execute().await?;
        assert!(exit_code.did_succeed);

        let shared_spans: Vec<_> = capture
            .edges()
            .into_iter()
            .filter(|(name, _)| name == "group:shared")
            .collect();
        assert_eq!(
            1,
            shared_spans.len(),
            "the shared dependency's span must be opened exactly once"
        );
        assert_eq!(
            Some("group:a".to_string()),
            shared_spans[0].1.clone(),
            "the shared dep nests under the first dependent (`a`) to reach it, not `b`"
        );

        assert_eq!(
            Some("doctor run".to_string()),
            capture.parent_of("group:a"),
            "`a` is a root"
        );
        assert_eq!(
            Some("doctor run".to_string()),
            capture.parent_of("group:b"),
            "`b` is a root too"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_execute_run_with_multiple_paths_only_run_group_once() -> Result<()> {
        let group_actions = BTreeMap::from([
            make_group_action("group_1", make_action_runs(ActionRunStatus::CheckSucceeded)),
            make_group_action("group_2", make_action_runs(ActionRunStatus::CheckSucceeded)),
            make_group_action("group_3", make_action_runs(ActionRunStatus::CheckSucceeded)),
        ]);

        let all_paths = vec![
            "group_1".to_string(),
            "group_2".to_string(),
            "group_3".to_string(),
        ];
        let run_groups = RunGroups {
            group_actions,
            graph: flat_graph(&all_paths),
            full_order: all_paths.clone(),
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let exit_code = execute_locked(&run_groups).await?;
        assert!(exit_code.did_succeed);
        assert_eq!(BTreeSet::from_iter(all_paths), exit_code.succeeded_groups);
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

        let all_paths = vec![
            "fails".to_string(),
            "skipped_1".to_string(),
            "skipped_2".to_string(),
        ];
        let run_groups = RunGroups {
            group_actions,
            graph: flat_graph(&all_paths),
            full_order: all_paths,
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let exit_code = execute_locked(&run_groups).await?;
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

        let all_paths = vec![
            "succeeds".to_string(),
            "user_denies".to_string(),
            "skipped".to_string(),
        ];
        let run_groups = RunGroups {
            group_actions,
            graph: flat_graph(&all_paths),
            full_order: all_paths,
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let exit_code = execute_locked(&run_groups).await?;
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

        let all_paths = vec![
            "succeeds_1".to_string(),
            "user_denies".to_string(),
            "succeeds_2".to_string(),
        ];
        let run_groups = RunGroups {
            group_actions,
            graph: flat_graph(&all_paths),
            full_order: all_paths,
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let exit_code = execute_locked(&run_groups).await?;
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

        let all_paths = vec![
            "succeeds_1".to_string(),
            "fails".to_string(),
            "succeeds_2".to_string(),
        ];
        let run_groups = RunGroups {
            group_actions,
            graph: flat_graph(&all_paths),
            full_order: all_paths,
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let exit_code = execute_locked(&run_groups).await?;
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

    #[tokio::test]
    async fn test_execute_group_check_failed_fix_failed_stop_halts_remaining_actions() -> Result<()>
    {
        let mut actions = vec![make_action_run(
            ActionRunStatus::CheckFailedFixFailedStop,
            true,
        )];
        actions.extend(will_not_run());
        let (_name, container) = make_group_action("fix-failed-stop", actions);
        let run_groups = RunGroups {
            group_actions: BTreeMap::new(),
            graph: flat_graph(&[]),
            full_order: Vec::new(),
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let group_span = info_span!("test_group", "indicatif.pb_show" = true);
        let result = execute_group_locked(&run_groups, &group_span, &container).await?;

        assert!(matches!(result.status, GroupExecutionStatus::Failed));
        assert!(result.skip_remaining);
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_group_check_failed_fix_failed_stop_halts_even_when_not_required()
    -> Result<()> {
        // `CheckFailedFixFailedStop` is an unconditional abort signal — a fix ran and
        // failed catastrophically — so it must halt the group even when the action
        // is `required: false`, unlike ordinary failures which only halt if required.
        let mut actions = vec![make_action_run(
            ActionRunStatus::CheckFailedFixFailedStop,
            false,
        )];
        actions.extend(will_not_run());
        let (_name, container) = make_group_action("fix-failed-stop-not-required", actions);
        let run_groups = RunGroups {
            group_actions: BTreeMap::new(),
            graph: flat_graph(&[]),
            full_order: Vec::new(),
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let group_span = info_span!("test_group", "indicatif.pb_show" = true);
        let result = execute_group_locked(&run_groups, &group_span, &container).await?;

        assert!(result.skip_remaining);
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_group_check_failed_no_run_fix_does_not_skip_remaining_actions()
    -> Result<()> {
        let (_name, container) = make_group_action(
            "fix-false",
            vec![
                make_action_run(ActionRunStatus::CheckFailedNoRunFix, true),
                make_action_run(ActionRunStatus::CheckSucceeded, true),
            ],
        );
        let run_groups = RunGroups {
            group_actions: BTreeMap::new(),
            graph: flat_graph(&[]),
            full_order: Vec::new(),
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let group_span = info_span!("test_group", "indicatif.pb_show" = true);
        let result = execute_group_locked(&run_groups, &group_span, &container).await?;

        assert!(matches!(result.status, GroupExecutionStatus::Failed));
        assert!(!result.skip_remaining);
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_check_failed_no_run_fix_does_not_halt_dependent_groups() -> Result<()> {
        let group_actions = BTreeMap::from([
            make_group_action(
                "fails",
                make_action_runs(ActionRunStatus::CheckFailedNoRunFix),
            ),
            make_group_action(
                "depends_on_fails",
                make_action_runs(ActionRunStatus::CheckSucceeded),
            ),
        ]);

        let all_paths = vec!["fails".to_string(), "depends_on_fails".to_string()];
        let run_groups = RunGroups {
            group_actions,
            graph: flat_graph(&all_paths),
            full_order: all_paths,
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let exit_code = execute_locked(&run_groups).await?;
        assert!(!exit_code.did_succeed);
        assert_eq!(
            BTreeSet::from(["depends_on_fails"].map(str::to_string)),
            exit_code.succeeded_groups
        );
        assert_eq!(
            BTreeSet::from(["fails"].map(str::to_string)),
            exit_code.failed_group
        );
        assert_eq!(BTreeSet::new(), exit_code.skipped_group);
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_group_status_is_sticky_once_failed() -> Result<()> {
        // The first action fails but is `required: false`, so it doesn't halt the
        // group; a later action in the same group then succeeds. The group must
        // still be reported as failed overall — a later success must not clear an
        // earlier failure.
        let (_name, container) = make_group_action(
            "mixed-results",
            vec![
                make_action_run(ActionRunStatus::CheckFailedFixSucceedVerifyFailed, false),
                make_action_run(ActionRunStatus::CheckSucceeded, true),
            ],
        );
        let run_groups = RunGroups {
            group_actions: BTreeMap::new(),
            graph: flat_graph(&[]),
            full_order: Vec::new(),
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let group_span = info_span!("test_group", "indicatif.pb_show" = true);
        let result = execute_group_locked(&run_groups, &group_span, &container).await?;

        assert!(
            matches!(result.status, GroupExecutionStatus::Failed),
            "a later succeeding action must not clear an earlier failure"
        );
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
    async fn test_should_skip_group_propagates_command_error() {
        let test_group = make_root_model_additional(
            vec![],
            |meta| meta.name("test-group"),
            |group| {
                group.skip(SkipSpec::Command {
                    command: "boom".to_string(),
                })
            },
        );

        let mut mock_exec = MockExecutionProvider::new();
        mock_exec.expect_run_command().times(1).returning(|_| {
            Err(CaptureError::MissingShExec {
                name: "boom".to_string(),
            })
        });

        let container: GroupActionContainer<MockDoctorActionRun> = GroupActionContainer {
            group: test_group,
            actions: vec![],
            exec_provider: Arc::new(mock_exec),
            exec_working_dir: Default::default(),
            sys_path: "".to_string(),
        };

        assert!(container.should_skip_group().await.is_err());
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
            graph: flat_graph(&[]),
            full_order: Vec::new(),
            skipped_groups: BTreeSet::new(),
            yolo: false,
        };

        let group_span = info_span!("test_group", "indicatif.pb_show" = true);
        let result = execute_group_locked(&run_groups, &group_span, &container).await?;

        assert_eq!(result.group_name, "test-group");
        assert!(matches!(result.status, GroupExecutionStatus::Succeeded));
        assert!(!result.skip_remaining);

        Ok(())
    }

    #[tokio::test]
    async fn test_execute_reports_planned_skips_in_their_natural_position() -> Result<()> {
        let group_actions = BTreeMap::from([
            make_group_action("before", make_action_runs(ActionRunStatus::CheckSucceeded)),
            make_group_action("after", make_action_runs(ActionRunStatus::CheckSucceeded)),
        ]);

        // "trimmed" sits between "before" and "after" in the full candidate order, even though
        // it's absent from the graph — proving the skip warning interleaves at that position
        // instead of being reported only after every runnable group has finished.
        let run_groups = RunGroups {
            group_actions,
            graph: flat_graph(&["before".to_string(), "after".to_string()]),
            full_order: vec![
                "before".to_string(),
                "trimmed".to_string(),
                "after".to_string(),
            ],
            skipped_groups: BTreeSet::from(["trimmed".to_string()]),
            yolo: false,
        };

        let exit_code = execute_locked(&run_groups).await?;

        // Groups trimmed during planning are reported as skipped without failing the run.
        assert!(exit_code.did_succeed);
        assert_eq!(
            BTreeSet::from(["before".to_string(), "after".to_string()]),
            exit_code.succeeded_groups
        );
        assert_eq!(
            BTreeSet::from(["trimmed".to_string()]),
            exit_code.skipped_group
        );

        Ok(())
    }
}
