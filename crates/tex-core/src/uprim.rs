//! LuaTeX-only primitives (`\Umathcode`, the `\Umath` parameter family,
//! direction commands, ...) with their meanings.

/// A LuaTeX-only primitive without its own [`crate::prim::Prim`] variant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u16)]
pub enum UPrim {
    UMathCode,
    UMathCodeNum,
    UDelCode,
    UDelCodeNum,
    UMathCharDef,
    UMathCharNumDef,
    UMathChar,
    UMathCharNum,
    UMathAccent,
    UDelimiter,
    URadical,
    URoot,
    UDelimiterOver,
    UDelimiterUnder,
    UOverDelimiter,
    UUnderDelimiter,
    UHExtensible,
    UVExtensible,
    USkewed,
    USkewedWithDelims,
    UNoSubscript,
    UNoSuperscript,
    USubscript,
    USuperscript,
    UStartDisplayMath,
    UStopDisplayMath,
    UChar,
    UMathCharClass,
    UMathCharFam,
    UMathCharSlot,
    MathStyleValue,
    ImmediateAssignment,
    ImmediateAssigned,
    CrampedDisplayStyle,
    CrampedTextStyle,
    CrampedScriptStyle,
    CrampedScriptScriptStyle,
    HjCode,
    HyphenationMin,
    PreHyphenChar,
    PostHyphenChar,
    PreExHyphenChar,
    PostExHyphenChar,
    LateLua,
    LateLuaFunction,
    AutomaticDiscretionary,
    ClearMarks,
    EndLocalControl,
    GLeaders,
    IfCondition,
    LeftGhost,
    RightGhost,
    LocalLeftBox,
    LocalRightBox,
    MathOption,
    NoHRule,
    NoVRule,
    ScanTextokens,
    SetFontId,
    TextDir,
    ParDir,
    BodyDir,
    PageDir,
    MathDir,
    LineDir,
    BoxDir,
    BoxDirection,
}

pub const NUM_UPRIMS: usize = 67;

impl UPrim {
    pub const ALL: [UPrim; NUM_UPRIMS] = [
        UPrim::UMathCode,
        UPrim::UMathCodeNum,
        UPrim::UDelCode,
        UPrim::UDelCodeNum,
        UPrim::UMathCharDef,
        UPrim::UMathCharNumDef,
        UPrim::UMathChar,
        UPrim::UMathCharNum,
        UPrim::UMathAccent,
        UPrim::UDelimiter,
        UPrim::URadical,
        UPrim::URoot,
        UPrim::UDelimiterOver,
        UPrim::UDelimiterUnder,
        UPrim::UOverDelimiter,
        UPrim::UUnderDelimiter,
        UPrim::UHExtensible,
        UPrim::UVExtensible,
        UPrim::USkewed,
        UPrim::USkewedWithDelims,
        UPrim::UNoSubscript,
        UPrim::UNoSuperscript,
        UPrim::USubscript,
        UPrim::USuperscript,
        UPrim::UStartDisplayMath,
        UPrim::UStopDisplayMath,
        UPrim::UChar,
        UPrim::UMathCharClass,
        UPrim::UMathCharFam,
        UPrim::UMathCharSlot,
        UPrim::MathStyleValue,
        UPrim::ImmediateAssignment,
        UPrim::ImmediateAssigned,
        UPrim::CrampedDisplayStyle,
        UPrim::CrampedTextStyle,
        UPrim::CrampedScriptStyle,
        UPrim::CrampedScriptScriptStyle,
        UPrim::HjCode,
        UPrim::HyphenationMin,
        UPrim::PreHyphenChar,
        UPrim::PostHyphenChar,
        UPrim::PreExHyphenChar,
        UPrim::PostExHyphenChar,
        UPrim::LateLua,
        UPrim::LateLuaFunction,
        UPrim::AutomaticDiscretionary,
        UPrim::ClearMarks,
        UPrim::EndLocalControl,
        UPrim::GLeaders,
        UPrim::IfCondition,
        UPrim::LeftGhost,
        UPrim::RightGhost,
        UPrim::LocalLeftBox,
        UPrim::LocalRightBox,
        UPrim::MathOption,
        UPrim::NoHRule,
        UPrim::NoVRule,
        UPrim::ScanTextokens,
        UPrim::SetFontId,
        UPrim::TextDir,
        UPrim::ParDir,
        UPrim::BodyDir,
        UPrim::PageDir,
        UPrim::MathDir,
        UPrim::LineDir,
        UPrim::BoxDir,
        UPrim::BoxDirection,
    ];

    #[inline]
    pub fn idx(self) -> u16 {
        self as u16
    }

    pub fn from_idx(i: u16) -> Option<UPrim> {
        Self::ALL.get(i as usize).copied()
    }

    /// `convert_cmd`, `if_test_cmd` and `input_cmd` primitives.
    pub fn is_expandable(self) -> bool {
        matches!(
            self,
            UPrim::UChar
                | UPrim::UMathCharClass
                | UPrim::UMathCharFam
                | UPrim::UMathCharSlot
                | UPrim::MathStyleValue
                | UPrim::ImmediateAssignment
                | UPrim::ImmediateAssigned
                | UPrim::IfCondition
                | UPrim::ScanTextokens
        )
    }
}

/// LuaTeX names of the [`UPrim`] primitives.
pub static UPRIMS: &[(&[u8], UPrim)] = &[
    (b"Umathcode", UPrim::UMathCode),
    (b"Umathcodenum", UPrim::UMathCodeNum),
    (b"Udelcode", UPrim::UDelCode),
    (b"Udelcodenum", UPrim::UDelCodeNum),
    (b"Umathchardef", UPrim::UMathCharDef),
    (b"Umathcharnumdef", UPrim::UMathCharNumDef),
    (b"Umathchar", UPrim::UMathChar),
    (b"Umathcharnum", UPrim::UMathCharNum),
    (b"Umathaccent", UPrim::UMathAccent),
    (b"Udelimiter", UPrim::UDelimiter),
    (b"Uradical", UPrim::URadical),
    (b"Uroot", UPrim::URoot),
    (b"Udelimiterover", UPrim::UDelimiterOver),
    (b"Udelimiterunder", UPrim::UDelimiterUnder),
    (b"Uoverdelimiter", UPrim::UOverDelimiter),
    (b"Uunderdelimiter", UPrim::UUnderDelimiter),
    (b"Uhextensible", UPrim::UHExtensible),
    (b"Uvextensible", UPrim::UVExtensible),
    (b"Uskewed", UPrim::USkewed),
    (b"Uskewedwithdelims", UPrim::USkewedWithDelims),
    (b"Unosubscript", UPrim::UNoSubscript),
    (b"Unosuperscript", UPrim::UNoSuperscript),
    (b"Usubscript", UPrim::USubscript),
    (b"Usuperscript", UPrim::USuperscript),
    (b"Ustartdisplaymath", UPrim::UStartDisplayMath),
    (b"Ustopdisplaymath", UPrim::UStopDisplayMath),
    (b"Uchar", UPrim::UChar),
    (b"Umathcharclass", UPrim::UMathCharClass),
    (b"Umathcharfam", UPrim::UMathCharFam),
    (b"Umathcharslot", UPrim::UMathCharSlot),
    (b"mathstyle", UPrim::MathStyleValue),
    (b"immediateassignment", UPrim::ImmediateAssignment),
    (b"immediateassigned", UPrim::ImmediateAssigned),
    (b"crampeddisplaystyle", UPrim::CrampedDisplayStyle),
    (b"crampedtextstyle", UPrim::CrampedTextStyle),
    (b"crampedscriptstyle", UPrim::CrampedScriptStyle),
    (b"crampedscriptscriptstyle", UPrim::CrampedScriptScriptStyle),
    (b"hjcode", UPrim::HjCode),
    (b"hyphenationmin", UPrim::HyphenationMin),
    (b"prehyphenchar", UPrim::PreHyphenChar),
    (b"posthyphenchar", UPrim::PostHyphenChar),
    (b"preexhyphenchar", UPrim::PreExHyphenChar),
    (b"postexhyphenchar", UPrim::PostExHyphenChar),
    (b"latelua", UPrim::LateLua),
    (b"lateluafunction", UPrim::LateLuaFunction),
    (b"automaticdiscretionary", UPrim::AutomaticDiscretionary),
    (b"clearmarks", UPrim::ClearMarks),
    (b"endlocalcontrol", UPrim::EndLocalControl),
    (b"gleaders", UPrim::GLeaders),
    (b"ifcondition", UPrim::IfCondition),
    (b"leftghost", UPrim::LeftGhost),
    (b"rightghost", UPrim::RightGhost),
    (b"localleftbox", UPrim::LocalLeftBox),
    (b"localrightbox", UPrim::LocalRightBox),
    (b"mathoption", UPrim::MathOption),
    (b"nohrule", UPrim::NoHRule),
    (b"novrule", UPrim::NoVRule),
    (b"scantextokens", UPrim::ScanTextokens),
    (b"setfontid", UPrim::SetFontId),
    (b"textdir", UPrim::TextDir),
    (b"pardir", UPrim::ParDir),
    (b"bodydir", UPrim::BodyDir),
    (b"pagedir", UPrim::PageDir),
    (b"mathdir", UPrim::MathDir),
    (b"linedir", UPrim::LineDir),
    (b"boxdir", UPrim::BoxDir),
    (b"boxdirection", UPrim::BoxDirection),
];

/// `\Umath` parameter names indexed by LuaTeX's `math_param_*` number.
pub static UMATH_NAMES: [&[u8]; 113] = [
    b"Umathquad",
    b"Umathaxis",
    b"Umathoperatorsize",
    b"Umathoverbarkern",
    b"Umathoverbarrule",
    b"Umathoverbarvgap",
    b"Umathunderbarkern",
    b"Umathunderbarrule",
    b"Umathunderbarvgap",
    b"Umathradicalkern",
    b"Umathradicalrule",
    b"Umathradicalvgap",
    b"Umathradicaldegreebefore",
    b"Umathradicaldegreeafter",
    b"Umathradicaldegreeraise",
    b"Umathstackvgap",
    b"Umathstacknumup",
    b"Umathstackdenomdown",
    b"Umathfractionrule",
    b"Umathfractionnumvgap",
    b"Umathfractionnumup",
    b"Umathfractiondenomvgap",
    b"Umathfractiondenomdown",
    b"Umathfractiondelsize",
    b"Umathskewedfractionhgap",
    b"Umathskewedfractionvgap",
    b"Umathlimitabovevgap",
    b"Umathlimitabovebgap",
    b"Umathlimitabovekern",
    b"Umathlimitbelowvgap",
    b"Umathlimitbelowbgap",
    b"Umathlimitbelowkern",
    b"Umathnolimitsubfactor",
    b"Umathnolimitsupfactor",
    b"Umathunderdelimitervgap",
    b"Umathunderdelimiterbgap",
    b"Umathoverdelimitervgap",
    b"Umathoverdelimiterbgap",
    b"Umathsubshiftdrop",
    b"Umathsupshiftdrop",
    b"Umathsubshiftdown",
    b"Umathsubsupshiftdown",
    b"Umathsubtopmax",
    b"Umathsupshiftup",
    b"Umathsupbottommin",
    b"Umathsupsubbottommax",
    b"Umathsubsupvgap",
    b"Umathspaceafterscript",
    b"Umathconnectoroverlapmin",
    b"Umathordordspacing",
    b"Umathordopspacing",
    b"Umathordbinspacing",
    b"Umathordrelspacing",
    b"Umathordopenspacing",
    b"Umathordclosespacing",
    b"Umathordpunctspacing",
    b"Umathordinnerspacing",
    b"Umathopordspacing",
    b"Umathopopspacing",
    b"Umathopbinspacing",
    b"Umathoprelspacing",
    b"Umathopopenspacing",
    b"Umathopclosespacing",
    b"Umathoppunctspacing",
    b"Umathopinnerspacing",
    b"Umathbinordspacing",
    b"Umathbinopspacing",
    b"Umathbinbinspacing",
    b"Umathbinrelspacing",
    b"Umathbinopenspacing",
    b"Umathbinclosespacing",
    b"Umathbinpunctspacing",
    b"Umathbininnerspacing",
    b"Umathrelordspacing",
    b"Umathrelopspacing",
    b"Umathrelbinspacing",
    b"Umathrelrelspacing",
    b"Umathrelopenspacing",
    b"Umathrelclosespacing",
    b"Umathrelpunctspacing",
    b"Umathrelinnerspacing",
    b"Umathopenordspacing",
    b"Umathopenopspacing",
    b"Umathopenbinspacing",
    b"Umathopenrelspacing",
    b"Umathopenopenspacing",
    b"Umathopenclosespacing",
    b"Umathopenpunctspacing",
    b"Umathopeninnerspacing",
    b"Umathcloseordspacing",
    b"Umathcloseopspacing",
    b"Umathclosebinspacing",
    b"Umathcloserelspacing",
    b"Umathcloseopenspacing",
    b"Umathcloseclosespacing",
    b"Umathclosepunctspacing",
    b"Umathcloseinnerspacing",
    b"Umathpunctordspacing",
    b"Umathpunctopspacing",
    b"Umathpunctbinspacing",
    b"Umathpunctrelspacing",
    b"Umathpunctopenspacing",
    b"Umathpunctclosespacing",
    b"Umathpunctpunctspacing",
    b"Umathpunctinnerspacing",
    b"Umathinnerordspacing",
    b"Umathinneropspacing",
    b"Umathinnerbinspacing",
    b"Umathinnerrelspacing",
    b"Umathinneropenspacing",
    b"Umathinnerclosespacing",
    b"Umathinnerpunctspacing",
    b"Umathinnerinnerspacing",
];

/// LuaTeX `math_param_*` numbers.
#[allow(dead_code)]
pub mod mp {
    pub const MATH_PARAM_QUAD: u32 = 0;
    pub const MATH_PARAM_AXIS: u32 = 1;
    pub const MATH_PARAM_OPERATOR_SIZE: u32 = 2;
    pub const MATH_PARAM_OVERBAR_KERN: u32 = 3;
    pub const MATH_PARAM_OVERBAR_RULE: u32 = 4;
    pub const MATH_PARAM_OVERBAR_VGAP: u32 = 5;
    pub const MATH_PARAM_UNDERBAR_KERN: u32 = 6;
    pub const MATH_PARAM_UNDERBAR_RULE: u32 = 7;
    pub const MATH_PARAM_UNDERBAR_VGAP: u32 = 8;
    pub const MATH_PARAM_RADICAL_KERN: u32 = 9;
    pub const MATH_PARAM_RADICAL_RULE: u32 = 10;
    pub const MATH_PARAM_RADICAL_VGAP: u32 = 11;
    pub const MATH_PARAM_RADICAL_DEGREE_BEFORE: u32 = 12;
    pub const MATH_PARAM_RADICAL_DEGREE_AFTER: u32 = 13;
    pub const MATH_PARAM_RADICAL_DEGREE_RAISE: u32 = 14;
    pub const MATH_PARAM_STACK_VGAP: u32 = 15;
    pub const MATH_PARAM_STACK_NUM_UP: u32 = 16;
    pub const MATH_PARAM_STACK_DENOM_DOWN: u32 = 17;
    pub const MATH_PARAM_FRACTION_RULE: u32 = 18;
    pub const MATH_PARAM_FRACTION_NUM_VGAP: u32 = 19;
    pub const MATH_PARAM_FRACTION_NUM_UP: u32 = 20;
    pub const MATH_PARAM_FRACTION_DENOM_VGAP: u32 = 21;
    pub const MATH_PARAM_FRACTION_DENOM_DOWN: u32 = 22;
    pub const MATH_PARAM_FRACTION_DEL_SIZE: u32 = 23;
    pub const MATH_PARAM_SKEWED_FRACTION_HGAP: u32 = 24;
    pub const MATH_PARAM_SKEWED_FRACTION_VGAP: u32 = 25;
    pub const MATH_PARAM_LIMIT_ABOVE_VGAP: u32 = 26;
    pub const MATH_PARAM_LIMIT_ABOVE_BGAP: u32 = 27;
    pub const MATH_PARAM_LIMIT_ABOVE_KERN: u32 = 28;
    pub const MATH_PARAM_LIMIT_BELOW_VGAP: u32 = 29;
    pub const MATH_PARAM_LIMIT_BELOW_BGAP: u32 = 30;
    pub const MATH_PARAM_LIMIT_BELOW_KERN: u32 = 31;
    pub const MATH_PARAM_NOLIMIT_SUB_FACTOR: u32 = 32;
    pub const MATH_PARAM_NOLIMIT_SUP_FACTOR: u32 = 33;
    pub const MATH_PARAM_UNDER_DELIMITER_VGAP: u32 = 34;
    pub const MATH_PARAM_UNDER_DELIMITER_BGAP: u32 = 35;
    pub const MATH_PARAM_OVER_DELIMITER_VGAP: u32 = 36;
    pub const MATH_PARAM_OVER_DELIMITER_BGAP: u32 = 37;
    pub const MATH_PARAM_SUB_SHIFT_DROP: u32 = 38;
    pub const MATH_PARAM_SUP_SHIFT_DROP: u32 = 39;
    pub const MATH_PARAM_SUB_SHIFT_DOWN: u32 = 40;
    pub const MATH_PARAM_SUB_SUP_SHIFT_DOWN: u32 = 41;
    pub const MATH_PARAM_SUB_TOP_MAX: u32 = 42;
    pub const MATH_PARAM_SUP_SHIFT_UP: u32 = 43;
    pub const MATH_PARAM_SUP_BOTTOM_MIN: u32 = 44;
    pub const MATH_PARAM_SUP_SUB_BOTTOM_MAX: u32 = 45;
    pub const MATH_PARAM_SUBSUP_VGAP: u32 = 46;
    pub const MATH_PARAM_SPACE_AFTER_SCRIPT: u32 = 47;
    pub const MATH_PARAM_CONNECTOR_OVERLAP_MIN: u32 = 48;
    pub const MATH_PARAM_ORD_ORD_SPACING: u32 = 49;
    pub const MATH_PARAM_ORD_OP_SPACING: u32 = 50;
    pub const MATH_PARAM_ORD_BIN_SPACING: u32 = 51;
    pub const MATH_PARAM_ORD_REL_SPACING: u32 = 52;
    pub const MATH_PARAM_ORD_OPEN_SPACING: u32 = 53;
    pub const MATH_PARAM_ORD_CLOSE_SPACING: u32 = 54;
    pub const MATH_PARAM_ORD_PUNCT_SPACING: u32 = 55;
    pub const MATH_PARAM_ORD_INNER_SPACING: u32 = 56;
    pub const MATH_PARAM_OP_ORD_SPACING: u32 = 57;
    pub const MATH_PARAM_OP_OP_SPACING: u32 = 58;
    pub const MATH_PARAM_OP_BIN_SPACING: u32 = 59;
    pub const MATH_PARAM_OP_REL_SPACING: u32 = 60;
    pub const MATH_PARAM_OP_OPEN_SPACING: u32 = 61;
    pub const MATH_PARAM_OP_CLOSE_SPACING: u32 = 62;
    pub const MATH_PARAM_OP_PUNCT_SPACING: u32 = 63;
    pub const MATH_PARAM_OP_INNER_SPACING: u32 = 64;
    pub const MATH_PARAM_BIN_ORD_SPACING: u32 = 65;
    pub const MATH_PARAM_BIN_OP_SPACING: u32 = 66;
    pub const MATH_PARAM_BIN_BIN_SPACING: u32 = 67;
    pub const MATH_PARAM_BIN_REL_SPACING: u32 = 68;
    pub const MATH_PARAM_BIN_OPEN_SPACING: u32 = 69;
    pub const MATH_PARAM_BIN_CLOSE_SPACING: u32 = 70;
    pub const MATH_PARAM_BIN_PUNCT_SPACING: u32 = 71;
    pub const MATH_PARAM_BIN_INNER_SPACING: u32 = 72;
    pub const MATH_PARAM_REL_ORD_SPACING: u32 = 73;
    pub const MATH_PARAM_REL_OP_SPACING: u32 = 74;
    pub const MATH_PARAM_REL_BIN_SPACING: u32 = 75;
    pub const MATH_PARAM_REL_REL_SPACING: u32 = 76;
    pub const MATH_PARAM_REL_OPEN_SPACING: u32 = 77;
    pub const MATH_PARAM_REL_CLOSE_SPACING: u32 = 78;
    pub const MATH_PARAM_REL_PUNCT_SPACING: u32 = 79;
    pub const MATH_PARAM_REL_INNER_SPACING: u32 = 80;
    pub const MATH_PARAM_OPEN_ORD_SPACING: u32 = 81;
    pub const MATH_PARAM_OPEN_OP_SPACING: u32 = 82;
    pub const MATH_PARAM_OPEN_BIN_SPACING: u32 = 83;
    pub const MATH_PARAM_OPEN_REL_SPACING: u32 = 84;
    pub const MATH_PARAM_OPEN_OPEN_SPACING: u32 = 85;
    pub const MATH_PARAM_OPEN_CLOSE_SPACING: u32 = 86;
    pub const MATH_PARAM_OPEN_PUNCT_SPACING: u32 = 87;
    pub const MATH_PARAM_OPEN_INNER_SPACING: u32 = 88;
    pub const MATH_PARAM_CLOSE_ORD_SPACING: u32 = 89;
    pub const MATH_PARAM_CLOSE_OP_SPACING: u32 = 90;
    pub const MATH_PARAM_CLOSE_BIN_SPACING: u32 = 91;
    pub const MATH_PARAM_CLOSE_REL_SPACING: u32 = 92;
    pub const MATH_PARAM_CLOSE_OPEN_SPACING: u32 = 93;
    pub const MATH_PARAM_CLOSE_CLOSE_SPACING: u32 = 94;
    pub const MATH_PARAM_CLOSE_PUNCT_SPACING: u32 = 95;
    pub const MATH_PARAM_CLOSE_INNER_SPACING: u32 = 96;
    pub const MATH_PARAM_PUNCT_ORD_SPACING: u32 = 97;
    pub const MATH_PARAM_PUNCT_OP_SPACING: u32 = 98;
    pub const MATH_PARAM_PUNCT_BIN_SPACING: u32 = 99;
    pub const MATH_PARAM_PUNCT_REL_SPACING: u32 = 100;
    pub const MATH_PARAM_PUNCT_OPEN_SPACING: u32 = 101;
    pub const MATH_PARAM_PUNCT_CLOSE_SPACING: u32 = 102;
    pub const MATH_PARAM_PUNCT_PUNCT_SPACING: u32 = 103;
    pub const MATH_PARAM_PUNCT_INNER_SPACING: u32 = 104;
    pub const MATH_PARAM_INNER_ORD_SPACING: u32 = 105;
    pub const MATH_PARAM_INNER_OP_SPACING: u32 = 106;
    pub const MATH_PARAM_INNER_BIN_SPACING: u32 = 107;
    pub const MATH_PARAM_INNER_REL_SPACING: u32 = 108;
    pub const MATH_PARAM_INNER_OPEN_SPACING: u32 = 109;
    pub const MATH_PARAM_INNER_CLOSE_SPACING: u32 = 110;
    pub const MATH_PARAM_INNER_PUNCT_SPACING: u32 = 111;
    pub const MATH_PARAM_INNER_INNER_SPACING: u32 = 112;
    pub const MATH_PARAM_FIRST_MU_GLUE: u32 = MATH_PARAM_ORD_ORD_SPACING;
    pub const MATH_PARAM_LAST: u32 = 113;
}
