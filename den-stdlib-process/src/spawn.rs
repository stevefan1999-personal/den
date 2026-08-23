//! `process.spawn` — `tokio::process::Command` plus a JS-facing child handle.

use std::sync::Arc;

use either::Either;
use rquickjs::{
    Ctx, Exception, IntoJs, JsLifetime, Object, Result, Value,
    class::{Class, Trace},
    function::Opt,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::Mutex,
};

use crate::signal::Signal;

/// How a child's stdio stream is wired.
pub enum StdioKind {
    Pipe,
    Ignore,
    Inherit,
}

impl StdioKind {
    pub fn parse(name: &str, ctx: &Ctx<'_>) -> Result<Self> {
        match name {
            "pipe" => Ok(Self::Pipe),
            "ignore" => Ok(Self::Ignore),
            "inherit" => Ok(Self::Inherit),
            other => {
                Err(Exception::throw_type(
                    ctx,
                    &format!("invalid stdio '{other}'"),
                ))
            }
        }
    }

    pub fn to_stdio(&self) -> std::process::Stdio {
        match self {
            Self::Pipe => std::process::Stdio::piped(),
            Self::Ignore => std::process::Stdio::null(),
            Self::Inherit => std::process::Stdio::inherit(),
        }
    }
}

/// Options accepted by `process.spawn`.
pub struct SpawnOptions {
    cwd:    Option<String>,
    env:    Option<Vec<(String, String)>>,
    stdin:  StdioKind,
    stdout: StdioKind,
    stderr: StdioKind,
}

impl SpawnOptions {
    pub fn from_js<'js>(options: Option<Object<'js>>, ctx: &Ctx<'js>) -> Result<Self> {
        let mut parsed = Self {
            cwd:    None,
            env:    None,
            // stdin is not returned, so a pipe would only ever be closed; ignore
            // is the same outcome without a dangling writer.
            stdin:  StdioKind::Ignore,
            stdout: StdioKind::Pipe,
            stderr: StdioKind::Pipe,
        };
        let Some(options) = options else {
            return Ok(parsed);
        };
        if let Ok(Some(cwd)) = options.get::<_, Option<String>>("cwd") {
            parsed.cwd = Some(cwd);
        }
        if let Ok(Some(env)) = options.get::<_, Option<Object>>("env") {
            parsed.env = Some(Self::read_env(env)?);
        }
        if let Ok(Some(stdin)) = options.get::<_, Option<String>>("stdin") {
            parsed.stdin = StdioKind::parse(&stdin, ctx)?;
        }
        if let Ok(Some(stdout)) = options.get::<_, Option<String>>("stdout") {
            parsed.stdout = StdioKind::parse(&stdout, ctx)?;
        }
        if let Ok(Some(stderr)) = options.get::<_, Option<String>>("stderr") {
            parsed.stderr = StdioKind::parse(&stderr, ctx)?;
        }
        Ok(parsed)
    }

    fn read_env(env: Object<'_>) -> Result<Vec<(String, String)>> {
        let mut pairs = Vec::new();
        for entry in env.props::<String, rquickjs::Coerced<String>>() {
            let (name, rquickjs::Coerced(value)) = entry?;
            pairs.push((name, value));
        }
        Ok(pairs)
    }

    pub fn apply(&self, command: &mut Command) {
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        if let Some(env) = &self.env {
            command.env_clear();
            command.envs(env.iter().cloned());
        }
        command.stdin(self.stdin.to_stdio());
        command.stdout(self.stdout.to_stdio());
        command.stderr(self.stderr.to_stdio());
        command.kill_on_drop(true);
    }
}

enum ChildSlot {
    Running(tokio::process::Child),
    Exited(Option<i32>),
}

/// A spawned child: `pid`, `wait()`, `kill()`, and piped stdio readers.
#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename = "ChildProcess")]
pub struct Child {
    #[qjs(get, enumerable)]
    pid:  u32,
    #[qjs(skip_trace)]
    slot: Arc<Mutex<ChildSlot>>,
}

/// Async text reader wrapping a piped stdout or stderr.
#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "PipeReader")]
pub struct PipeReader {
    #[qjs(skip_trace)]
    inner: Arc<Mutex<Option<Box<dyn AsyncRead + Unpin + Send>>>>,
}

impl PipeReader {
    pub fn new(stream: Box<dyn AsyncRead + Unpin + Send>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Some(stream))),
        }
    }
}

#[rquickjs::methods]
impl PipeReader {
    pub async fn text(&self, ctx: Ctx<'_>) -> Result<String> {
        let mut slot = self.inner.lock().await;
        let Some(mut stream) = slot.take() else {
            return Err(Exception::throw_internal(&ctx, "stdio already consumed"));
        };
        drop(slot);
        let mut text = String::new();
        stream
            .read_to_string(&mut text)
            .await
            .map_err(|error| Exception::throw_internal(&ctx, &error.to_string()))?;
        Ok(text)
    }
}

impl Child {
    pub fn spawn<'js>(
        ctx: Ctx<'js>, cmd: Either<String, Vec<String>>, options: Option<Object<'js>>,
    ) -> Result<Class<'js, Self>> {
        let mut command = match cmd {
            Either::Left(program) => {
                if program.is_empty() {
                    return Err(Exception::throw_type(&ctx, "command must be non-empty"));
                }
                Command::new(program)
            }
            Either::Right(argv) => {
                let mut argv = argv.into_iter();
                let Some(program) = argv.next() else {
                    return Err(Exception::throw_type(&ctx, "command must be non-empty"));
                };
                let mut command = Command::new(program);
                command.args(argv);
                command
            }
        };

        let options = SpawnOptions::from_js(options, &ctx)?;
        let stdout_piped = matches!(options.stdout, StdioKind::Pipe);
        let stderr_piped = matches!(options.stderr, StdioKind::Pipe);
        options.apply(&mut command);

        let mut child = command
            .spawn()
            .map_err(|error| Exception::throw_internal(&ctx, &error.to_string()))?;
        let pid = child
            .id()
            .ok_or_else(|| Exception::throw_internal(&ctx, "failed to read child pid"))?;

        // Piped stdin is not exposed; close it so the child sees EOF instead of
        // hanging on a writer we never hand to script.
        drop(child.stdin.take());

        let stdout = if stdout_piped {
            child
                .stdout
                .take()
                .map(|pipe| PipeReader::new(Box::new(pipe)))
        } else {
            None
        };
        let stderr = if stderr_piped {
            child
                .stderr
                .take()
                .map(|pipe| PipeReader::new(Box::new(pipe)))
        } else {
            None
        };

        let instance = Class::instance(ctx.clone(), Self {
            pid,
            slot: Arc::new(Mutex::new(ChildSlot::Running(child))),
        })?;
        instance.set("stdout", Self::optional_reader(&ctx, stdout)?)?;
        instance.set("stderr", Self::optional_reader(&ctx, stderr)?)?;
        Ok(instance)
    }

    fn optional_reader<'js>(ctx: &Ctx<'js>, reader: Option<PipeReader>) -> Result<Value<'js>> {
        match reader {
            Some(reader) => Class::instance(ctx.clone(), reader)?.into_js(ctx),
            None => Ok(Value::new_null(ctx.clone())),
        }
    }
}

/// `{ code }` from `child.wait()`. `code` is `null` when the child was
/// signaled.
#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename = "ExitStatus")]
pub struct ExitStatus {
    #[qjs(skip_trace)]
    code: Option<i32>,
}

#[rquickjs::methods]
impl ExitStatus {
    #[qjs(get, enumerable)]
    pub fn code<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        match self.code {
            Some(code) => code.into_js(&ctx),
            None => Ok(Value::new_null(ctx)),
        }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Child {
    pub async fn wait(&self, ctx: Ctx<'_>) -> Result<ExitStatus> {
        let mut slot = self.slot.lock().await;
        let code = match &mut *slot {
            ChildSlot::Exited(code) => *code,
            ChildSlot::Running(child) => {
                let status = child
                    .wait()
                    .await
                    .map_err(|error| Exception::throw_internal(&ctx, &error.to_string()))?;
                let code = status.code();
                *slot = ChildSlot::Exited(code);
                code
            }
        };
        Ok(ExitStatus { code })
    }

    pub fn kill(&self, Opt(sig): Opt<String>, ctx: Ctx<'_>) -> Result<()> {
        Signal::send(self.pid as i32, sig.as_deref(), &ctx)
    }
}
