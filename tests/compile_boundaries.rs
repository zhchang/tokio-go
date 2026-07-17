use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn write_case(root: &Path, name: &str, source: &str) -> PathBuf {
    let case = root.join(name);
    fs::create_dir_all(case.join("src")).expect("create downstream case directory");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let manifest = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\ntokio-go = {{ path = \"{manifest_dir}\" }}\n"
    );
    fs::write(case.join("Cargo.toml"), manifest).expect("write downstream manifest");
    fs::write(case.join("src/main.rs"), source).expect("write downstream source");
    case
}

fn check_case(case: &Path, target: &Path) -> Output {
    Command::new(env!("CARGO"))
        .args(["check", "--offline", "--quiet"])
        .current_dir(case)
        .env("CARGO_TARGET_DIR", target)
        .output()
        .expect("run downstream cargo check")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn downstream_ownership_send_and_must_use_boundaries_are_specific() {
    let root = std::env::temp_dir().join(format!(
        "tokio-go-compile-boundaries-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).expect("remove stale compile-boundary directory");
    }
    fs::create_dir_all(&root).expect("create compile-boundary root");
    let target = root.join("target");

    let owned = write_case(
        &root,
        "owned_capture",
        r#"
use std::sync::Arc;
use tokio_go::go;

fn main() {
    let text = String::from("owned");
    let shared = Arc::new(7usize);
    let task = go!(async move { (text, shared) });
    drop(task);
}
"#,
    );
    let output = check_case(&owned, &target);
    assert!(
        output.status.success(),
        "owned/Arc case failed: {}",
        stderr(&output)
    );

    let borrowed = write_case(
        &root,
        "borrowed_capture",
        r#"
use tokio_go::go;

fn main() {
    let text = String::from("borrowed");
    let task = go!(async { text.len() });
    drop(task);
}
"#,
    );
    let output = check_case(&borrowed, &target);
    let error = stderr(&output);
    assert!(
        !output.status.success(),
        "borrowed local unexpectedly compiled"
    );
    assert!(
        error.contains("async block may outlive the current function")
            && error.contains("borrows `text`"),
        "borrowed-local failure was not the expected lifetime diagnostic: {error}"
    );

    let non_send = write_case(
        &root,
        "non_send_capture",
        r#"
use std::rc::Rc;
use tokio_go::go;

fn main() {
    let value = Rc::new(7usize);
    let task = go!(async move { *value });
    drop(task);
}
"#,
    );
    let output = check_case(&non_send, &target);
    let error = stderr(&output);
    assert!(!output.status.success(), "Rc capture unexpectedly compiled");
    assert!(
        error.contains("cannot be sent between threads safely") && error.contains("Rc<usize>"),
        "Rc failure was not the expected Send diagnostic: {error}"
    );

    let must_use = write_case(
        &root,
        "must_use_handle",
        r#"
#![deny(unused_must_use)]
use tokio_go::go;

fn main() {
    go!(async move { 7usize });
}
"#,
    );
    let output = check_case(&must_use, &target);
    let error = stderr(&output);
    assert!(
        !output.status.success(),
        "discarded GoTask unexpectedly compiled"
    );
    assert!(
        error.contains("unused `GoTask`") && error.contains("must be used"),
        "discarded-handle failure was not the expected must_use diagnostic: {error}"
    );

    fs::remove_dir_all(&root).expect("remove compile-boundary root");
}
