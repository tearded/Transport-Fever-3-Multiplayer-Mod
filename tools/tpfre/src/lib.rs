//! tpfre: a static binary-analysis kit for decoding a game executable.
//!
//! `tpfre index` makes one SQLite file from a PE32+ x86-64 binary in a
//! single parallel pass; `tpfre q` answers small questions about it with
//! short, greppable lines; `tpfre diff` compares two builds; `tpfre match`
//! carries names from one build to another that lacks them. See README.md.

pub mod archive;
pub mod audit;
pub mod build_gate;
pub mod cli;
pub mod db;
pub mod diff;
pub mod disasm;
pub mod index;
pub mod matching;
pub mod naming;
pub mod pe;
pub mod query;
pub mod rtti;
pub mod sig;
pub mod strings;
