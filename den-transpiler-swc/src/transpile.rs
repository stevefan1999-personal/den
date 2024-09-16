use derive_more::{Debug, Display, Error, From};
use swc_core::ecma::parser::{EsSyntax, Syntax, TsSyntax};

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
