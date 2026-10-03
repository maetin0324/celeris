//! A tiny greeting crate used as the Phase 4 (the former `docs/DESIGN.md` §6, removed 2026-10-03) claude-code
//! dogfooding target: the task is to add a usage example to `README.md`.

/// Returns a greeting for `name`.
pub fn greet(name: &str) -> String {
    format!("Hello, {name}!")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greet_includes_name() {
        assert_eq!(greet("world"), "Hello, world!");
    }
}
