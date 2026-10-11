//! Call of Duty 4's game scripts (GSC), as the campaign's levels ship them
//! (rawfiles in each level's zone and `common.ff`): a parser, and (to come)
//! an interpreter whose builtins the game provides.

pub mod ast;
pub mod level;
pub mod lexer;
pub mod parser;
pub mod vm;

pub use parser::parse;
