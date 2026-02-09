pub mod analyze;
pub mod doctor;
pub mod internal;
pub mod lint;
pub mod models;
pub mod report;
pub mod shared;

// CLI module is internal - not part of public library API
pub(crate) mod cli;

pub mod prelude {
    pub use crate::analyze::prelude::*;
    pub use crate::doctor::prelude::*;
    pub use crate::lint::prelude::*;
    pub use crate::models::prelude::*;
    pub use crate::report::prelude::*;
    pub use crate::shared::prelude::*;
}

// Re-export internal abstractions at crate root for convenience
pub use internal::progress::{NoOpProgress, ProgressReporter};
pub use internal::prompts::{AutoApprove, DenyAll, UserInteraction};

// Re-export CLI implementation for interactive applications
pub use cli::InquireInteraction;

/// Preferred way to output data to users. This macro will write the output to tracing for debugging
/// and to stdout using the global stdout writer. Because we use the stdout writer, the calls
/// will all be async.
#[macro_export]
macro_rules! report_stdout {
    ($($arg:tt)*) => {
        tracing::info!(target="stdout", $($arg)*);
        writeln!($crate::prelude::STDOUT_WRITER.write().await, $($arg)*).ok()
    };
}
