use std::io::stderr;

use bytes::Bytes;
use color_eyre::eyre::eyre;
#[cfg(feature = "react")]
use swc_core::ecma::transforms::react::react;
#[cfg(feature = "typescript")]
use swc_core::ecma::transforms::typescript::strip;
use swc_core::{
    base::{config::IsModule, Compiler},
    common::{
        comments::Comments, errors::Handler, sync::Lrc, BytePos, FileName, Globals, LineCol, Mark,
        SourceFile, SourceMap, GLOBALS,
    },
    ecma::{
        ast::EsVersion,
        codegen::{self, text_writer::JsWriter, Emitter},
        parser::Syntax,
        transforms::base::{fixer::fixer, hygiene::hygiene, resolver},
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
        source: &str,
        syntax: Syntax,
        is_module: IsModule,
        emit_sourcemap: bool,
    ) -> color_eyre::Result<(Bytes, Option<swc_core::base::sourcemap::SourceMap>)> {
        let fm = self
            .source_map
            .new_source_file_from(FileName::Anon, source.to_string().into());

        GLOBALS.set(&self.globals, || {
            self.do_transpile(syntax, is_module, emit_sourcemap, fm)
        })
    }

    fn do_transpile(
        &self,
        syntax: Syntax,
        is_module: IsModule,
        emit_sourcemap: bool,
        fm: Lrc<SourceFile>,
    ) -> color_eyre::Result<(Bytes, Option<swc_core::base::sourcemap::SourceMap>)> {
        let mut program = self
            .compiler
            .parse_js(
                fm,
                &self.handler,
                EsVersion::Es2022,
                syntax,
                is_module,
                Some(self.compiler.comments()),
            )
            .map_err(|err| eyre!(err))?;

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();

        program = match syntax {
            #[cfg(feature = "typescript")]
            Syntax::Typescript(_) => {
                program
                    .fold_with(&mut resolver(unresolved_mark, top_level_mark, true))
                    .fold_with(&mut strip(top_level_mark))
            }
            Syntax::Es(_) => {
                program.fold_with(&mut resolver(unresolved_mark, top_level_mark, false))
            }
            // This is needed because we are left with one case, so if we disabled typescript it is
            // left with an...interesting case effectively we are left with the
            // ECMAScript option, i.e. have exhaustive match already
            #[allow(unreachable_patterns)]
            _ => program,
        };

        let comments: Option<&dyn Comments> = Some(self.compiler.comments());

        #[cfg(feature = "react")]
        {
            program = program.fold_with(&mut react::<&dyn Comments>(
                self.source_map.clone(),
                comments,
                Default::default(),
                top_level_mark,
                unresolved_mark,
            ));
        }

        program = program
            .fold_with(&mut hygiene())
            .fold_with(&mut fixer(comments));

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
        
        let mut cfg = codegen::Config::default();
        cfg.target = EsVersion::Es2020;
        
        let mut emitter = Emitter {
            cfg,
            cm: self.source_map.clone(),
            comments,
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
