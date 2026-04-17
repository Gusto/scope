use clap::{ArgGroup, Parser, ValueEnum};
use gethostname::gethostname;
use indicatif::ProgressStyle;
use lazy_static::lazy_static;

use opentelemetry::{KeyValue, trace::TracerProvider as _};
use opentelemetry_otlp::{
    ExporterBuildError, MetricExporter, SpanExporter, WithExportConfig, WithTonicConfig,
};
use opentelemetry_sdk::{
    Resource,
    metrics::{SdkMeterProvider, periodic_reader_with_async_runtime::PeriodicReader},
    runtime,
    trace::{RandomIdGenerator, Sampler, SdkTracerProvider},
};
use tonic::metadata::MetadataMap;
use url::Url;

use std::env;
use std::fs::File;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use tracing::level_filters::LevelFilter;
use tracing::warn;
use tracing_indicatif::IndicatifLayer;
use tracing_indicatif::filter::{IndicatifFilter, hide_indicatif_span_fields};
use tracing_opentelemetry::{MetricsLayer, OpenTelemetryLayer};
use tracing_subscriber::fmt::format::DefaultFields;
use tracing_subscriber::{
    Registry,
    fmt::format::{Format, PrettyFields},
    layer::SubscriberExt,
};
use tracing_subscriber::{filter::filter_fn, prelude::*};

lazy_static! {
    static ref IGNORED_MODULES: &'static [&'static str] = &[
        "want",
        "hyper",
        "mio",
        "rustls",
        "tokio_threadpool",
        "tokio_reactor",
        "tower",
        "tonic",
        "h2",
    ];
}

pub fn default_progress_bar() -> ProgressStyle {
    ProgressStyle::with_template(
        "{span_child_prefix} {spinner:.green} {wide_msg} {pos:>7}/{len:7} [{elapsed_precise}]",
    )
    .unwrap()
    .progress_chars("##-")
}

pub fn progress_bar_without_pos() -> ProgressStyle {
    ProgressStyle::with_template(
        "{span_child_prefix} {spinner:.green} {wide_msg} [{elapsed_precise}]",
    )
    .unwrap()
    .progress_chars("##-")
}

#[derive(Parser, Debug)]
#[clap(group = ArgGroup::new("logging"))]
pub struct LoggingOpts {
    /// A level of verbosity, and can be used multiple times
    #[arg(short, long, action = clap::ArgAction::Count, global(true))]
    pub verbose: u8,

    #[arg(
        long,
        global(true),
        default_value = "auto",
        env = "SCOPE_OUTPUT_PROGRESS"
    )]
    /// Set the progress output. Use plain to disable updating UI.
    pub progress: LoggingProgress,

    #[arg(skip = LevelFilter::WARN)]
    default_level: LevelFilter,

    /// When set metrics will be sent to an otel collector at the endpoint provided
    #[clap(long = "otel-collector", env = "SCOPE_OTEL_ENDPOINT", global(true))]
    otel_collector: Option<String>,

    #[clap(
        long = "otel-protocol",
        env = "SCOPE_OTEL_PROTOCOL",
        global(true),
        default_value = "grpc"
    )]
    otel_protocol: OtelProtocol,

    #[clap(
        long = "otel-service",
        env = "SCOPE_OTEL_SERVICE",
        global(true),
        default_value = "scope"
    )]
    otel_service: String,

    /// When set, we'll send debug details to otel endpoint.
    /// This option is hidden when running --help
    #[arg(long, hide = true, global(true))]
    pub otel_debug: bool,
}

#[derive(ValueEnum, Debug, Copy, Clone)]
pub enum OtelProtocol {
    Http,
    Grpc,
}

#[derive(ValueEnum, Debug, Copy, Clone)]
pub enum LoggingProgress {
    /// Determine output format based on execution context
    Auto,
    /// Standard output, no progress bar, no auto-updating output.
    Plain,
    /// Use progress bar
    Tty,
}

impl LoggingProgress {
    fn is_tty(&self) -> bool {
        match self {
            LoggingProgress::Auto => std::io::stdout().is_terminal(),
            LoggingProgress::Plain => false,
            LoggingProgress::Tty => true,
        }
    }
}

lazy_static! {
    pub static ref STDOUT_WRITER: Arc<RwLock<Box<dyn Write + Sync + Send>>> =
        Arc::new(RwLock::new(Box::new(std::io::stdout())));
    pub static ref STDERR_WRITER: Arc<RwLock<Box<dyn Write + Sync + Send>>> =
        Arc::new(RwLock::new(Box::new(std::io::stderr())));
}

pub struct ConfiguredLogger {
    /// needed to drop otel and finish the last write later
    _otel: Option<OtelProperties>,
    /// needed to keep the logger running
    _guard: tracing_appender::non_blocking::WorkerGuard,
    pub log_location: String,
}

/// RAII wrapper that ensures metrics and traces are flushed on shutdown
struct OtelProperties {
    tracer: SdkTracerProvider,
    metrics: SdkMeterProvider,
}

impl Drop for OtelProperties {
    fn drop(&mut self) {
        if let Err(e) = self.metrics.shutdown() {
            warn!("Unable to emit final metrics: {:?}", e);
        }
        if let Err(e) = self.tracer.shutdown() {
            warn!("Unable to emit final traces: {:?}", e);
        }
    }
}

impl LoggingOpts {
    pub fn with_new_default(&self, new_default: LevelFilter) -> Self {
        Self {
            verbose: self.verbose,
            progress: self.progress,
            default_level: new_default,
            otel_collector: self.otel_collector.clone(),
            otel_protocol: self.otel_protocol,
            otel_service: self.otel_service.clone(),
            otel_debug: self.otel_debug,
        }
    }

    pub fn to_level_filter(&self) -> LevelFilter {
        match self.verbose {
            0 => self.default_level,
            1 => LevelFilter::INFO,
            2 => LevelFilter::DEBUG,
            _ => LevelFilter::TRACE,
        }
    }

    fn setup_otel(&self, run_id: &str) -> Result<Option<OtelProperties>, anyhow::Error> {
        if self.otel_collector.is_some() {
            let resource = self.resource(run_id);
            let metadata_map = self.metadata_map(run_id);
            let timeout = Duration::from_secs(3);
            let endpoint = &self.otel_collector.clone().unwrap();

            let tracer = self.init_tracer_provider(&resource, &metadata_map, endpoint, &timeout)?;
            let metrics = self.init_meter_provider(&resource, &metadata_map, endpoint, &timeout)?;

            Ok(Some(OtelProperties { metrics, tracer }))
        } else {
            Ok(None)
        }
    }

    fn init_tracer_provider(
        &self,
        resource: &Resource,
        metadata_map: &MetadataMap,
        endpoint: &str,
        timeout: &Duration,
    ) -> Result<SdkTracerProvider, ExporterBuildError> {
        let span_exporter = match self.otel_protocol {
            OtelProtocol::Grpc => SpanExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .with_timeout(*timeout)
                .with_metadata(metadata_map.clone())
                .build(),
            OtelProtocol::Http => {
                // with_endpoint() is _roughly_ equivalent to using the OTEL_EXPORTER_OTLP_TRACES_ENDPOINT env var
                // So we have to append the `v1/traces` to the base URL when using http
                // https://opentelemetry.io/docs/languages/sdk-configuration/otlp-exporter/#otel_exporter_otlp_traces_endpoint
                // https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/protocol/exporter.md#endpoint-urls-for-otlphttp
                let url = Url::parse(endpoint)
                    .expect("Expected a valid URI")
                    .join("v1/traces")
                    .unwrap();
                SpanExporter::builder()
                    .with_http()
                    .with_endpoint(url)
                    .with_timeout(*timeout)
                    .build()
            }
        };

        let tracer = SdkTracerProvider::builder()
            .with_batch_exporter(span_exporter?)
            .with_sampler(Sampler::AlwaysOn)
            .with_id_generator(RandomIdGenerator::default())
            .with_max_attributes_per_span(128)
            .with_max_events_per_span(128)
            .with_resource(resource.clone())
            .build();

        Ok(tracer)
    }

    fn init_meter_provider(
        &self,
        resource: &Resource,
        metadata_map: &MetadataMap,
        endpoint: &str,
        timeout: &Duration,
    ) -> Result<SdkMeterProvider, ExporterBuildError> {
        let metric_exporter = match self.otel_protocol {
            OtelProtocol::Grpc => MetricExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .with_timeout(*timeout)
                .with_metadata(metadata_map.clone())
                .build(),
            OtelProtocol::Http => MetricExporter::builder()
                .with_http()
                .with_endpoint(endpoint)
                .with_timeout(*timeout)
                .build(),
        };

        let reader = PeriodicReader::builder(metric_exporter?, runtime::Tokio)
            .with_interval(Duration::from_secs(3))
            .with_timeout(Duration::from_secs(10))
            .build();

        let metrics = SdkMeterProvider::builder()
            .with_reader(reader)
            .with_resource(resource.clone())
            .build();

        Ok(metrics)
    }

    fn resource(&self, run_id: &str) -> Resource {
        Resource::builder_empty()
            .with_attributes([
                KeyValue::new("service.name", self.otel_service.clone()),
                KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                KeyValue::new(
                    "host.name",
                    gethostname()
                        .into_string()
                        .unwrap_or_else(|_| "unknown".to_string()),
                ),
                KeyValue::new("scope.id", run_id.to_string()),
            ])
            .build()
    }

    fn metadata_map(&self, run_id: &str) -> MetadataMap {
        let mut metadata_map = MetadataMap::with_capacity(2);
        metadata_map.insert(
            "host",
            gethostname()
                .into_string()
                .unwrap_or_else(|_| "unknown".to_string())
                .parse()
                .unwrap(),
        );
        metadata_map.insert("scope.id", run_id.parse().unwrap());

        metadata_map
    }

    pub async fn configure_logging(&self, run_id: &str, prefix: &str) -> ConfiguredLogger {
        let file_name = format!("scope-{prefix}-{run_id}.log");
        let full_file_name = format!("/tmp/scope/{file_name}");
        std::fs::create_dir_all("/tmp/scope").expect("to be able to create tmp dir");

        let otel_props = self.setup_otel(run_id).unwrap_or_else(|e| {
            println!("opentelemetry configuration failed. Events will not be sent. {e:?}");
            None
        });

        let file_path = PathBuf::from(&full_file_name);
        let (non_blocking, guard) =
            tracing_appender::non_blocking(strip_ansi_escapes::Writer::new(
                File::create(file_path).expect("to be able to create log file"),
            ));

        let file_output = tracing_subscriber::fmt::layer()
            .event_format(Format::default().pretty())
            .with_ansi(false)
            .with_writer(non_blocking);

        let indicatif_layer = IndicatifLayer::new()
            .with_span_field_formatter(hide_indicatif_span_fields(DefaultFields::new()))
            .with_progress_style(default_progress_bar());
        let indicatif_writer = indicatif_layer.get_stdout_writer();

        *STDOUT_WRITER.write().await = Box::new(indicatif_layer.get_stdout_writer());
        *STDERR_WRITER.write().await = Box::new(indicatif_layer.get_stderr_writer());

        let is_tty_output = self.progress.is_tty();

        let level_filter = self.to_level_filter();
        let console_output = tracing_subscriber::fmt::layer()
            .event_format(
                Format::default()
                    .with_target(false)
                    .without_time()
                    .compact(),
            )
            .with_writer(indicatif_writer)
            .fmt_fields(PrettyFields::new())
            .with_filter(filter_fn(move |metadata| match metadata.target() {
                "user" => level_filter >= *metadata.level(),
                "always" => true,
                "progress" => !is_tty_output,
                "stdout" => false,
                _ => false,
            }));

        let progress_layer = if is_tty_output {
            Some(indicatif_layer.with_filter(IndicatifFilter::new(false)))
        } else {
            None
        };

        let otel_level = if self.otel_debug {
            LevelFilter::DEBUG
        } else {
            LevelFilter::INFO
        };
        let filter_func = filter_fn(move |metadata| {
            if metadata
                .module_path()
                .map(|x| IGNORED_MODULES.iter().any(|module| x.starts_with(module)))
                .unwrap_or(false)
            {
                return false;
            }

            otel_level >= *metadata.level()
        });

        let (otel_tracer_layer, otel_metrics_layer) = match otel_props {
            Some(ref props) => (
                Some(
                    OpenTelemetryLayer::new(props.tracer.tracer("scope"))
                        .with_filter(filter_func.clone()),
                ),
                Some(MetricsLayer::new(props.metrics.clone()).with_filter(filter_func)),
            ),
            None => (None, None),
        };

        let subscriber = Registry::default()
            .with(otel_metrics_layer)
            .with(otel_tracer_layer)
            .with(console_output)
            .with(progress_layer)
            .with(file_output);

        tracing::subscriber::set_global_default(subscriber)
            .expect("setting default subscriber failed");

        ConfiguredLogger {
            _guard: guard,
            log_location: full_file_name,
            _otel: otel_props,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_span_exporter_builds_with_default_client() {
        let url = Url::parse("http://127.0.0.1:4318/v1/traces").unwrap();
        let result = SpanExporter::builder()
            .with_http()
            .with_endpoint(url)
            .with_timeout(Duration::from_secs(3))
            .build();
        assert!(result.is_ok(), "HTTP span exporter must build; got {:?}", result.err());
    }

    #[test]
    fn http_metric_exporter_builds_with_default_client() {
        let result = MetricExporter::builder()
            .with_http()
            .with_endpoint("http://127.0.0.1:4318")
            .with_timeout(Duration::from_secs(3))
            .build();
        assert!(result.is_ok(), "HTTP metric exporter must build; got {:?}", result.err());
    }
}
