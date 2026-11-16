use std::process::Command;

fn models() -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("models")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "semlith models failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn model_dimensions_share_a_column() {
    let output = models();
    let mut columns = output
        .lines()
        .map(|line| line.find(" dim  ").expect("missing dimension column"));
    let default_column = columns.next().expect("missing default model row");
    assert!(
        columns.all(|column| column == default_column),
        "dimension columns are misaligned:\n{output}"
    );
}

#[test]
fn model_list_starts_with_the_default_then_sorts_by_name() {
    let output = models();
    let mut lines = output.lines();
    let default = lines.next().expect("missing default model row");
    assert!(default.contains(" dim  default."));
    let names: Vec<_> = lines
        .map(|line| line.split_whitespace().next().expect("missing model name"))
        .collect();
    assert!(names.len() > 1);
    assert!(
        names.windows(2).all(|pair| pair[0] < pair[1]),
        "built-in models are out of order: {names:?}"
    );
    assert_eq!(output, models());
}
