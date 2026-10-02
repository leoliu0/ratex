use core::fmt;

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum LuaTokenKind {
    // KeyWord
    TkAnd,
    TkBreak,
    TkDo,
    TkElse,
    TkElseIf,
    TkEnd,
    TkFalse,
    TkFor,
    TkFunction,
    TkGoto,
    TkIf,
    TkIn,
    TkLocal,
    TkNil,
    TkNot,
    TkOr,
    TkRepeat,
    TkReturn,
    TkThen,
    TkTrue,
    TkUntil,
    TkWhile,

    TkWhitespace, // whitespace
    TkEndOfLine,  // end of line
    TkPlus,       // +
    TkMinus,      // -
    TkMul,        // *
    TkDiv,        // /
    TkIDiv,       // //
    TkDot,        // .
    TkConcat,     // ..
    TkDots,       // ...
    TkComma,      // ,
    TkAssign,     // =
    TkEq,         // ==
    TkGe,         // >=
    TkLe,         // <=
    TkNe,         // ~=
    TkShl,        // <<
    TkShr,        // >>
    TkLt,         // <
    TkGt,         // >
    TkMod,        // %
    TkPow,        // ^
    TkLen,        // #
    TkBitAnd,     // &
    TkBitOr,      // |
    TkBitXor,     // ~
    TkColon,      // :
    TkDbColon,    // ::
    TkSemicolon,  // ;

    TkLeftBracket,  // [
    TkRightBracket, // ]
    TkLeftParen,    // (
    TkRightParen,   // )
    TkLeftBrace,    // {
    TkRightBrace,   // }
    TkComplex,      // complex
    TkInt,          // int
    TkFloat,        // float

    TkName,         // name
    TkString,       // string
    TkLongString,   // long string
    TkShortComment, // short comment
    TkLongComment,  // long comment
    TkEof,          // eof

    TkUnknown, // unknown
}

impl fmt::Display for LuaTokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_user_string())
    }
}

impl LuaTokenKind {
    /// Convert token kind to user-readable string (like Lua's luaX_token2str)
    pub fn to_user_string(self) -> &'static str {
        match self {
            // Keywords
            LuaTokenKind::TkAnd => "and",
            LuaTokenKind::TkBreak => "break",
            LuaTokenKind::TkDo => "do",
            LuaTokenKind::TkElse => "else",
            LuaTokenKind::TkElseIf => "elseif",
            LuaTokenKind::TkEnd => "end",
            LuaTokenKind::TkFalse => "false",
            LuaTokenKind::TkFor => "for",
            LuaTokenKind::TkFunction => "function",
            LuaTokenKind::TkGoto => "goto",
            LuaTokenKind::TkIf => "if",
            LuaTokenKind::TkIn => "in",
            LuaTokenKind::TkLocal => "local",
            LuaTokenKind::TkNil => "nil",
            LuaTokenKind::TkNot => "not",
            LuaTokenKind::TkOr => "or",
            LuaTokenKind::TkRepeat => "repeat",
            LuaTokenKind::TkReturn => "return",
            LuaTokenKind::TkThen => "then",
            LuaTokenKind::TkTrue => "true",
            LuaTokenKind::TkUntil => "until",
            LuaTokenKind::TkWhile => "while",
            // Symbols
            LuaTokenKind::TkPlus => "+",
            LuaTokenKind::TkMinus => "-",
            LuaTokenKind::TkMul => "*",
            LuaTokenKind::TkDiv => "/",
            LuaTokenKind::TkIDiv => "//",
            LuaTokenKind::TkDot => ".",
            LuaTokenKind::TkConcat => "..",
            LuaTokenKind::TkDots => "...",
            LuaTokenKind::TkComma => ",",
            LuaTokenKind::TkAssign => "=",
            LuaTokenKind::TkEq => "==",
            LuaTokenKind::TkGe => ">=",
            LuaTokenKind::TkLe => "<=",
            LuaTokenKind::TkNe => "~=",
            LuaTokenKind::TkShl => "<<",
            LuaTokenKind::TkShr => ">>",
            LuaTokenKind::TkLt => "<",
            LuaTokenKind::TkGt => ">",
            LuaTokenKind::TkMod => "%",
            LuaTokenKind::TkPow => "^",
            LuaTokenKind::TkLen => "#",
            LuaTokenKind::TkBitAnd => "&",
            LuaTokenKind::TkBitOr => "|",
            LuaTokenKind::TkBitXor => "~",
            LuaTokenKind::TkColon => ":",
            LuaTokenKind::TkDbColon => "::",
            LuaTokenKind::TkSemicolon => ";",
            LuaTokenKind::TkLeftBracket => "[",
            LuaTokenKind::TkRightBracket => "]",
            LuaTokenKind::TkLeftParen => "(",
            LuaTokenKind::TkRightParen => ")",
            LuaTokenKind::TkLeftBrace => "{",
            LuaTokenKind::TkRightBrace => "}",
            // Literals
            LuaTokenKind::TkInt => "<integer>",
            LuaTokenKind::TkFloat => "<number>",
            LuaTokenKind::TkName => "<name>",
            LuaTokenKind::TkString => "<string>",
            LuaTokenKind::TkLongString => "<string>",
            LuaTokenKind::TkEof => "<eof>",
            // Others
            _ => "<unknown>",
        }
    }

    /// The token as `error_expected` prints it (`luaX_token2str`): reserved
    /// words and symbols are quoted, the placeholders `<name>`, `<eof>`... are not.
    pub fn expected_text(self) -> String {
        match self {
            LuaTokenKind::TkInt
            | LuaTokenKind::TkFloat
            | LuaTokenKind::TkName
            | LuaTokenKind::TkString
            | LuaTokenKind::TkLongString
            | LuaTokenKind::TkEof => self.to_user_string().to_string(),
            _ => format!("'{}'", self.to_user_string()),
        }
    }
}
