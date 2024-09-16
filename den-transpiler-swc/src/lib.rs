use std::{io, string::FromUtf8Error};

use derive_more::{Debug, Display, Error, From, Into};
pub use swc_core;
#[cfg(feature = "react")]
use swc_core::ecma::transforms::react::react;
#[cfg(feature = "typescript")]
use swc_core::ecma::transforms::typescript::strip;
use swc_core::{
    base::{config::IsModule, Compiler},
    common::{
        comments::Comments,
        errors::{ColorConfig, Handler},
        sync::Lrc,
        BytePos, FileName, Globals, LineCol, Mark, SourceFile, SourceMap, GLOBALS,
    },
    ecma::{
        ast::EsVersion,
        codegen::{self, text_writer::JsWriter, Emitter},
        parser::{EsSyntax, Syntax, TsSyntax},
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

        let handler =
            Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(source_map.clone()));
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
    ) -> Result<(String, Option<swc_core::base::sourcemap::SourceMap>), EasySwcTranspilerError>
    {
        let fm = self
            .source_map
            .new_source_file_from(FileName::Anon.into(), source.to_string().into());

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
    ) -> Result<(String, Option<swc_core::base::sourcemap::SourceMap>), EasySwcTranspilerError>
    {
        let mut program = self
            .compiler
            .parse_js(
                fm,
                &self.handler,
                EsVersion::EsNext,
                syntax,
                is_module,
                Some(self.compiler.comments()),
            )
            .map_err(EasySwcTranspilerError::SwcParse)?;

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();

        program = match syntax {
            #[cfg(feature = "typescript")]
            Syntax::Typescript(_) => {
                program
                    .fold_with(&mut resolver(unresolved_mark, top_level_mark, true))
                    .fold_with(&mut strip(unresolved_mark, top_level_mark))
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
        cfg.target = EsVersion::Es2022;
        cfg.omit_last_semi = true;

        let mut emitter = Emitter {
            cfg,
            cm: self.source_map.clone(),
            comments,
            wr,
        };

        emitter
            .emit_program(&program)
            .map_err(EasySwcTranspilerError::SwcEmitProgram)?;

        Ok((
            String::from_utf8(buf)?,
            if emit_sourcemap {
                Some(self.source_map.build_source_map(srcmap.as_ref()))
            } else {
                None
            },
        ))
    }
}

#[derive(From, Error, Display, Debug)]
pub enum EasySwcTranspilerError {
    #[from]
    SwcParse(anyhow::Error),
    #[from]
    SwcEmitProgram(io::Error),
    #[from]
    Utf8(FromUtf8Error),
}

pub fn infer_transpile_syntax_by_extension(extension: &str) -> Option<Syntax> {
    trie_match::trie_match! {
        match extension {
            "js" | "mjs" => { Some(Syntax::Es(Default::default())) }
            "jsx" | "mjsx" => {
                if cfg!(feature = "react") {
                    Some(Syntax::Es(EsSyntax { jsx: true, ..Default::default() }))
                } else {
                    None
                }
            }
            "ts" => {
                if cfg!(feature = "typescript") {
                    Some(Syntax::Typescript(Default::default()))
                } else {
                    None
                }
            }
            "tsx" => {
                if cfg!(all(feature = "typescript", feature = "react")) {
                    Some(Syntax::Typescript(TsSyntax { tsx: true, ..Default::default() }))
                } else {
                    None
                }
            }
            _ => { None }
        }
    }
}

#[derive(Display, From, Error, Debug)]
pub enum InferTranspileSyntaxError {
    InvalidExtension,
}

pub const fn get_best_transpiling() -> &'static str {
    match (cfg!(feature = "typescript"), cfg!(feature = "react")) {
        (false, false) => "js",
        (false, true) => "jsx",
        (true, false) => "ts",
        (true, true) => "tsx",
    }
}
