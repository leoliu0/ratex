use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum LuaLanguageLevel {
    #[allow(unused)]
    LuaJIT,
    /// The language and standard-library contract embedded by LuaTeX.
    #[default]
    Lua53,
    /// Upstream lua-rs compatibility mode.
    Lua55,
}

impl fmt::Display for LuaLanguageLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LuaLanguageLevel::LuaJIT => write!(f, "LuaJIT"),
            LuaLanguageLevel::Lua53 => write!(f, "Lua 5.3"),
            LuaLanguageLevel::Lua55 => write!(f, "Lua 5.5"),
        }
    }
}
