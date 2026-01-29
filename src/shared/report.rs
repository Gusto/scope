use super::capture::OutputCapture;
use super::config_load::FoundConfig;
use super::models::prelude::ReportUploadLocationDestination;
use crate::prelude::{ExecutionProvider, ReportUploadLocation};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use derive_builder::Builder;
use itertools::Itertools;
use jsonwebtoken::EncodingKey;
use minijinja::context;
use normpath::PathExt;
use octocrab::Octocrab;
use octocrab::models::{AppId, InstallationToken};
use octocrab::params::apps::CreateInstallationAccessToken;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use tracing::{debug, info, warn};
use url::Url;

impl ReportUploadLocationDestination {
    async fn upload(&self, title: &str, report: &str) -> Result<()> {
        match self {
            ReportUploadLocationDestination::RustyPaste { url } => {
                ReportUploadLocationDestination::upload_to_rusty_paste(url, report).await
            }
            ReportUploadLocationDestination::GithubIssue { owner, repo, tags } => {
                ReportUploadLocationDestination::upload_to_github_issue(
                    owner,
                    repo,
                    tags.clone(),
                    title,
                    report,
                )
                .await
            }
            ReportUploadLocationDestination::Local { destination } => {
                let id = nanoid::nanoid!(10, &nanoid::alphabet::SAFE);
                fs::create_dir_all(destination)?;
                let file_path = format!("{destination}/scope-{id}.md");
                let mut file = File::create(&file_path)?;
                file.write_all(report.as_bytes())?;

                // make this path nicer
                let file_path = PathBuf::from(&file_path)
                    .normalize()
                    .map(|x| x.into_path_buf().display().to_string())
                    .unwrap_or(file_path);
                info!(target: "always", "Report was created at {}", file_path);

                Ok(())
            }
        }
    }

    async fn upload_to_github_issue(
        owner: &str,
        repo: &str,
        tags: Vec<String>,
        title: &str,
        report: &str,
    ) -> Result<()> {
        let client = get_octocrab(repo).await?;

        let res = client
            .issues(owner, repo)
            .create(title)
            .body(report)
            .labels(tags)
            .send()
            .await;

        match res {
            Ok(issue) => {
                debug!("Created issue was {:?}", issue);
                info!(target: "always", "Report was uploaded to {}.", issue.html_url)
            }
            Err(e) => {
                warn!(target: "always", "Unable to upload report to GitHub because {}", e)
            }
        }

        Ok(())
    }

    async fn upload_to_rusty_paste(url: &str, report: &str) -> Result<()> {
        let client = reqwest::Client::new();
        let some_file = reqwest::multipart::Part::stream(report.to_string())
            .file_name("file")
            .mime_str("text/plain")?;

        let form = reqwest::multipart::Form::new().part("file", some_file);

        let res = client.post(url).multipart(form).send().await;

        match res {
            Ok(res) => {
                debug!(server = "RustyPaste", "API Response was {:?}", res);
                let status = res.status();
                match res.text().await {
                    Err(e) => {
                        warn!(target: "user",server="RustyPaste",  "Unable to fetch body from Server: {:?}", e)
                    }
                    Ok(body) => {
                        let body = body.trim();
                        if !status.is_success() {
                            info!(target: "always", server="RustyPaste", "Report was uploaded to {}.", body)
                        } else {
                            info!(target: "always", server="RustyPaste", "Report upload failed for {}.", body)
                        }
                    }
                }
            }
            Err(e) => {
                warn!(target: "always", server="RustyPaste", "Unable to upload report to server because {}", e)
            }
        }
        Ok(())
    }
}

async fn get_octocrab(repo: &str) -> Result<Octocrab> {
    match (
        std::env::var("SCOPE_GH_APP_ID"),
        std::env::var("SCOPE_GH_APP_KEY"),
        std::env::var("SCOPE_GH_TOKEN"),
    ) {
        (Ok(app_id), Ok(app_key), _) => {
            // Influenced by https://github.com/XAMPPRocky/octocrab/blob/main/examples/github_app_authentication_manual.rs
            let app_id = app_id.parse::<u64>()?;
            let app_key = EncodingKey::from_rsa_pem(app_key.as_bytes())?;

            let client = Octocrab::builder().app(AppId(app_id), app_key).build()?;

            let installations = client
                .apps()
                .installations()
                .send()
                .await
                .unwrap()
                .take_items();

            let mut create_access_token = CreateInstallationAccessToken::default();
            create_access_token.repositories = vec![repo.to_string()];

            let access_token_url =
                Url::parse(installations[0].access_tokens_url.as_ref().unwrap()).unwrap();

            let access: InstallationToken = client
                .post(access_token_url.path(), Some(&create_access_token))
                .await
                .unwrap();

            Ok(Octocrab::builder().personal_token(access.token).build()?)
        }
        (_, _, Ok(token)) => Ok(Octocrab::builder().personal_token(token).build()?),
        (_, _, _) => Err(anyhow!("No GitHub auth configured")),
    }
}

#[derive(Debug, Clone, Default, Builder)]
#[builder(setter(into))]
pub struct AdditionalDataReport {
    #[builder(default)]
    pub name: String,

    #[builder(default)]
    pub command: String,

    #[builder(default)]
    pub output: String,
}

#[derive(Debug, Clone, Default, Builder)]
#[builder(setter(into))]
pub struct ActionTaskReport {
    #[builder(default)]
    pub command: String,
    #[builder(default)]
    pub output: Option<String>,
    #[builder(default)]
    pub exit_code: Option<i32>,
    #[builder(default)]
    pub start_time: DateTime<Utc>,
    #[builder(default)]
    pub end_time: DateTime<Utc>,
}

impl From<&OutputCapture> for ActionTaskReport {
    fn from(value: &OutputCapture) -> Self {
        ActionTaskReport {
            exit_code: value.exit_code,
            output: Some(value.generate_user_output()),
            command: value.command.clone(),
            start_time: value.start_time,
            end_time: value.end_time,
        }
    }
}

#[derive(Debug, Clone, Default, Builder)]
#[builder(setter(into))]
pub struct ActionReport {
    #[builder(default)]
    pub action_name: String,
    #[builder(default)]
    pub check: Vec<ActionTaskReport>,
    #[builder(default)]
    pub fix: Vec<ActionTaskReport>,
    #[builder(default)]
    pub validate: Vec<ActionTaskReport>,
}

#[derive(Debug, Clone)]
pub struct GroupReport {
    group_name: String,
    action_result: Vec<ActionReport>,
    additional_data: Vec<AdditionalDataReport>,
}

impl GroupReport {
    pub fn add_action(&mut self, action_report: &ActionReport) {
        self.action_result.push(action_report.clone());
    }

    pub fn add_additional_details(&mut self, name: &str, command: &str, value: &str) {
        self.additional_data.push(AdditionalDataReport {
            name: name.to_string(),
            command: command.to_string(),
            output: value.to_string(),
        });
    }

    pub fn new(group_name: &str) -> Self {
        Self {
            group_name: group_name.to_string(),
            action_result: Vec::new(),
            additional_data: Vec::new(),
        }
    }
}

// Shared
pub struct Report {
    title: String,
    body: String,
    destination: ReportUploadLocation,
}

impl Report {
    pub fn title(&self) -> String {
        self.title.clone()
    }

    pub fn body(&self) -> String {
        self.body.clone()
    }

    pub async fn distribute(&self) -> Result<()> {
        if let Err(e) = &self
            .destination
            .destination
            .upload(&self.title, &self.body)
            .await
        {
            warn!(target: "user", "Unable to upload to {}: {}", self.destination.metadata.name(), e);
        }

        Ok(())
    }
}

pub trait ReportRenderer {
    fn render(&self, destination: &ReportUploadLocation) -> Result<Report>;
}

// Unstructured reports
#[async_trait]
pub trait UnstructuredReportBuilder {
    async fn run_and_append_additional_data(
        &mut self,
        found_config: &FoundConfig,
        exec_runner: Arc<dyn ExecutionProvider>,
        commands: &BTreeMap<String, String>,
    ) -> Result<()>;
}

#[derive(Clone, Debug)]
pub struct DefaultUnstructuredReportBuilder {
    entrypoint: String,
    capture: OutputCapture,
    additional_data: Vec<AdditionalDataReport>,
}

impl DefaultUnstructuredReportBuilder {
    pub fn new(entrypoint: &str, capture: &OutputCapture) -> Self {
        Self {
            entrypoint: entrypoint.to_string(),
            capture: capture.clone(),
            additional_data: vec![],
        }
    }
}

#[async_trait]
impl UnstructuredReportBuilder for DefaultUnstructuredReportBuilder {
    async fn run_and_append_additional_data(
        &mut self,
        found_config: &FoundConfig,
        exec_provider: Arc<dyn ExecutionProvider>,
        commands: &BTreeMap<String, String>,
    ) -> Result<()> {
        for (name, command) in commands {
            let output = exec_provider
                .run_for_output(&found_config.bin_path, &found_config.working_dir, command)
                .await;
            self.additional_data.push(AdditionalDataReport {
                name: name.to_string(),
                command: command.to_string(),
                output,
            });
        }

        Ok(())
    }
}

impl ReportRenderer for DefaultUnstructuredReportBuilder {
    fn render(&self, destination: &ReportUploadLocation) -> Result<Report> {
        let title = self.render_title(destination)?;
        let body = self.render_body(destination)?;

        Ok(Report {
            title,
            body,
            destination: destination.clone(),
        })
    }
}

impl DefaultUnstructuredReportBuilder {
    fn render_title(&self, destination: &ReportUploadLocation) -> Result<String> {
        destination
            .templates
            .render_title(context! { entrypoint => self.entrypoint })
    }

    fn render_body(&self, destination: &ReportUploadLocation) -> Result<String> {
        let ctx = context! {
            command => self.capture.command,
            entrypoint => self.entrypoint,
            result => ReportCommandResultContext::from(&ActionTaskReport::from(&self.capture)),
            additionalData => self.additional_data.iter().map(ReportAdditionalDataContext::from).collect_vec(),
        };
        let rendered = destination.templates.render_analyze(ctx)?;

        Ok(rendered)
    }
}

// Grouped reports
#[async_trait]
pub trait GroupedReportBuilder {
    fn append_group(&mut self, group_result: &GroupReport) -> Result<()>;

    async fn run_and_append_additional_data(
        &mut self,
        found_config: &FoundConfig,
        exec_provider: Arc<dyn ExecutionProvider>,
        commands: &BTreeMap<String, String>,
    ) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct DefaultGroupedReportBuilder {
    entrypoint: String,
    groups: Vec<GroupReport>,
    additional_data: Vec<AdditionalDataReport>,
}

impl DefaultGroupedReportBuilder {
    pub fn new(entrypoint: &str) -> Self {
        Self {
            entrypoint: entrypoint.to_string(),
            groups: Vec::new(),
            additional_data: Vec::new(),
        }
    }
}

#[async_trait]
impl GroupedReportBuilder for DefaultGroupedReportBuilder {
    fn append_group(&mut self, group_result: &GroupReport) -> Result<()> {
        self.groups.push(group_result.clone());

        Ok(())
    }

    async fn run_and_append_additional_data(
        &mut self,
        found_config: &FoundConfig,
        exec_provider: Arc<dyn ExecutionProvider>,
        commands: &BTreeMap<String, String>,
    ) -> Result<()> {
        for (name, command) in commands {
            let output = exec_provider
                .run_for_output(&found_config.bin_path, &found_config.working_dir, command)
                .await;
            self.additional_data.push(AdditionalDataReport {
                name: name.to_string(),
                command: command.to_string(),
                output,
            });
        }

        Ok(())
    }
}

impl ReportRenderer for DefaultGroupedReportBuilder {
    fn render(&self, destination: &ReportUploadLocation) -> Result<Report> {
        let title = self.render_title(destination)?;
        let body = self.render_body(destination)?;

        Ok(Report {
            title,
            body,
            destination: destination.clone(),
        })
    }
}

impl DefaultGroupedReportBuilder {
    fn render_title(&self, destination: &ReportUploadLocation) -> Result<String> {
        destination
            .templates
            .render_title(context! { entrypoint => self.entrypoint })
    }

    fn render_body(&self, destination: &ReportUploadLocation) -> Result<String> {
        let ctx = context! {
            command => self.entrypoint,
            entrypoint => self.entrypoint,
            groups => self.groups.iter().map(ReportGroupItemContext::from).collect_vec(),
            additionalData => self.additional_data.iter().map(ReportAdditionalDataContext::from).collect_vec(),
        };
        let rendered = destination.templates.render_doctor(ctx)?;

        Ok(rendered)
    }
}

// Rendering objects
#[derive(Serialize, Deserialize, Debug)]
struct ReportCommandResultContext {
    command: String,

    #[serde(rename = "exitCode")]
    exit_code: i32,

    #[serde(rename = "startTime")]
    start_time: String,

    #[serde(rename = "endTime")]
    end_time: String,

    output: String,
}

impl ReportCommandResultContext {
    fn from(report: &ActionTaskReport) -> Self {
        Self {
            command: report.command.to_string(),
            exit_code: report.exit_code.unwrap_or(-1),
            start_time: report.start_time.to_string(),
            end_time: report.end_time.to_string(),
            output: report.output.clone().unwrap_or("".to_string()),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct ReportAdditionalDataContext {
    name: String,
    command: String,
    output: String,
}

impl ReportAdditionalDataContext {
    fn from(report: &AdditionalDataReport) -> Self {
        Self {
            name: report.name.to_string(),
            command: report.command.to_string(),
            output: report.output.to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct ReportGroupItemContext {
    name: String,

    #[serde(default)]
    actions: Vec<ReportActionItemContext>,

    #[serde(default, rename = "additionalData")]
    additional_data: Vec<ReportAdditionalDataContext>,
}

impl ReportGroupItemContext {
    fn from(report: &GroupReport) -> Self {
        Self {
            name: report.group_name.to_string(),
            actions: report
                .action_result
                .iter()
                .map(ReportActionItemContext::from)
                .collect(),
            additional_data: report
                .additional_data
                .iter()
                .map(ReportAdditionalDataContext::from)
                .collect(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct ReportActionItemContext {
    name: String,

    #[serde(default)]
    check: Vec<ReportCommandResultContext>,

    #[serde(default)]
    fix: Vec<ReportCommandResultContext>,

    #[serde(default)]
    verify: Vec<ReportCommandResultContext>,
}

impl ReportActionItemContext {
    fn from(report: &ActionReport) -> Self {
        Self {
            name: report.action_name.to_string(),
            check: report
                .check
                .iter()
                .map(ReportCommandResultContext::from)
                .collect(),
            fix: report
                .fix
                .iter()
                .map(ReportCommandResultContext::from)
                .collect(),
            verify: report
                .validate
                .iter()
                .map(ReportCommandResultContext::from)
                .collect(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

    use anyhow::Result;
    use chrono::DateTime;

    use crate::models::prelude::ModelMetadata;
    use crate::shared::models::prelude::{
        ReportTemplates, ReportUploadLocation, ReportUploadLocationDestination,
    };
    use crate::shared::prelude::{FoundConfig, MockExecutionProvider, OutputCaptureBuilder};
    use super::{
        ActionReport, ActionTaskReport, DefaultGroupedReportBuilder,
        DefaultUnstructuredReportBuilder, GroupReport, GroupedReportBuilder, ReportRenderer,
        UnstructuredReportBuilder,
    };

    #[tokio::test]
    async fn test_grouped_report_builder() -> Result<()> {
        let found_config = FoundConfig::empty(PathBuf::from("/tmp"));
        let mut exec_provider = MockExecutionProvider::new();

        let mut templates = ReportTemplates::default();
        templates.add_template("message", "# Error\nAn error occured with {{ command }}");

        let report_destination = ReportUploadLocation {
            full_name: "ReportUploadLocation/test".to_string(),
            metadata: ModelMetadata::new("test"),
            destination: ReportUploadLocationDestination::Local {
                destination: "/tmp/test".to_string(),
            },
            templates,
            additional_data: Default::default(),
        };

        let additional_data = BTreeMap::from([("baz".to_string(), "baz".to_string())]);

        exec_provider
            .expect_run_for_output()
            .times(1)
            .withf(move |_, _, command| command.eq("baz"))
            .returning(move |_, _, _| "qux".to_string());

        let mut group = GroupReport::new("g_first");
        group.add_action(&ActionReport {
            action_name: "a_first".to_string(),
            check: vec![ActionTaskReport {
                command: "action first".to_string(),
                output: Some("first line\nsecond line\n".to_string()),
                exit_code: Some(0),
                start_time: DateTime::from_timestamp(1715612600, 0).unwrap(),
                end_time: DateTime::from_timestamp(1715612699, 0).unwrap(),
            }],
            fix: vec![],
            validate: vec![],
        });

        let mut builder = DefaultGroupedReportBuilder::new("hello world");
        builder.append_group(&group)?;
        builder
            .run_and_append_additional_data(
                &found_config,
                Arc::new(exec_provider),
                &additional_data,
            )
            .await?;

        let report = builder.render(&report_destination)?;

        let expected_title = "Scope bug report: `hello world`".to_string();
        assert_eq!(expected_title, report.title);

        let expected_body = "# Error
An error occured with hello world

**Additional Capture Data**

| Name | Value |
|---|---|
|baz|`qux`|

## Group g_first

### Action g_first/a_first

---
Check Command: `action first`

Output:
```text
first line
second line

```

|Name|Value|
|:---|:---|
| Exit code| `0` |
| Started at| `2024-05-13 15:03:20 UTC` |
| Finished at| `2024-05-13 15:04:59 UTC` |



"
        .to_string();
        assert_eq!(expected_body, report.body);

        Ok(())
    }

    #[tokio::test]
    async fn test_unstructured_report_builder() -> Result<()> {
        let found_config = FoundConfig::empty(PathBuf::from("/tmp"));
        let mut exec_provider = MockExecutionProvider::new();

        let mut templates = ReportTemplates::default();
        templates.add_template("message", "# Error\nAn error occured with `{{ command }}`.");

        let report_destination = ReportUploadLocation {
            full_name: "ReportUploadLocation/test".to_string(),
            metadata: ModelMetadata::new("test"),
            destination: ReportUploadLocationDestination::Local {
                destination: "/tmp/test".to_string(),
            },
            templates,
            additional_data: Default::default(),
        };

        let additional_data = BTreeMap::from([
            ("baz".to_string(), "baz".to_string()),
            ("lines".to_string(), "lines".to_string()),
        ]);

        exec_provider
            .expect_run_for_output()
            .times(1)
            .withf(move |_, _, command| command.eq("baz"))
            .returning(move |_, _, _| "qux".to_string());

        exec_provider
            .expect_run_for_output()
            .times(1)
            .withf(move |_, _, command| command.eq("lines"))
            .returning(move |_, _, _| "line 1\nline2".to_string());

        let capture = OutputCaptureBuilder::default()
            .command("hello world")
            .stdout(vec![(
                DateTime::from_timestamp(1715612600, 0).unwrap(),
                "stdout".to_string(),
            )])
            .stderr(vec![(
                DateTime::from_timestamp(1715612601, 0).unwrap(),
                "stderr".to_string(),
            )])
            .exit_code(1)
            .start_time(DateTime::from_timestamp(1715612599, 0).unwrap())
            .end_time(DateTime::from_timestamp(1715612602, 0).unwrap())
            .build()?;

        let mut builder = DefaultUnstructuredReportBuilder::new("hello world", &capture);
        builder
            .run_and_append_additional_data(
                &found_config,
                Arc::new(exec_provider),
                &additional_data,
            )
            .await?;

        let report = builder.render(&report_destination)?;

        let expected_title = "Scope bug report: `hello world`".to_string();
        assert_eq!(expected_title, report.title);

        let expected_body = "# Error
An error occured with `hello world`.

## Command `hello world`

Output:
```text
stdout
stderr
```

|Name|Value|
|:---|:---|
| Exit code| `1` |
| Started at| `2024-05-13 15:03:19 UTC` |
| Finished at| `2024-05-13 15:03:22 UTC` |

**Additional Capture Data**

| Name | Value |
|---|---|
|baz|`qux`|
|lines|`line 1`<br>`line2`|
"
        .to_string();
        assert_eq!(expected_body, report.body);

        Ok(())
    }
}
