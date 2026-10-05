//! Pure, deterministic model routing kernel. Callers own observations and reservations.
pub mod context;
pub mod cost;
pub mod estimator;
pub mod feedback;
pub mod optimizer;
pub mod policy;
pub mod profiles;
pub mod trace;

#[cfg(test)]
mod tests;
