use std::path::PathBuf;

use color_eyre::eyre;
use den_core::engine::Engine;

struct Tree {
    _dir: tempfile::TempDir,
    dir:  String,
    file: String,
    sub:  String,
    link: String,
}

impl Tree {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("hello.txt");
        let sub = dir.path().join("sub");
        let link = dir.path().join("hello.link");
        std::fs::write(&file, b"hello").expect("write file");
        std::fs::create_dir(&sub).expect("create subdir");
        #[cfg(unix)]
        std::os::unix::fs::symlink("hello.txt", &link).expect("symlink");
        Self {
            file: file.to_string_lossy().into_owned(),
            sub:  sub.to_string_lossy().into_owned(),
            link: link.to_string_lossy().into_owned(),
            dir:  dir.path().to_string_lossy().into_owned(),
            _dir: dir,
        }
    }
}

fn case(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/js")
        .join(name)
}

fn export_tree(tree: &Tree) {
    // SAFETY: test-only keys, set before the engine starts.
    unsafe {
        std::env::set_var("DEN_TEST_DIR", &tree.dir);
        std::env::set_var("DEN_TEST_FILE", &tree.file);
        std::env::set_var("DEN_TEST_SUB", &tree.sub);
        std::env::set_var("DEN_TEST_LINK", &tree.link);
        std::env::set_var("DEN_TEST_UNIX", if cfg!(unix) { "1" } else { "0" });
    }
}

async fn run_with_tree(name: &str) -> eyre::Result<()> {
    let tree = Tree::new();
    export_tree(&tree);
    Engine::new().await.run_file(case(name)).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn metadata_read_dir_and_set_permissions() -> eyre::Result<()> {
    run_with_tree("metadata.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn read_write_copy_and_remove_files() -> eyre::Result<()> {
    run_with_tree("read_write.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn create_rename_and_remove_directories() -> eyre::Result<()> {
    run_with_tree("dirs.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn atomic_write_replaces_the_target_and_leaves_no_temp_file() -> eyre::Result<()> {
    run_with_tree("write_atomic.js").await
}
