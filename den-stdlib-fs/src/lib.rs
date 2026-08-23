use rquickjs::{Ctx, IntoJs, Object, Result, Value};

/// `std::fs::Metadata` flattened into the handful of fields a script actually
/// reads. `mode` is Unix-only: Windows has no useful equivalent beyond the
/// readonly bit, which `setPermissions` maps separately.
pub struct Stat {
    len:        u64,
    is_file:    bool,
    is_dir:     bool,
    is_symlink: bool,
    mode:       Option<u32>,
}

impl Stat {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            len:        metadata.len(),
            is_file:    metadata.is_file(),
            is_dir:     metadata.is_dir(),
            is_symlink: metadata.is_symlink(),
            mode:       Self::unix_mode(metadata),
        }
    }

    fn unix_mode(metadata: &std::fs::Metadata) -> Option<u32> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            Some(metadata.permissions().mode())
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            None
        }
    }
}

impl<'js> IntoJs<'js> for Stat {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let object = Object::new(ctx.clone())?;
        object.set("len", self.len)?;
        object.set("isFile", self.is_file)?;
        object.set("isDir", self.is_dir)?;
        object.set("isSymlink", self.is_symlink)?;
        if let Some(mode) = self.mode {
            object.set("mode", mode)?;
        }
        object.into_js(ctx)
    }
}

pub struct DirEntry {
    name:       String,
    is_file:    bool,
    is_dir:     bool,
    is_symlink: bool,
}

impl DirEntry {
    async fn from_tokio(entry: tokio::fs::DirEntry) -> Result<Self> {
        let file_type = entry.file_type().await?;
        Ok(Self {
            name:       entry.file_name().to_string_lossy().into_owned(),
            is_file:    file_type.is_file(),
            is_dir:     file_type.is_dir(),
            is_symlink: file_type.is_symlink(),
        })
    }
}

impl<'js> IntoJs<'js> for DirEntry {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let object = Object::new(ctx.clone())?;
        object.set("name", self.name)?;
        object.set("isFile", self.is_file)?;
        object.set("isDir", self.is_dir)?;
        object.set("isSymlink", self.is_symlink)?;
        object.into_js(ctx)
    }
}

struct Permissions;

impl Permissions {
    async fn set(path: String, mode: u32) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::Permissions::from_mode(mode);
            tokio::fs::set_permissions(path, permissions).await?;
        }
        #[cfg(windows)]
        {
            // Windows has no Unix mode bits. Mapping the write bits onto
            // `readonly` is the closest equivalent; missing write bits
            // (0o222) become readonly.
            let mut permissions = tokio::fs::metadata(&path).await?.permissions();
            permissions.set_readonly(mode & 0o222 == 0);
            tokio::fs::set_permissions(path, permissions).await?;
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (path, mode);
        }
        Ok(())
    }
}

#[rquickjs::module(
    rename = "den:fs",
    rename_vars = "camelCase",
    rename_types = "camelCase"
)]
pub mod fs {
    use rquickjs::{Result, module::Declarations};

    #[qjs(declare)]
    pub fn declare(declare: &Declarations) -> rquickjs::Result<()> {
        declare.declare("hardLink")?;
        declare.declare("canonicalize")?;
        declare.declare("rename")?;
        declare.declare("createDir")?;
        declare.declare("removeDirAll")?;
        declare.declare("symlinkMetadata")?;
        declare.declare("metadata")?;
        declare.declare("createDirAll")?;
        declare.declare("setPermissions")?;
        declare.declare("copy")?;
        declare.declare("readDir")?;
        declare.declare("readLink")?;
        declare.declare("readToString")?;
        declare.declare("removeFile")?;
        declare.declare("read")?;
        declare.declare("write")?;
        declare.declare("removeDir")?;
        Ok(())
    }

    #[rquickjs::function(rename = "canonicalize")]
    pub async fn canonicalize(path: String) -> Result<Option<String>> {
        Ok(tokio::fs::canonicalize(path)
            .await?
            .to_str()
            .map(|x| x.to_string()))
    }
    #[rquickjs::function(rename = "copy")]
    pub async fn copy(from: String, to: String) -> Result<()> {
        tokio::fs::copy(from, to).await?;
        Ok(())
    }
    #[rquickjs::function(rename = "createDir")]
    pub async fn create_dir(path: String) -> Result<()> {
        tokio::fs::create_dir(path).await?;
        Ok(())
    }
    #[rquickjs::function(rename = "createDirAll")]
    pub async fn create_dir_all(path: String) -> Result<()> {
        tokio::fs::create_dir_all(path).await?;
        Ok(())
    }
    #[rquickjs::function(rename = "hardLink")]
    pub async fn hard_link(src: String, dst: String) -> Result<()> {
        tokio::fs::hard_link(src, dst).await?;
        Ok(())
    }
    #[rquickjs::function(rename = "metadata")]
    pub async fn metadata(path: String) -> Result<super::Stat> {
        Ok(super::Stat::from_metadata(
            &tokio::fs::metadata(path).await?,
        ))
    }
    #[rquickjs::function(rename = "read")]
    pub async fn read(path: String) -> Result<Vec<u8>> { Ok(tokio::fs::read(path).await?) }
    #[rquickjs::function(rename = "readDir")]
    pub async fn read_dir(path: String) -> Result<Vec<super::DirEntry>> {
        let mut entries = tokio::fs::read_dir(path).await?;
        let mut out = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            out.push(super::DirEntry::from_tokio(entry).await?);
        }
        Ok(out)
    }

    #[rquickjs::function(rename = "readLink")]
    pub async fn read_link(path: String) -> Result<String> {
        Ok(tokio::fs::read_link(path)
            .await?
            .to_string_lossy()
            .into_owned())
    }

    #[rquickjs::function(rename = "readToString")]
    pub async fn read_to_string(path: String) -> Result<String> {
        Ok(tokio::fs::read_to_string(path).await?)
    }

    #[rquickjs::function(rename = "removeDir")]
    #[qjs(rename = "removeDir")]
    pub async fn remove_dir(path: String) -> Result<()> {
        tokio::fs::remove_dir(path).await?;
        Ok(())
    }

    #[rquickjs::function(rename = "removeDirAll")]
    pub async fn remove_dir_all(path: String) -> Result<()> {
        tokio::fs::remove_dir_all(path).await?;
        Ok(())
    }

    #[rquickjs::function(rename = "removeFile")]
    pub async fn remove_file(path: String) -> Result<()> {
        tokio::fs::remove_file(path).await?;
        Ok(())
    }

    #[rquickjs::function(rename = "rename")]
    pub async fn rename(from: String, to: String) -> Result<()> {
        tokio::fs::rename(from, to).await?;
        Ok(())
    }

    #[rquickjs::function(rename = "setPermissions")]
    pub async fn set_permissions(path: String, mode: u32) -> Result<()> {
        super::Permissions::set(path, mode).await
    }

    #[rquickjs::function(rename = "symlinkMetadata")]
    pub async fn symlink_metadata(path: String) -> Result<super::Stat> {
        Ok(super::Stat::from_metadata(
            &tokio::fs::symlink_metadata(path).await?,
        ))
    }

    #[rquickjs::function(rename = "write")]
    pub async fn write(path: String, contents: Vec<u8>) -> Result<()> {
        tokio::fs::write(path, contents).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rquickjs::{
        AsyncContext, AsyncRuntime, CatchResultExt, FromJs, Module, Object, Promise,
        context::EvalOptions,
    };

    /// A temp tree with a file, a directory, and (on Unix) a symlink to the
    /// file. Paths are published as globals so the snippet can call `den:fs`
    /// the way a script would.
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

    async fn eval<T>(tree: &Tree, source: &str) -> Result<T, String>
    where
        T: for<'js> FromJs<'js> + Send + Sync + 'static,
    {
        let runtime = AsyncRuntime::new().expect("runtime");
        let context = AsyncContext::full(&runtime).await.expect("context");
        let dir = tree.dir.clone();
        let file = tree.file.clone();
        let sub = tree.sub.clone();
        let link = tree.link.clone();
        context
            .async_with(async move |ctx| {
                let run = async {
                    let (module, evaluated) =
                        Module::evaluate_def::<crate::js_fs, _>(ctx.clone(), "den:fs")?;
                    evaluated.into_future::<()>().await?;
                    ctx.globals().set("fs", module.namespace()?)?;
                    ctx.globals().set("DIR", dir)?;
                    ctx.globals().set("FILE", file)?;
                    ctx.globals().set("SUB", sub)?;
                    ctx.globals().set("LINK", link)?;
                    ctx.globals().set("UNIX", cfg!(unix))?;
                    let mut options = EvalOptions::default();
                    options.global = true;
                    options.promise = true;
                    options.strict = true;
                    ctx.eval_with_options::<Promise, _>(source, options)?
                        .into_future::<Object>()
                        .await?
                        .get::<_, T>("value")
                };
                run.await.catch(&ctx).map_err(|error| error.to_string())
            })
            .await
    }

    #[tokio::test]
    async fn metadata_read_dir_read_link_and_set_permissions() {
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
        .await
        .expect("the fs snippet evaluates");
        assert_eq!(failures, "");
    }
}
