//! Authentic execution profiles for pdfTeX, XeTeX, and LuaTeX.

/// Explicit authentic engine profile identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum EngineKind {
    PdfTeX = 0,
    XeTeX = 1,
    LuaTeX = 2,
}

impl Default for EngineKind {
    fn default() -> Self {
        Self::PdfTeX
    }
}

impl EngineKind {
    /// Canonical CLI command name for this profile.
    pub const fn command_name(self) -> &'static str {
        match self {
            Self::PdfTeX => "pdflatex",
            Self::XeTeX => "xelatex",
            Self::LuaTeX => "lualatex",
        }
    }

    /// Underlying TeX program / binary name.
    pub const fn program_name(self) -> &'static str {
        match self {
            Self::PdfTeX => "pdftex",
            Self::XeTeX => "xetex",
            Self::LuaTeX => "luahbtex",
        }
    }

    /// Canonical format dump name for this engine.
    pub const fn default_format(self) -> &'static str {
        match self {
            Self::PdfTeX => "pdflatex.fmt",
            Self::XeTeX => "xelatex.fmt",
            Self::LuaTeX => "lualatex.fmt",
        }
    }

    /// Whether this profile operates in native Unicode scalar space.
    #[inline]
    pub const fn is_unicode(self) -> bool {
        matches!(self, Self::XeTeX | Self::LuaTeX)
    }

    /// The maximum valid character code in this profile's character tables.
    #[inline]
    pub const fn max_character_code(self) -> u32 {
        if self.is_unicode() {
            0x10_FFFF
        } else {
            255
        }
    }

    /// Whether this profile exposes the Lua runtime and primitives.
    #[inline]
    pub const fn supports_lua(self) -> bool {
        matches!(self, Self::LuaTeX)
    }

    /// Whether this profile exposes the XeTeX primitives and font queries.
    #[inline]
    pub const fn supports_xetex(self) -> bool {
        matches!(self, Self::XeTeX)
    }
}

/// Compilation engine selection request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum EngineChoice {
    /// Automatic detection and transactional convergence.
    #[default]
    Auto,
    /// Explicit engine profile specified by user or CLI flag.
    Explicit(EngineKind),
}

impl EngineChoice {
    /// Resolves the effective engine kind given an optional detected profile.
    pub fn resolve(self, detected: Option<EngineKind>) -> EngineKind {
        match self {
            Self::Explicit(kind) => kind,
            Self::Auto => detected.unwrap_or(EngineKind::PdfTeX),
        }
    }
}
