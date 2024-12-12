use std::{io, string::FromUtf8Error};

use derive_more::{Debug, Display, Error, From, Into};
pub use sourcemap::SourceMap;
use swc_common::{
    comments::Comments,
    errors::{ColorConfig, Handler},
    sync::Lrc,
    BytePos, FileName, Globals, LineCol, Mark, SourceFile, SourceMap as SwcSourceMap, GLOBALS,
};
pub use swc_config::IsModule;
use swc_ecma_ast::EsVersion;
use swc_ecma_codegen::{self, text_writer::JsWriter, Emitter};
pub use swc_ecma_parser::Syntax;
use swc_ecma_parser::{EsSyntax, TsSyntax};
use swc_ecma_transforms_base::{fixer::fixer, hygiene::hygiene, resolver};
#[cfg(feature = "react")]
use swc_ecma_transforms_react::react;
#[cfg(feature = "typescript")]
use swc_ecma_transforms_typescript::typescript;
use swc_node_comments::SwcComments;
pub struct EasySwcTranspiler {
    source_map: Lrc<SwcSourceMap>,
    comments:   SwcComments,
    handler:    Handler,
    globals:    Globals,
}

impl Default for EasySwcTranspiler {
    fn default() -> Self {
        let source_map: Lrc<SwcSourceMap> = Default::default();

        let handler =
            Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(source_map.clone()));
        let globals = Globals::new();
        let comments = SwcComments::default();

        Self {
            source_map,
            comments,
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
    ) -> Result<(String, Option<::sourcemap::SourceMap>), EasySwcTranspilerError> {
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
    ) -> Result<(String, Option<::sourcemap::SourceMap>), EasySwcTranspilerError> {
        let mut program = swc_compiler_base::parse_js(
            self.source_map.clone(),
            fm,
            &self.handler,
            EsVersion::EsNext,
            syntax,
            is_module,
            Some(&self.comments),
        )
        .map_err(EasySwcTranspilerError::SwcParse)?;

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();

        program = match syntax {
            #[cfg(feature = "typescript")]
            Syntax::Typescript(_) => {
                program
                    .apply(&mut resolver(unresolved_mark, top_level_mark, true))
                    .apply(&mut typescript(
                        {
                            let mut config = swc_ecma_transforms_typescript::Config::default();
                            config.native_class_properties = true;
                            config
                        },
                        unresolved_mark,
                        top_level_mark,
                    ))
            }
            Syntax::Es(_) => program.apply(&mut resolver(unresolved_mark, top_level_mark, false)),
            // This is needed because we are left with one case, so if we disabled typescript it is
            // left with an...interesting case effectively we are left with the
            // ECMAScript option, i.e. have exhaustive match already
            #[allow(unreachable_patterns)]
            _ => program,
        };

        let comments: Option<&dyn Comments> = Some(&self.comments);

        #[cfg(feature = "react")]
        {
            program = program.apply(&mut react::<&dyn Comments>(
                self.source_map.clone(),
                comments,
                Default::default(),
                top_level_mark,
                unresolved_mark,
            ));
        }

        program = program.apply(&mut hygiene()).apply(&mut fixer(comments));

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

        let mut cfg = swc_ecma_codegen::Config::default();
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

        let source_map = emit_sourcemap.then(|| self.source_map.build_source_map(srcmap.as_ref()));
        Ok((String::from_utf8(buf)?, source_map))
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
