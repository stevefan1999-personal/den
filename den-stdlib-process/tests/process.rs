use std::path::PathBuf;

use color_eyre::eyre;
use den_core::engine::Engine;
use rquickjs::FromJs;

/// `chdir` mutates process-global state; tests that touch it take this lock.
static CWD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn eval<T>(source: &str) -> eyre::Result<T>
where
    T: for<'js> FromJs<'js> + Send + Sync + 'static,
{
    Ok(Engine::new().await.eval(source).await?)
}

async fn run(source: &str) -> eyre::Result<()> {
    let _: String = eval(&format!("{source}\n\"ok\"")).await?;
    Ok(())
}

fn echo_argv() -> Vec<String> {
    #[cfg(windows)]
    {
        vec![
            "cmd".into(),
            "/c".into(),
            "echo".into(),
            "hello-from-den".into(),
        ]
    }
    #[cfg(not(windows))]
    {
        let program = ["/bin/echo", "/usr/bin/echo"]
            .into_iter()
            .find(|path| PathBuf::from(path).exists())
            .unwrap_or("echo");
        vec![program.into(), "hello-from-den".into()]
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn process_global_exposes_pid_argv_and_env() -> eyre::Result<()> {
    run(include_str!("js/process.js")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn argv_is_a_non_empty_array_of_strings() -> eyre::Result<()> {
    let failures: String = eval(
        r#"
          Object.entries({
            isArray: Array.isArray(process.argv),
            nonEmpty: process.argv.length > 0,
            allStrings: process.argv.every((arg) => typeof arg === "string"),
          }).filter(([, held]) => !held).map(([name]) => name).join(",")
        "#,
    )
    .await?;
    assert_eq!(failures, "");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn env_path_or_home_is_a_string() -> eyre::Result<()> {
    let kind: String = eval(
        r#"
          const value = process.env.PATH ?? process.env.HOME;
          typeof value
        "#,
    )
    .await?;
    assert_eq!(kind, "string");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn env_get_set_delete_round_trips() -> eyre::Result<()> {
    let key = format!("DEN_PROCESS_TEST_{}", std::process::id());
    let report: String = eval(&format!(
        r#"
          const key = {key:?};
          process.env[key] = 123;
          const afterSet = process.env[key];
          const has = key in process.env;
          delete process.env[key];
          const afterDelete = process.env[key];
          [afterSet, has, afterDelete === undefined].join(",")
        "#
    ))
    .await?;
    assert_eq!(report, "123,true,true");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cwd_round_trips_with_chdir_in_a_temp_dir() -> eyre::Result<()> {
    let _guard = CWD_LOCK.lock().await;
    let original = std::env::current_dir().expect("cwd");
    let dir = std::env::temp_dir().join(format!("den-process-cwd-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let dir = dir.canonicalize()?;
    let dir_js = dir.to_string_lossy().replace('\\', "/");

    let outcome = eval::<String>(&format!(
        r#"
          const original = process.cwd();
          process.chdir({dir_js:?});
          const now = process.cwd();
          process.chdir(original);
          const restored = process.cwd();
          now + "\n" + restored
        "#
    ))
    .await;

    let _ = std::env::set_current_dir(&original);
    let payload = outcome?;
    let (now, restored) = payload.split_once('\n').unwrap_or((payload.as_str(), ""));
    let now_path = PathBuf::from(now)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(now));
    assert_eq!(now_path, dir, "chdir should land in the temp dir");
    let restored_path = PathBuf::from(restored)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(restored));
    assert_eq!(
        restored_path,
        original.canonicalize().unwrap_or(original),
        "chdir should restore the original cwd"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn lookup_localhost_returns_loopback() -> eyre::Result<()> {
    let ok: bool = eval(
        r#"
          const addr = await process.lookup("localhost");
          addr.ip === "127.0.0.1" || addr.ip === "::1"
        "#,
    )
    .await?;
    assert!(ok, "localhost should resolve to a loopback address");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn lookup_all_returns_an_array() -> eyre::Result<()> {
    let ok: bool = eval(
        r#"
          const addrs = await process.lookup("localhost", { all: true });
          Array.isArray(addrs) && addrs.length > 0 && addrs.every((a) => a.ip && (a.family === 4 || a.family === 6))
        "#,
    )
    .await?;
    assert!(ok);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn spawn_echo_exits_zero_and_reads_stdout() -> eyre::Result<()> {
    let argv = echo_argv();
    let argv_js = format!(
        "[{}]",
        argv.iter()
            .map(|arg| format!("{arg:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let report: String = eval(&format!(
        r#"
          const child = process.spawn({argv_js}, {{ stdout: "pipe", stderr: "ignore" }});
          const out = await child.stdout.text();
          const status = await child.wait();
          [child.pid > 0, status.code, out.trim()].join("|")
        "#
    ))
    .await?;
    let parts: Vec<&str> = report.split('|').collect();
    assert_eq!(
        parts.first().copied(),
        Some("true"),
        "pid should be positive: {report}"
    );
    assert_eq!(
        parts.get(1).copied(),
        Some("0"),
        "echo should exit 0: {report}"
    );
    assert_eq!(
        parts.get(2).copied(),
        Some("hello-from-den"),
        "stdout should contain the echo text: {report}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn pid_is_a_positive_number() -> eyre::Result<()> {
    let report: String = eval(
        r#"
          Object.entries({
            pidPositive: typeof process.pid === "number" && process.pid > 0,
            ppidPositive: typeof process.ppid === "number" && process.ppid > 0,
            exitIsFunction: typeof process.exit === "function",
          }).filter(([, held]) => !held).map(([name]) => name).join(",")
        "#,
    )
    .await?;
    assert_eq!(report, "");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn add_and_remove_signal_listener_do_not_throw() -> eyre::Result<()> {
    let ok: bool = eval(
        r#"
          const listener = () => {};
          process.addSignalListener("SIGTERM", listener);
          process.removeSignalListener("SIGTERM", listener);
          true
        "#,
    )
    .await?;
    assert!(ok);
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn kill_terminates_a_spawned_sleep() -> eyre::Result<()> {
    let sleep = ["/bin/sleep", "/usr/bin/sleep"]
        .into_iter()
        .find(|path| PathBuf::from(path).exists())
        .unwrap_or("sleep");
    let report: String = eval(&format!(
        r#"
          const child = process.spawn([{sleep:?}, "30"], {{ stdout: "ignore", stderr: "ignore" }});
          process.kill(child.pid, "SIGKILL");
          const status = await child.wait();
          [child.pid > 0, status.code === null].join("|")
        "#
    ))
    .await?;
    assert_eq!(report, "true|true", "SIGKILL should leave a null exit code");
    Ok(())
}
