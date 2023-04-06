use bytes::Bytes;
use color_eyre::eyre::eyre;
use std::io::stderr;
use std::sync::Arc as Lrc;
use swc_core::base::config::IsModule;
use swc_core::base::Compiler;
use swc_core::common::errors::Handler;
use swc_core::common::{FileName, Globals, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::EsVersion;
use swc_core::ecma::codegen::text_writer::JsWriter;
use swc_core::ecma::codegen::Emitter;
use swc_core::ecma::parser::Syntax;
use swc_core::ecma::transforms::base::fixer::fixer;
use swc_core::ecma::transforms::base::hygiene::hygiene;
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::transforms::typescript::strip;
use swc_core::ecma::visit::FoldWith;
pub struct EasySwcTranspiler {
    source_map: Lrc<SourceMap>,
    compiler: Compiler,
    handler: Handler,
    globals: Globals,
}

impl Default for EasySwcTranspiler {
    fn default() -> Self {
        let source_map: Lrc<SourceMap> = Default::default();
        let compiler = Compiler::new(source_map.clone());
        let handler = Handler::with_emitter_writer(Box::new(stderr()), Some(compiler.cm.clone()));
        let globals = Globals::new();

        Self {
            source_map,
            compiler,
            handler,
            globals,
        }
    }
}

impl EasySwcTranspiler {
    pub fn transpile(
        &mut self,
        source: String,
        syntax: Syntax,
        is_module: IsModule,
    ) -> color_eyre::Result<Bytes> {
        let fm = self
            .source_map
            .new_source_file(FileName::Anon, source.clone());

        GLOBALS.set(&self.globals, || {
            let mut program = self
                .compiler
                .parse_js(
                    fm,
                    &self.handler,
                    EsVersion::Es2022,
                    syntax,
                    is_module,
                    None,
                )
                .map_err(|err| eyre!(err))?;

            if let Syntax::Typescript(_) = syntax {
                let unresolved_mark = Mark::new();
                let top_level_mark = Mark::new();

                program = program
                    .fold_with(&mut resolver(unresolved_mark, top_level_mark, true))
                    .fold_with(&mut strip(top_level_mark));
            }

            program = program
                .fold_with(&mut hygiene())
                .fold_with(&mut fixer(None));

            let mut buf = vec![];

            let mut emitter = Emitter {
                cfg: swc_core::ecma::codegen::Config {
                    target: EsVersion::Es2020,
                    ..Default::default()
                },
                cm: self.source_map.clone(),
                comments: None,
                wr: JsWriter::new(self.source_map.clone(), "\n", &mut buf, None),
            };

            emitter.emit_program(&program)?;
            Ok(buf.into())
        })
    }
}
