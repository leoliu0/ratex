// Standard libraries for the supported Lua language contracts.

pub mod basic;
pub mod bit32;
pub mod coroutine;
pub mod debug;
pub mod io;
pub mod math;
pub mod os;
pub mod package;
mod sort_table;
pub mod string;
pub mod table;
pub mod utf8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stdlib {
    Bit32,
    Io,
    Os,
    Math,
    String,
    Table,
    Basic,
    Package,
    Utf8,
    Coroutine,
    Debug,

    All,
}
