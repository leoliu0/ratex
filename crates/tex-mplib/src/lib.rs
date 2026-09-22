//! tex-mplib: Pure-Rust MetaPost 3.00 engine and mplib implementation.

pub mod curves;
pub mod interp;
pub mod session;
pub mod solver;
pub mod types;

pub use curves::{solve_path, KnotSpec};
pub use interp::Interpreter;
pub use session::{MpConfig, MpResult, MpSession};
pub use solver::{LinearExpr, LinearSolver};
pub use types::{Color, Dash, Knot, MpFigure, MpObject, Pair, Path, Pen, Transform};

/// Returns the mplib implementation version string.
pub const MPLIB_VERSION: &str = "3.00";
