use std::{io::stderr, sync::Arc};

use bytes::Bytes;
use color_eyre::eyre::eyre;
use swc_core::{
    base::{config::IsModule, Compiler},
    common::{
        errors::Handler, sync::Lrc, BytePos, FileName, Globals, LineCol, Mark, SourceFile,
        SourceMap, GLOBALS,
    },
    ecma::{
        ast::EsVersion,
        codegen::{text_writer::JsWriter, Emitter},
        parser::Syntax,
        transforms::{
            base::{fixer::fixer, hygiene::hygiene, resolver},
            typescript::strip,
        },
        visit::FoldWith,
    },
};

pub struct EasySwcTranspiler {
    source_map: Lrc<SourceMap>,
    compiler:   Compiler,
    handler:    Handler,
    globals:    Globals,
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
        &self,
        source: String,
        syntax: Syntax,
        is_module: IsModule,
        emit_sourcemap: bool,
    ) -> color_eyre::Result<(Bytes, Option<swc_core::base::sourcemap::SourceMap>)> {
        let fm = self
            .source_map
            .new_source_file_from(FileName::Anon, source.into());

        GLOBALS.set(&self.globals, || {
            self.do_transpile(syntax, is_module, emit_sourcemap, fm)
        })
    }

    fn do_transpile(
        &self,
        syntax: Syntax,
        is_module: IsModule,
        emit_sourcemap: bool,
        fm: Arc<SourceFile>,
    ) -> color_eyre::Result<(Bytes, Option<swc_core::base::sourcemap::SourceMap>)> {
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

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();

        program = match syntax {
            Syntax::Typescript(_) => {
                program
                    .fold_with(&mut resolver(unresolved_mark, top_level_mark, true))
                    .fold_with(&mut strip(top_level_mark))
            }
            Syntax::Es(_) => {
                program.fold_with(&mut resolver(unresolved_mark, top_level_mark, false))
            }
        };

        program = program
            .fold_with(&mut hygiene())
            .fold_with(&mut fixer(None));

        let mut buf = vec![];
        let mut srcmap: Vec<(BytePos, LineCol)> = vec![];

        let wr = JsWriter::new(
            self.source_map.clone(),
            "\n",
            &mut buf,
            if emit_sourcemap {
                Some(&mut srcmap)
            } else {
                None
            },
        );
        let cfg = swc_core::ecma::codegen::Config {
            target: EsVersion::Es2020,
            ..Default::default()
        };
        let mut emitter = Emitter {
            cfg,
            cm: self.source_map.clone(),
            comments: None,
            wr,
        };

        emitter.emit_program(&program)?;

        Ok((
            buf.into(),
            if emit_sourcemap {
                Some(self.source_map.build_source_map(srcmap.as_ref()))
            } else {
                None
            },
        ))
    }
}
