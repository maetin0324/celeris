pub mod aggregate;
pub mod candidates;
pub mod dryrun;
pub mod export;
pub mod manifest;
pub mod recheck;
pub mod run;
pub mod stats;

pub const MANIFEST_SCHEMA: &str = "celeris.routing-eval-manifest/1";

pub fn not_implemented(command: &str) -> String {
    format!("{command} is not implemented yet")
}
