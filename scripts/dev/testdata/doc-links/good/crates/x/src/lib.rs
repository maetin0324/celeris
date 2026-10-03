const S: &str = include_str!("../../../docs/SPEC.md");
const T: &str = "docs/adr/NNNN-x.md";
fn f(x: &str) -> String { format!("docs/{x}") }
// see https://github.com/o/r/blob/main/docs/none.md and gui/docs/none.md
