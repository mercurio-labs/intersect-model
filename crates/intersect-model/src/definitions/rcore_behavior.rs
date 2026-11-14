//! Handwritten Rcore behavior frontend. Syntax is separate from semantic execution.
use crate::definitions::bundle::{DefinitionSource, SourceSpan};
use serde::{Deserialize, Serialize};

pub const SYNTAX_VERSION: u32 = 1;
#[path = "rcore_behavior_ast.rs"]
pub mod ast;
#[path = "rcore_behavior_parser.rs"]
mod parser;
pub use parser::parse;
pub(crate) use parser::parse_with_limits;
#[path = "rcore_behavior_bindings.rs"]
pub mod bindings;
#[path = "rcore_behavior_check.rs"]
pub mod checking;
#[path = "rcore_behavior_function_lowering.rs"]
pub mod function_lowering;
#[path = "rcore_behavior_inference.rs"]
mod inference;
#[path = "rcore_behavior_ir.rs"]
pub mod ir;
#[path = "rcore_behavior_ir_v2.rs"]
pub mod ir_v2;
#[path = "rcore_behavior_lowering.rs"]
pub mod lowering;
#[path = "rcore_behavior_model_types.rs"]
pub mod model_types;
#[path = "rcore_behavior_modules.rs"]
pub mod modules;
#[path = "rcore_behavior_patterns.rs"]
mod patterns;
#[path = "rcore_behavior_signatures.rs"]
pub mod signatures;
#[path = "rcore_behavior_symbolic.rs"]
mod symbolic;
#[path = "rcore_behavior_types.rs"]
pub mod types;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyntaxDiagnostic {
    pub span: SourceSpan,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenKind {
    Word(String),
    QuotedName(String),
    String(String),
    /// Exact decimal spelling, never rounded to a host numeric representation.
    Number(String),
    Symbol(String),
    Error,
    End,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    pub kind: TokenKind,
    pub span: SourceSpan,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lexed {
    pub source: DefinitionSource,
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<SyntaxDiagnostic>,
}

#[path = "rcore_lexical_preparation.rs"]
mod lexical;
pub(crate) use lexical::parts_retained as lex_parts_retained;
pub use lexical::{LexicalOutput, LexicalPreparation};

/// Tokenize exact source bytes by draining the shared lexical continuation.
pub fn lex(source: DefinitionSource) -> Lexed {
    let (tokens, diagnostics) =
        crate::definitions::continuation::complete(lex_parts_retained(&source));
    Lexed {
        source,
        tokens,
        diagnostics,
    }
}
