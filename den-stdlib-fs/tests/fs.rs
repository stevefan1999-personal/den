use color_eyre::eyre;
use den_core::engine::Engine;
use rquickjs::FromJs;

/// A temp tree with a file, a directory, and (on Unix) a symlink to the file.
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

async fn eval<T>(tree: &Tree, source: &str) -> eyre::Result<T>
where
    T: for<'js> FromJs<'js> + Send + Sync + 'static,
{
    Ok(Engine::new()
        .await
        .eval(&format!(
            "const fs = await import('den:fs');\nconst DIR = {dir:?};\nconst FILE = \
             {file:?};\nconst SUB = {sub:?};\nconst LINK = {link:?};\nconst UNIX = \
             {unix};\n{source}",
            dir = tree.dir,
            file = tree.file,
            sub = tree.sub,
            link = tree.link,
            unix = cfg!(unix),
        ))
        .await?)
}

#[tokio::test(flavor = "multi_thread")]
async fn den_fs_metadata_reports_a_regular_file() -> eyre::Result<()> {
    let tree = Tree::new();
    let failures: String = eval(
        &tree,
        r#"
          const { assertEquals } = await import("den:assert");
          const meta = await fs.metadata(FILE);
          assertEquals(meta.len, 5);
          assertEquals(meta.isFile, true);
          assertEquals(meta.isDir, false);
          assertEquals(meta.isSymlink, false);
          ""
        "#,
    )
    .await?;
    assert_eq!(failures, "");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn metadata_read_dir_read_link_and_set_permissions() -> eyre::Result<()> {
    let tree = Tree::new();
    let failures: String = eval(
        &tree,
        r#"
          const meta = await fs.metadata(FILE);
          const dirMeta = await fs.metadata(SUB);
          const entries = await fs.readDir(DIR);
          const byName = Object.fromEntries(entries.map((entry) => [entry.name, entry]));
          const checks = {
            fileLen: meta.len === 5,
            isFile: meta.isFile === true,
            fileNotDir: meta.isDir === false,
            fileNotLink: meta.isSymlink === false,
            isDir: dirMeta.isDir === true,
            dirNotFile: dirMeta.isFile === false,
            readDirFile: byName["hello.txt"]?.isFile === true,
            readDirDir: byName["sub"]?.isDir === true,
          };
          if (UNIX) {
            const linked = await fs.readLink(LINK);
            const linkMeta = await fs.symlinkMetadata(LINK);
            const followed = await fs.metadata(LINK);
            await fs.setPermissions(FILE, 0o600);
            const chmodded = await fs.metadata(FILE);
            checks.readLink = linked === "hello.txt";
            checks.symlinkIsLink = linkMeta.isSymlink === true;
            checks.symlinkNotFile = linkMeta.isFile === false;
            checks.metadataFollows = followed.isFile === true;
            checks.metadataFollowsNotLink = followed.isSymlink === false;
            checks.mode = (chmodded.mode & 0o777) === 0o600;
            checks.readDirLink = byName["hello.link"]?.isSymlink === true;
          }
          Object.entries(checks)
            .filter(([, held]) => !held)
            .map(([name]) => name)
            .join(",")
        "#,
    )
    .await?;
    assert_eq!(failures, "");
    Ok(())
}
