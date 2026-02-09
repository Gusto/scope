mod api;
mod check;
mod cli;
mod commands;
mod error;
mod file_cache;
pub mod options;
mod runner;
#[cfg(test)]
mod tests;

pub mod prelude {
    pub use super::cli::DoctorArgs;
    pub use super::cli::doctor_root;
    pub use super::commands::generate_doctor_list;
}

// Re-export key types for library usage
pub use options::DoctorRunOptions;
pub use runner::PathRunResult;

// Public API functions
pub use api::{run, list};
