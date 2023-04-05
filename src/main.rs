use rquickjs::{Coerced, Context, Func, Runtime, Tokio};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::default::Default;
use std::io::stderr;
use swc_core::base::config::IsModule;
use swc_core::base::Compiler;
use swc_core::common::errors::Handler;
use swc_core::common::sync::Lrc;
use swc_core::common::Globals;
use swc_core::common::SourceMap;
use swc_core::common::GLOBALS;
use swc_core::common::{FileName, Mark};
use swc_core::ecma::ast::EsVersion;
use swc_core::ecma::codegen::text_writer::JsWriter;
use swc_core::ecma::codegen::Emitter;
use swc_core::ecma::parser::Syntax;
use swc_core::ecma::parser::TsConfig;
use swc_core::ecma::transforms::base::fixer::fixer;
use swc_core::ecma::transforms::base::hygiene::hygiene;
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::transforms::typescript::strip;
use swc_core::ecma::visit::FoldWith;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

fn print(msg: String) {
    println!("{msg}");
}

#[tokio::main]
async fn main() {
    let (repl_tx, mut repl_rx) = mpsc::unbounded_channel();
    let (parse_tx, mut parse_rx) = mpsc::unbounded_channel();

    let mut set = JoinSet::new();

    let repl = async move {
        let mut interrupted = false;
        let mut rl = DefaultEditor::new().unwrap();

        'repl: loop {
            match rl.readline("> ") {
                Err(ReadlineError::Eof) => break,
                Err(ReadlineError::Interrupted) if interrupted => break,
                Err(ReadlineError::Interrupted) => {
                    println!("(To exit, press Ctrl+C again or Ctrl+D)");
                    interrupted = true;
                }
                Err(_) => {}
                Ok(line) => {
                    interrupted = false;

                    if line.is_empty() {
                        continue 'repl;
                    }

                    let _ = repl_tx.send(line.clone());
                    let _ = rl.add_history_entry(&line);
                }
            }
        }
    };

    let parse = async move {
        let cm: Lrc<SourceMap> = Default::default();
        let compiler = Compiler::new(cm.clone());
        let handler = Handler::with_emitter_writer(Box::new(stderr()), Some(compiler.cm.clone()));
        let syntax = Syntax::Typescript(TsConfig {
            ..Default::default()
        });
        let globals = Globals::new();

        while let Some(line) = repl_rx.recv().await {
            let fm = cm.new_source_file(FileName::Anon, line.clone());

            let res = GLOBALS.set(&globals, || {
                if let Ok(program) = compiler.parse_js(
                    fm,
                    &handler,
                    EsVersion::Es2020,
                    syntax,
                    IsModule::Bool(false),
                    None,
                ) {
                    let unresolved_mark = Mark::new();
                    let top_level_mark = Mark::new();

                    let program = program
                        .fold_with(&mut resolver(unresolved_mark, top_level_mark, true))
                        .fold_with(&mut strip(top_level_mark))
                        .fold_with(&mut hygiene())
                        .fold_with(&mut fixer(None));

                    let mut buf = vec![];

                    let mut emitter = Emitter {
                        cfg: swc_core::ecma::codegen::Config {
                            minify: false,
                            ..Default::default()
                        },
                        cm: cm.clone(),
                        comments: None,
                        wr: JsWriter::new(cm.clone(), "\n", &mut buf, None),
                    };

                    if let Ok(_) = emitter.emit_program(&program) {
                        Some(buf)
                    } else {
                        None
                    }
                } else {
                    None
                }
            });

            if let Some(x) = res {
                let _ = parse_tx.send(x);
            }
        }
    };

    let interpreter = async move {
        let rt = Runtime::new().unwrap();
        let _ = rt.spawn_executor(Tokio);
        let ctx = Context::full(&rt).unwrap();
        ctx.enable_big_num_ext(true);
        ctx.with(|ctx| {
            let global = ctx.globals();
            global.set("print", Func::new("print", print)).unwrap();
        });

        while let Some(source) = parse_rx.recv().await {
            ctx.with(|ctx| {
                let result: rquickjs::Result<Coerced<String>> = ctx.eval(source);
                match result {
                    Ok(res) => {
                        println!("{}", res.0)
                    }
                    Err(err) => {
                        eprintln!("{}", err)
                    }
                }
            });
        }

        rt.idle().await;
    };

    set.spawn(interpreter);
    set.spawn(parse);
    set.spawn(repl);

    while let Some(_) = set.join_next().await {}
}
