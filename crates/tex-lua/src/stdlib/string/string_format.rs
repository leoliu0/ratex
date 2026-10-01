//! `string.format`: port of `str_format` from lstrlib.c for the Lua 5.3
//! (`scanformat`) and Lua 5.5 (`getformat`/`checkformat`) dialects, with the
//! C conversions done by `stdlib::numfmt`.

use crate::LuaLanguageLevel;
use crate::lua_value::{LuaValue, LuaValueKind};
use crate::lua_vm::{LuaResult, LuaState};
use crate::stdlib::lauxlib;
use crate::stdlib::numfmt::{self, Spec};

/// Flags accepted by Lua 5.5's `checkformat` per conversion.
const FLAGS_F: &[u8] = b"-+ #0";
const FLAGS_X: &[u8] = b"-#0";
const FLAGS_I: &[u8] = b"-+ 0";
const FLAGS_U: &[u8] = b"-0";
const FLAGS_C: &[u8] = b"-";

/// Lua 5.5's `MAX_FORMAT`: longest accepted specification is 21 bytes.
const MAX_FORMAT: usize = 32;

/// string.format(formatstring, ...)
pub fn string_format(l: &mut LuaState) -> LuaResult<usize> {
    let lua53 = l.global_state().language() == LuaLanguageLevel::Lua53;
    let format = lauxlib::check_lstring(l, 1)?;
    let fmt: &[u8] = &format;
    let top = l.arg_count();
    let mut arg = 1;
    let mut out: Vec<u8> = Vec::with_capacity(fmt.len() + 16);
    let mut i = 0;
    while i < fmt.len() {
        let start = i;
        while i < fmt.len() && fmt[i] != b'%' {
            i += 1;
        }
        out.extend_from_slice(&fmt[start..i]);
        if i == fmt.len() {
            break;
        }
        i += 1;
        if fmt.get(i) == Some(&b'%') {
            out.push(b'%');
            i += 1;
            continue;
        }
        arg += 1;
        if arg > top {
            return Err(lauxlib::argerror(l, arg, "no value"));
        }
        let conv_pos = if lua53 {
            scan_format_53(l, fmt, i)?
        } else {
            scan_format_55(l, fmt, i)?
        };
        // `form` is the C specification without the '%': modifiers and
        // conversion (absent when the format string ends early).
        let form = &fmt[i..(conv_pos + 1).min(fmt.len())];
        let modifiers = &fmt[i..conv_pos];
        let conv = fmt.get(conv_pos).copied().unwrap_or(0);
        i = conv_pos + 1;
        let value = l.get_arg(arg).unwrap_or_default();
        match conv {
            b'c' => {
                if !lua53 {
                    check_format(l, form, FLAGS_C, false)?;
                }
                let n = lauxlib::check_integer(l, arg)?;
                numfmt::push_padded_bytes(&mut out, &[n as u8], &parse_spec(modifiers));
            }
            b'd' | b'i' | b'u' | b'o' | b'x' | b'X' => {
                if value.ttisinteger() && modifiers.is_empty() && matches!(conv, b'd' | b'i') {
                    out.extend_from_slice(itoa::Buffer::new().format(value.ivalue()).as_bytes());
                    continue;
                }
                let n = lauxlib::check_integer(l, arg)?;
                if !lua53 {
                    let flags = match conv {
                        b'd' | b'i' => FLAGS_I,
                        b'u' => FLAGS_U,
                        _ => FLAGS_X,
                    };
                    check_format(l, form, flags, true)?;
                }
                numfmt::push_int(&mut out, n, conv, &parse_spec(modifiers));
            }
            b'a' | b'A' => {
                if !lua53 {
                    check_format(l, form, FLAGS_F, true)?;
                }
                let n = lauxlib::check_number(l, arg)?;
                numfmt::push_float(&mut out, n, conv, &parse_spec(modifiers));
            }
            b'e' | b'E' | b'f' | b'g' | b'G' => {
                let n = lauxlib::check_number(l, arg)?;
                if !lua53 {
                    check_format(l, form, FLAGS_F, true)?;
                }
                numfmt::push_float(&mut out, n, conv, &parse_spec(modifiers));
            }
            b'p' if !lua53 => {
                check_format(l, form, FLAGS_C, false)?;
                let spec = parse_spec(modifiers);
                match pointer_of(&value) {
                    Some(pointer) => {
                        let text = format!("{pointer:#x}");
                        numfmt::push_padded_bytes(&mut out, text.as_bytes(), &spec);
                    }
                    None => numfmt::push_padded_bytes(&mut out, b"(null)", &spec),
                }
            }
            b'q' => {
                if !lua53 && !modifiers.is_empty() {
                    return Err(lauxlib::lual_error(l, "specifier '%q' cannot have modifiers"));
                }
                add_literal(l, &mut out, arg, &value)?;
            }
            b's' => {
                let text = if value.is_string() {
                    lauxlib::LStr::Value(value)
                } else {
                    lauxlib::tolstring(l, &value)?
                };
                if modifiers.is_empty() {
                    out.extend_from_slice(&text);
                } else {
                    if text.contains(&0) {
                        return Err(lauxlib::argerror(l, arg, "string contains zeros"));
                    }
                    if !lua53 {
                        check_format(l, form, FLAGS_C, true)?;
                    }
                    let spec = parse_spec(modifiers);
                    if spec.precision.is_none() && text.len() >= 100 {
                        out.extend_from_slice(&text);
                    } else {
                        let shown = &text[..spec.precision.unwrap_or(usize::MAX).min(text.len())];
                        numfmt::push_padded_bytes(&mut out, shown, &spec);
                    }
                }
            }
            _ => {
                let message = if lua53 {
                    format!("invalid option '%{}' to 'format'", printable_char(conv))
                } else {
                    format!("invalid conversion '%{}' to 'format'", String::from_utf8_lossy(form))
                };
                return Err(lauxlib::lual_error(l, message));
            }
        }
    }
    let result = l.create_bytes(&out)?;
    l.push_value(result)?;
    Ok(1)
}

/// Lua 5.3 `scanformat`: flags, two width digits, '.', two precision
/// digits. Returns the position of the conversion character.
fn scan_format_53(l: &mut LuaState, fmt: &[u8], start: usize) -> LuaResult<usize> {
    let at = |p: usize| fmt.get(p).copied().unwrap_or(0);
    let mut p = start;
    while FLAGS_F.contains(&at(p)) && at(p) != 0 {
        p += 1;
    }
    if p - start >= FLAGS_F.len() + 1 {
        return Err(lauxlib::lual_error(l, "invalid format (repeated flags)"));
    }
    p = skip_2digits(fmt, p);
    if at(p) == b'.' {
        p = skip_2digits(fmt, p + 1);
    }
    if at(p).is_ascii_digit() {
        return Err(lauxlib::lual_error(l, "invalid format (width or precision too long)"));
    }
    Ok(p)
}

/// Lua 5.5 `getformat`: span flags, digits and '.'; the next byte is the
/// conversion. Returns its position.
fn scan_format_55(l: &mut LuaState, fmt: &[u8], start: usize) -> LuaResult<usize> {
    let span = fmt[start..]
        .iter()
        .take_while(|&&b| FLAGS_F.contains(&b) || b.is_ascii_digit() || b == b'.')
        .count();
    if span + 1 >= MAX_FORMAT - 10 {
        return Err(lauxlib::lual_error(l, "invalid format string to 'format'"));
    }
    Ok(start + span)
}

/// Lua 5.5 `checkformat`: only `flags`, a width that does not start with
/// '0', and (if `precision`) a precision, each of at most two digits.
fn check_format(l: &mut LuaState, form: &[u8], flags: &[u8], precision: bool) -> LuaResult<()> {
    let at = |p: usize| form.get(p).copied().unwrap_or(0);
    let mut p = form.iter().take_while(|b| flags.contains(b)).count();
    if at(p) != b'0' {
        p = skip_2digits(form, p);
        if at(p) == b'.' && precision {
            p = skip_2digits(form, p + 1);
        }
    }
    if !at(p).is_ascii_alphabetic() {
        let form = String::from_utf8_lossy(form);
        return Err(lauxlib::lual_error(l, format!("invalid conversion specification: '%{form}'")));
    }
    Ok(())
}

fn skip_2digits(s: &[u8], mut p: usize) -> usize {
    for _ in 0..2 {
        if s.get(p).is_some_and(u8::is_ascii_digit) {
            p += 1;
        }
    }
    p
}

/// Flags, width and precision of an already validated specification.
fn parse_spec(modifiers: &[u8]) -> Spec {
    let mut spec = Spec::default();
    let mut p = 0;
    while p < modifiers.len() {
        match modifiers[p] {
            b'-' => spec.left = true,
            b'+' => spec.plus = true,
            b' ' => spec.space = true,
            b'#' => spec.alt = true,
            b'0' => spec.zero = true,
            _ => break,
        }
        p += 1;
    }
    while p < modifiers.len() && modifiers[p].is_ascii_digit() {
        spec.width = spec.width * 10 + usize::from(modifiers[p] - b'0');
        p += 1;
    }
    if p < modifiers.len() && modifiers[p] == b'.' {
        p += 1;
        let mut precision = 0;
        while p < modifiers.len() && modifiers[p].is_ascii_digit() {
            precision = precision * 10 + usize::from(modifiers[p] - b'0');
            p += 1;
        }
        spec.precision = Some(precision);
    }
    spec
}

/// `luaO_pushfstring`'s `%c`: printable characters as-is, others as `<\N>`.
fn printable_char(c: u8) -> String {
    if (b' '..=b'~').contains(&c) {
        char::from(c).to_string()
    } else {
        format!("<\\{c}>")
    }
}

/// `lua_topointer`; `None` for values without an address.
fn pointer_of(value: &LuaValue) -> Option<usize> {
    match value.kind() {
        LuaValueKind::String
        | LuaValueKind::Table
        | LuaValueKind::Function
        | LuaValueKind::CFunction
        | LuaValueKind::CClosure
        | LuaValueKind::RClosure
        | LuaValueKind::Userdata
        | LuaValueKind::Thread => Some(value.raw_ptr_repr() as usize),
        _ => None,
    }
}

/// `%q`: `addliteral` from lstrlib.c.
fn add_literal(l: &mut LuaState, out: &mut Vec<u8>, arg: usize, value: &LuaValue) -> LuaResult<()> {
    match value.kind() {
        LuaValueKind::String => {
            add_quoted(out, value.as_bytes().unwrap_or_default());
        }
        LuaValueKind::Integer => {
            let n = value.ivalue();
            if n == i64::MIN {
                out.extend_from_slice(b"0x8000000000000000");
            } else {
                out.extend_from_slice(itoa::Buffer::new().format(n).as_bytes());
            }
        }
        LuaValueKind::Float => {
            let n = value.as_float().unwrap_or_default();
            let lua53 = l.global_state().language() == LuaLanguageLevel::Lua53;
            if !lua53 && n == f64::INFINITY {
                out.extend_from_slice(b"1e9999");
            } else if !lua53 && n == f64::NEG_INFINITY {
                out.extend_from_slice(b"-1e9999");
            } else if !lua53 && n.is_nan() {
                out.extend_from_slice(b"(0/0)");
            } else {
                numfmt::push_float(out, n, b'a', &Spec::default());
            }
        }
        LuaValueKind::Nil => out.extend_from_slice(b"nil"),
        LuaValueKind::Boolean => {
            out.extend_from_slice(if value.as_boolean() == Some(true) { b"true" } else { b"false" })
        }
        _ => return Err(lauxlib::argerror(l, arg, "value has no literal form")),
    }
    Ok(())
}

/// `addquoted`: escape quotes, backslashes, newlines and C control
/// characters (`\ddd` when a digit follows).
fn add_quoted(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(b'"');
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' | b'\\' | b'\n' => {
                out.push(b'\\');
                out.push(b);
            }
            0..=31 | 127 => {
                out.push(b'\\');
                let mut buffer = itoa::Buffer::new();
                let digits = buffer.format(b);
                if bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
                    out.resize(out.len() + 3 - digits.len(), b'0');
                }
                out.extend_from_slice(digits.as_bytes());
            }
            _ => out.push(b),
        }
    }
    out.push(b'"');
}
