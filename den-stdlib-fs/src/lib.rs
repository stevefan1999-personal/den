#[rquickjs::module(
    rename = "den:fs",
    rename_vars = "camelCase",
    rename_types = "camelCase"
)]
pub mod fs {
    use rquickjs::{module::Declarations, Ctx, Exception, Result};

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
    pub async fn metadata(ctx: Ctx<'_>) -> Result<()> {
        Err(Exception::throw_internal(&ctx, "not implemented"))
    }
    #[rquickjs::function(rename = "read")]
    pub async fn read(path: String) -> Result<Vec<u8>> {
        Ok(tokio::fs::read(path).await?)
    }
    #[rquickjs::function(rename = "readDir")]
    pub async fn read_dir(ctx: Ctx<'_>) -> Result<()> {
        Err(Exception::throw_internal(&ctx, "not implemented"))
    }

    #[rquickjs::function(rename = "readLink")]
    pub async fn read_link(ctx: Ctx<'_>) -> Result<()> {
        Err(Exception::throw_internal(&ctx, "not implemented"))
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
    pub async fn set_permissions(ctx: Ctx<'_>) -> Result<()> {
        Err(Exception::throw_internal(&ctx, "not implemented"))
    }

    #[rquickjs::function(rename = "symlinkMetadata")]
    pub async fn symlink_metadata(ctx: Ctx<'_>) -> Result<()> {
        Err(Exception::throw_internal(&ctx, "not implemented"))
    }

    #[rquickjs::function(rename = "write")]
    pub async fn write(path: String, contents: Vec<u8>) -> Result<()> {
        tokio::fs::write(path, contents).await?;
        Ok(())
    }
}
