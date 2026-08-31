use std::{io::Write as _, path::Path};

use rquickjs::{Ctx, FromJs, IntoJs, Object, Result, Value};
use tempfile::NamedTempFile;

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
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt as _;
            Some(metadata.permissions().mode())
        };
        #[cfg(not(unix))]
        let mode = None;
        Self {
            len: metadata.len(),
            is_file: metadata.is_file(),
            is_dir: metadata.is_dir(),
            is_symlink: metadata.is_symlink(),
            mode,
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

/// Options bag for `write`. Absent means the historical truncate-then-write
/// path; only `atomic` changes it.
#[derive(Default)]
pub struct WriteOptions {
    atomic: bool,
}

impl WriteOptions {
    /// Whole-file write. The default path truncates the target and then streams
    /// the bytes in, so a process that dies mid-write leaves a prefix behind.
    /// `atomic` writes to a sibling temporary file and renames it onto the
    /// target instead, and `rename(2)` on one filesystem is indivisible: a
    /// reader sees either the whole old file or the whole new one. No fsync —
    /// that would buy power-loss durability, which is a different question.
    async fn write(self, path: String, contents: Vec<u8>) -> Result<()> {
        if !self.atomic {
            tokio::fs::write(path, contents).await?;
            return Ok(());
        }
        // tempfile is sync, and this is the same blocking shape tokio::fs::write
        // already uses underneath.
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            let target = Path::new(&path);
            let parent = target
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            let mut temporary = NamedTempFile::new_in(parent)?;
            temporary.write_all(&contents)?;
            temporary.persist(target).map_err(|error| error.error)?;
            Ok(())
        })
        .await
        .map_err(std::io::Error::other)??;
        Ok(())
    }
}

impl<'js> FromJs<'js> for WriteOptions {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        Ok(Self {
            atomic: Object::from_js(ctx, value)?
                .get::<_, Option<bool>>("atomic")?
                .unwrap_or_default(),
        })
    }
}

#[rquickjs::module(
    rename = "den:fs",
    rename_vars = "camelCase",
    rename_types = "camelCase"
)]
pub mod fs {
    use rquickjs::{Result, function::Opt, module::Declarations};

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
            .map(std::string::ToString::to_string))
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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).await?;
        }
        #[cfg(windows)]
        {
            let mut permissions = tokio::fs::metadata(&path).await?.permissions();
            permissions.set_readonly(mode & 0o222 == 0);
            tokio::fs::set_permissions(path, permissions).await?;
        }
        #[cfg(not(any(unix, windows)))]
        let _ = (path, mode);
        Ok(())
    }

    #[rquickjs::function(rename = "symlinkMetadata")]
    pub async fn symlink_metadata(path: String) -> Result<super::Stat> {
        Ok(super::Stat::from_metadata(
            &tokio::fs::symlink_metadata(path).await?,
        ))
    }

    #[rquickjs::function(rename = "write")]
    pub async fn write(
        path: String, contents: Vec<u8>, options: Opt<super::WriteOptions>,
    ) -> Result<()> {
        options.0.unwrap_or_default().write(path, contents).await
    }
}
