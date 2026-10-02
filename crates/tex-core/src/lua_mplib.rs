//! The `mplib` library (luatex `lmplib.c`) over the pure-Rust MetaPost
//! interpreter of `tex-mplib`.
//!
//! The Lua surface is LuaTeX's: `mplib.new(options)` returns an `MPlib.meta`
//! userdata whose methods (`execute`, `finish`, `statistics`, `get_numeric`,
//! `get_number`, `get_boolean`, `get_string`, `get_path`, `solve_path`,
//! `char_width`, `char_height`, `char_depth`) are also members of the `mplib`
//! table, next to `version`, `new`, `fields` and `pen_info`. `execute` returns
//! a table with `status`, `log`, `term` and `fig`; a figure is an `MPlib.fig`
//! userdata and its graphical objects are `MPlib.gr` userdata whose fields
//! (`mplib.fields`) are read through `__index`.
//!
//! What the MetaPost language itself can do is bounded by `tex-mplib`.

use std::cell::RefCell;
use std::rc::Rc;

use tex_lua::{
    CallbackLua, Lua, LuaApi, LuaFunction, LuaResult, LuaString, LuaTable, LuaValueKind, UserDataTrait, Value,
};
use tex_mplib::{Color, Interpreter, KnotSide, KnotSpec, MpFigure, MpObject, Pair, Path};

const BANNER_LOG: &str = "This is MetaPost, Version 3.00  14 NOV 2023 22:13";
const BANNER_TERM: &str = "This is MetaPost, Version 3.00";
/// MetaPost's `infinity` bounds of an empty picture (`2^15 - 2^-16`).
const EMPTY_BOUND: f64 = 32767.999984741211;

/// History values of an mplib instance (`mp_history_state`).
const WARNING_ISSUED: i64 = 1;
const ERROR_MESSAGE_ISSUED: i64 = 2;
const FATAL_ERROR_STOP: i64 = 3;

// ---------------------------------------------------------------- instance

struct Instance {
    interp: Interpreter,
    job_name: String,
    find_file: Option<LuaFunction>,
    /// `interaction = "batch"`: an error aborts the run.
    batch: bool,
    /// The banner has been written to the log/terminal.
    started: bool,
    log_opened: bool,
    history: i64,
    /// `end` has been executed or the run was aborted.
    stopped: bool,
    finished: bool,
}

struct MpUd(Rc<RefCell<Instance>>);

impl UserDataTrait for MpUd {
    fn type_name(&self) -> &'static str {
        "MPlib.meta"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// A figure: its objects are handed out by `objects()` once and copied by
/// `copy_objects()`.
struct FigUd {
    figure: Rc<MpFigure>,
    job_name: String,
    body: RefCell<Option<Rc<Vec<MpObject>>>>,
}

impl UserDataTrait for FigUd {
    fn type_name(&self) -> &'static str {
        "MPlib.fig"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

struct GrUd(Rc<MpObject>);

impl UserDataTrait for GrUd {
    fn type_name(&self) -> &'static str {
        "MPlib.gr"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// The metatables the userdata are created with.
#[derive(Clone)]
struct Metas {
    mp: LuaTable,
    fig: LuaTable,
    gr: LuaTable,
}

fn err<E: std::fmt::Debug>(e: E) -> String {
    format!("mplib: {e:?}")
}

fn float_table(cx: &mut CallbackLua<'_>, values: &[f64]) -> LuaResult<LuaTable> {
    let t = cx.create_table_with_capacity(values.len(), 0)?;
    for (i, v) in values.iter().enumerate() {
        t.raw_seti(i as i64 + 1, *v)?;
    }
    Ok(t)
}

fn ud_arg<T: 'static>(cx: &mut CallbackLua<'_>, index: usize, expected: &str) -> LuaResult<tex_lua::UserDataRef<T>> {
    let value: Value = cx.arg(index)?;
    match value.as_userdata::<T>() {
        Some(ud) => Ok(ud),
        None => Err(cx.type_error(index, expected)),
    }
}

fn instance_arg(cx: &mut CallbackLua<'_>) -> LuaResult<Rc<RefCell<Instance>>> {
    let ud = ud_arg::<MpUd>(cx, 1, "MPlib.meta")?;
    let rc = ud.borrow()?.0.clone();
    Ok(rc)
}

/// The text of a string or number argument (`lua_isstring`).
fn string_arg(cx: &mut CallbackLua<'_>, index: usize) -> Option<Vec<u8>> {
    match cx.arg_kind(index) {
        Some(LuaValueKind::String) => cx.arg::<LuaString>(index).ok().map(|s| s.to_bytes()),
        Some(LuaValueKind::Integer | LuaValueKind::Float) => {
            cx.arg::<Value>(index).ok().map(|v| v.to_string_lossy().into_bytes())
        }
        _ => None,
    }
}

fn warn(text: &str) {
    let _ = crate::lua_bridge::with_engine(|e| e.lua_texio_print(3, true, text.as_bytes()));
}

// ------------------------------------------------------------------- paths

/// A path as a list of `(x, y, left_x, left_y, right_x, right_y)` with the
/// controls of straight segments filled in as MetaPost does for `--`.
fn knot_controls(path: &Path) -> Vec<[f64; 6]> {
    let n = path.knots.len();
    let mut out: Vec<[f64; 6]> = path
        .knots
        .iter()
        .map(|k| [k.p.x, k.p.y, k.left_control.x, k.left_control.y, k.right_control.x, k.right_control.y])
        .collect();
    let segments = if path.closed { n } else { n.saturating_sub(1) };
    for i in 0..segments {
        let j = (i + 1) % n;
        let (a, b) = (path.knots[i], path.knots[j]);
        if a.right_control == a.p && b.left_control == b.p && a.p != b.p {
            let (dx, dy) = ((b.p.x - a.p.x) / 3.0, (b.p.y - a.p.y) / 3.0);
            out[i][4] = a.p.x + dx;
            out[i][5] = a.p.y + dy;
            out[j][2] = b.p.x - dx;
            out[j][3] = b.p.y - dy;
        }
    }
    if !path.closed && n > 0 {
        out[0][2] = path.knots[0].p.x;
        out[0][3] = path.knots[0].p.y;
        out[n - 1][4] = path.knots[n - 1].p.x;
        out[n - 1][5] = path.knots[n - 1].p.y;
    }
    out
}

/// The knot tables of `object.path`.
fn path_table(cx: &mut CallbackLua<'_>, path: &Path) -> LuaResult<LuaTable> {
    let controls = knot_controls(path);
    let n = controls.len();
    let t = cx.create_table_with_capacity(n, 0)?;
    for (i, c) in controls.iter().enumerate() {
        let k = cx.create_table()?;
        if !path.closed {
            if i == 0 {
                k.set("left_type", "endpoint")?;
            }
            if i == n - 1 {
                k.set("right_type", "endpoint")?;
            }
        }
        k.set("left_x", c[2])?;
        k.set("left_y", c[3])?;
        k.set("right_x", c[4])?;
        k.set("right_y", c[5])?;
        k.set("x_coord", c[0])?;
        k.set("y_coord", c[1])?;
        t.raw_seti(i as i64 + 1, k)?;
    }
    Ok(t)
}

fn pen_table(cx: &mut CallbackLua<'_>, width: f64) -> LuaResult<LuaTable> {
    let t = cx.create_table()?;
    let k = cx.create_table()?;
    k.set("left_x", width)?;
    k.set("left_y", 0.0)?;
    k.set("right_x", 0.0)?;
    k.set("right_y", width)?;
    k.set("x_coord", 0.0)?;
    k.set("y_coord", 0.0)?;
    t.raw_seti(1, k)?;
    t.set("type", "elliptical")?;
    Ok(t)
}

fn color_values(color: &Color) -> Vec<f64> {
    match *color {
        Color::None => Vec::new(),
        Color::Gray(g) => vec![g, g, g],
        Color::Rgb(r, g, b) => vec![r, g, b],
        Color::Cmyk(c, m, y, k) => vec![c, m, y, k],
    }
}

// ----------------------------------------------------------------- objects

fn object_fields(object: &MpObject) -> &'static [&'static str] {
    match object {
        MpObject::Fill { .. } => {
            &["type", "path", "htap", "pen", "color", "linejoin", "miterlimit", "prescript", "postscript"]
        }
        MpObject::Stroke { .. } => {
            &["type", "path", "pen", "color", "linejoin", "miterlimit", "linecap", "dash", "prescript", "postscript"]
        }
        MpObject::Text { .. } => {
            &["type", "text", "dsize", "font", "color", "width", "height", "depth", "transform", "prescript", "postscript"]
        }
        MpObject::StartClip { .. } => &["type", "path"],
        MpObject::StopClip => &["type"],
    }
}

/// `object.<key>`: nil for everything the object type does not have.
fn object_field(cx: &mut CallbackLua<'_>, object: &MpObject, key: &str) -> LuaResult<Value> {
    let t = |cx: &mut CallbackLua<'_>, v: LuaTable| cx.pack(&v);
    Ok(match (object, key) {
        (MpObject::Fill { .. }, "type") => cx.pack("fill")?,
        (MpObject::Stroke { .. }, "type") => cx.pack("outline")?,
        (MpObject::Text { .. }, "type") => cx.pack("text")?,
        (MpObject::StartClip { .. }, "type") => cx.pack("start_clip")?,
        (MpObject::StopClip, "type") => cx.pack("stop_clip")?,
        (MpObject::Fill { path, .. } | MpObject::Stroke { path, .. } | MpObject::StartClip { path }, "path") => {
            let table = path_table(cx, path)?;
            t(cx, table)?
        }
        (MpObject::Fill { color, .. } | MpObject::Stroke { color, .. } | MpObject::Text { color, .. }, "color") => {
            let table = float_table(cx, &color_values(color))?;
            t(cx, table)?
        }
        (MpObject::Stroke { width, .. }, "pen") => {
            let table = pen_table(cx, *width)?;
            t(cx, table)?
        }
        (MpObject::Fill { .. }, "linejoin") => cx.pack(1.0_f64)?,
        (MpObject::Fill { .. }, "miterlimit") => cx.pack(10.0_f64)?,
        (MpObject::Stroke { line_join, .. }, "linejoin") => cx.pack(f64::from(*line_join))?,
        (MpObject::Stroke { miter_limit, .. }, "miterlimit") => cx.pack(*miter_limit)?,
        (MpObject::Stroke { line_cap, .. }, "linecap") => cx.pack(f64::from(*line_cap))?,
        (MpObject::Stroke { dash: Some(dash), .. }, "dash") => {
            let table = cx.create_table()?;
            let dashes = float_table(cx, &dash.pattern)?;
            table.set("dashes", dashes)?;
            table.set("offset", dash.offset)?;
            t(cx, table)?
        }
        (MpObject::Text { text, .. }, "text") => cx.pack(text.as_str())?,
        (MpObject::Text { font, .. }, "font") => cx.pack(font.as_str())?,
        (MpObject::Text { .. }, "dsize" | "width" | "height" | "depth") => cx.pack(0.0_f64)?,
        (MpObject::Text { transform: m, .. }, "transform") => {
            // tx, ty, txx, tyx, txy, tyy
            let table = float_table(cx, &[m.x0, m.y0, m.xx, m.yx, m.xy, m.yy])?;
            t(cx, table)?
        }
        _ => cx.pack(())?,
    })
}

fn new_object(cx: &mut CallbackLua<'_>, metas: &Metas, object: Rc<MpObject>) -> LuaResult<Value> {
    let ud = cx.create_userdata(GrUd(object))?;
    let value = cx.pack(&ud)?;
    value.set_metatable(Some(&metas.gr))?;
    Ok(value)
}

// ----------------------------------------------------------------- figures

fn new_figure(cx: &mut CallbackLua<'_>, metas: &Metas, figure: MpFigure, job_name: &str) -> LuaResult<Value> {
    let body = Rc::new(figure.objects.clone());
    let ud = cx.create_userdata(FigUd { figure: Rc::new(figure), job_name: job_name.to_string(), body: RefCell::new(Some(body)) })?;
    let value = cx.pack(&ud)?;
    value.set_metatable(Some(&metas.fig))?;
    Ok(value)
}

fn figure_arg(cx: &mut CallbackLua<'_>) -> LuaResult<tex_lua::UserDataRef<FigUd>> {
    ud_arg::<FigUd>(cx, 1, "MPlib.fig")
}

fn figure_bounds(figure: &MpFigure) -> [f64; 4] {
    if figure.objects.is_empty() {
        [EMPTY_BOUND, EMPTY_BOUND, -EMPTY_BOUND, -EMPTY_BOUND]
    } else {
        let (a, b, c, d) = figure.bounding_box;
        [a, b, c, d]
    }
}

fn figure_filename(figure: &MpFigure, job_name: &str) -> String {
    if figure.charcode < 0 {
        format!("{job_name}.ps")
    } else {
        format!("{job_name}.{}", figure.charcode)
    }
}

// --------------------------------------------------------------- execution

/// The Lua `find_file(name, mode, type)` of the instance.
fn lookup_file(f: &LuaFunction, name: &str, mode: &str, kind: &str) -> Result<Option<String>, String> {
    let found: Value = f.call((name.to_string(), mode.to_string(), kind.to_string())).map_err(|e| format!("{e:?}"))?;
    Ok(found.as_string())
}

struct Output {
    status: i64,
    log: String,
    term: String,
    figures: Vec<MpFigure>,
}

/// Run `code` on the instance and collect the outcome of this call.
fn run_code(inst: &mut Instance, code: &str) -> Output {
    let mut log = String::new();
    let mut term = String::new();
    if !inst.started {
        inst.started = true;
        log.push_str(BANNER_LOG);
        log.push('\n');
        term.push_str(BANNER_TERM);
        term.push_str("\n\n");
    }
    let find_file = inst.find_file.clone();
    inst.interp.input_resolver = Some(Box::new(move |name| {
        let path = match &find_file {
            Some(f) => lookup_file(f, name, "r", "mp").ok().flatten()?,
            None => name.to_string(),
        };
        std::fs::read_to_string(path).ok()
    }));
    let result = inst.interp.run(code);
    inst.interp.input_resolver = None;
    log.push_str(&std::mem::take(&mut inst.interp.log));
    term.push_str(&std::mem::take(&mut inst.interp.term));
    let figures = std::mem::take(&mut inst.interp.figures);
    let context = code.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim_end();
    if let Err(message) = result {
        let message = message.trim_end_matches('.').to_string();
        let text = if inst.interp.fatal {
            inst.history = FATAL_ERROR_STOP;
            format!(
                "! {message}.\n<*> {context}\nPlease type another input file name\n! Emergency stop.\n<*> {context}\n*** (job aborted, file error in nonstop mode)\n\n\n"
            )
        } else {
            inst.history = inst.history.max(ERROR_MESSAGE_ISSUED);
            if inst.batch {
                inst.history = FATAL_ERROR_STOP;
            }
            format!("! {message}.\n<*> {context}\n\n")
        };
        log.push_str(&text);
        term.push_str(&text);
    }
    if inst.interp.ended || inst.history == FATAL_ERROR_STOP {
        inst.stopped = true;
    }
    Output { status: inst.history, log, term, figures }
}

fn result_table(cx: &mut CallbackLua<'_>, metas: &Metas, job_name: &str, out: Output) -> LuaResult<LuaTable> {
    let t = cx.create_table()?;
    if !out.log.is_empty() {
        t.set("log", out.log.as_str())?;
    }
    if !out.term.is_empty() {
        t.set("term", out.term.as_str())?;
    }
    t.set("status", out.status)?;
    if !out.figures.is_empty() {
        let figs = cx.create_table_with_capacity(out.figures.len(), 0)?;
        for (i, figure) in out.figures.into_iter().enumerate() {
            let value = new_figure(cx, metas, figure, job_name)?;
            figs.raw_seti(i as i64 + 1, value)?;
        }
        t.set("fig", figs)?;
    }
    Ok(t)
}

fn busy(cx: &mut CallbackLua<'_>) -> tex_lua::LuaError {
    cx.error("mplib instance is busy")
}

fn mp_execute(cx: &mut CallbackLua<'_>, metas: &Metas) -> LuaResult<usize> {
    let rc = instance_arg(cx)?;
    let Some(code) = string_arg(cx, 2) else {
        return cx.push(Option::<i64>::None);
    };
    let code = String::from_utf8_lossy(&code).into_owned();
    let Ok(mut inst) = rc.try_borrow_mut() else {
        return Err(busy(cx));
    };
    if inst.finished {
        return cx.push(Option::<i64>::None);
    }
    let job_name = inst.job_name.clone();
    if inst.stopped {
        let t = cx.create_table()?;
        t.set("status", inst.history)?;
        return cx.push(t);
    }
    if !inst.log_opened {
        inst.log_opened = true;
        if let Some(f) = inst.find_file.clone() {
            let log_name = format!("{job_name}.log");
            let opened = lookup_file(&f, &log_name, "w", "log").map_err(|e| cx.error(e))?;
            if opened.is_none() {
                inst.started = true;
                inst.stopped = true;
                inst.history = FATAL_ERROR_STOP;
                let t = cx.create_table()?;
                t.set(
                    "term",
                    format!(
                        "{BANNER_TERM}\n! I can't write on file `{log_name}'.\nPlease type another transcript file name\n"
                    ),
                )?;
                t.set("status", FATAL_ERROR_STOP)?;
                return cx.push(t);
            }
        }
    }
    let out = run_code(&mut inst, &code);
    drop(inst);
    let t = result_table(cx, metas, &job_name, out)?;
    cx.push(t)
}

fn mp_finish(cx: &mut CallbackLua<'_>) -> LuaResult<usize> {
    let rc = instance_arg(cx)?;
    let Ok(mut inst) = rc.try_borrow_mut() else {
        return Err(busy(cx));
    };
    if inst.finished {
        return cx.push(Option::<i64>::None);
    }
    inst.finished = true;
    let t = cx.create_table()?;
    if !inst.started {
        t.set("log", BANNER_LOG)?;
    } else if !inst.stopped {
        t.set("log", "\n")?;
        t.set("term", "\n")?;
    }
    t.set("status", inst.history)?;
    cx.push(t)
}

fn mp_statistics(cx: &mut CallbackLua<'_>) -> LuaResult<usize> {
    let rc = instance_arg(cx)?;
    let inst = rc.borrow();
    if inst.finished {
        return cx.push(Option::<i64>::None);
    }
    let t = cx.create_table()?;
    t.set("hash", inst.interp.vars.len() as i64)?;
    t.set("memory", std::mem::size_of::<MpObject>() as i64 * inst.interp.current_objects.len() as i64)?;
    t.set("open", 0_i64)?;
    t.set("params", 0_i64)?;
    cx.push(t)
}

/// The variable name argument of `get_*`: absent or non-string means "".
fn variable_name(cx: &mut CallbackLua<'_>) -> String {
    string_arg(cx, 2).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
}

fn mp_get(cx: &mut CallbackLua<'_>, kind: &str) -> LuaResult<usize> {
    let rc = instance_arg(cx)?;
    let name = variable_name(cx);
    let inst = rc.borrow();
    if inst.finished {
        return cx.push(Option::<i64>::None);
    }
    match kind {
        "numeric" => cx.push(inst.interp.numeric_variable(&name).unwrap_or(0.0)),
        // tex-mplib has no boolean values: every boolean reads as false
        "boolean" => cx.push(false),
        "string" => cx.push(inst.interp.string_variable(&name).unwrap_or("").to_string()),
        _ => match inst.interp.path_variable(&name) {
            None => Ok(0),
            Some(path) => {
                let controls = knot_controls(path);
                let single = controls.len() == 1;
                let t = cx.create_table_with_capacity(controls.len(), 1)?;
                for (i, c) in controls.iter().enumerate() {
                    let row = if single { [c[0], c[1], 0.0, 0.0, 0.0, 0.0] } else { *c };
                    let k = float_table(cx, &row)?;
                    t.raw_seti(i as i64 + 1, k)?;
                }
                t.set("cycle", path.closed)?;
                cx.push(t)
            }
        },
    }
}

fn mp_char_dimension(cx: &mut CallbackLua<'_>) -> LuaResult<usize> {
    let rc = instance_arg(cx)?;
    if string_arg(cx, 2).is_none() {
        return Err(cx.type_error(2, "string"));
    }
    if rc.borrow().finished {
        return cx.push(Option::<i64>::None);
    }
    // No TFM is ever loaded by the interpreter, so every character is unknown.
    cx.push(0.0_f64)
}

// ------------------------------------------------------------- solve_path

fn number_field(t: &LuaTable, key: &str) -> Option<f64> {
    match t.get::<Value>(key).ok()? {
        v if v.kind() == LuaValueKind::Integer || v.kind() == LuaValueKind::Float => v.as_number(),
        _ => None,
    }
}

fn side_of(t: &LuaTable, side: &str) -> Result<(String, KnotSide), &'static str> {
    let kind: Option<String> = t.get::<Value>(format!("{side}_type")).ok().and_then(|v| v.as_string());
    let kind = kind.unwrap_or_else(|| "open".to_string());
    let constraint = match kind.as_str() {
        "open" | "endpoint" => KnotSide::Open,
        "curl" => KnotSide::Curl(number_field(t, &format!("{side}_curl")).unwrap_or(1.0)),
        "given" => KnotSide::Given(number_field(t, &format!("{side}_given")).unwrap_or(0.0)),
        "explicit" => KnotSide::Open,
        _ => return Err("Wrong argument types"),
    };
    Ok((kind, constraint))
}

fn mp_solve_path(cx: &mut CallbackLua<'_>) -> LuaResult<usize> {
    let rc = instance_arg(cx)?;
    if rc.borrow().finished {
        return cx.push(Option::<i64>::None);
    }
    fn fail(cx: &mut CallbackLua<'_>, message: &str) -> LuaResult<usize> {
        cx.push((false, message.to_string()))
    }
    if cx.arg_count() != 3 {
        return fail(cx, "Wrong number of arguments");
    }
    let (Some(LuaValueKind::Table), Some(LuaValueKind::Boolean)) = (cx.arg_kind(2), cx.arg_kind(3)) else {
        return fail(cx, "Wrong argument types");
    };
    let knots: LuaTable = cx.arg(2)?;
    let closed: bool = cx.arg(3)?;
    let n = knots.raw_len()?;
    let mut tables = Vec::with_capacity(n);
    let mut specs = Vec::with_capacity(n);
    let mut all_explicit = true;
    let mut any_explicit = false;
    for i in 1..=n {
        let Some(k) = knots.raw_geti::<Value>(i as i64)?.as_table() else {
            return fail(cx, "Wrong argument types");
        };
        let (Some(x), Some(y)) = (number_field(&k, "x_coord"), number_field(&k, "y_coord")) else {
            return fail(cx, "Wrong argument types");
        };
        let (lk, left) = match side_of(&k, "left") {
            Ok(s) => s,
            Err(m) => return fail(cx, m),
        };
        let (rk, right) = match side_of(&k, "right") {
            Ok(s) => s,
            Err(m) => return fail(cx, m),
        };
        let explicit = lk == "explicit" || rk == "explicit";
        any_explicit |= explicit;
        all_explicit &= lk == "explicit" && rk == "explicit";
        let mut spec = KnotSpec::new(Pair::new(x, y));
        spec.left = left;
        spec.right = right;
        spec.left_tension = number_field(&k, "left_tension").unwrap_or(1.0);
        spec.right_tension = number_field(&k, "right_tension").unwrap_or(1.0);
        specs.push(spec);
        tables.push(k);
    }
    if n == 0 {
        return fail(cx, "Wrong argument types");
    }
    if any_explicit {
        if !all_explicit {
            return fail(cx, "Mixing explicit and implicit knots is not supported");
        }
        // a path that is already solved stays as it is
        return cx.push(true);
    }
    let solved = if n > 1 { tex_mplib::solve_path(&specs, closed) } else { Path { knots: vec![tex_mplib::Knot::new(specs[0].p)], closed } };
    let controls = knot_controls(&solved);
    for (i, (k, c)) in tables.iter().zip(controls.iter()).enumerate() {
        let (left, right) = if closed {
            ("explicit", "explicit")
        } else {
            (if i == 0 { "endpoint" } else { "explicit" }, if i == n - 1 { "endpoint" } else { "explicit" })
        };
        k.set("left_type", left)?;
        k.set("left_x", c[2])?;
        k.set("left_y", c[3])?;
        k.set("right_type", right)?;
        k.set("right_x", c[4])?;
        k.set("right_y", c[5])?;
    }
    cx.push(true)
}

// ------------------------------------------------------------------ module

fn tostring_of(kind: &'static str) -> impl Fn(&mut CallbackLua<'_>) -> LuaResult<usize> {
    move |cx| {
        let value: Value = cx.arg(1)?;
        let address = value.to_pointer().map_or(0, |p| p as usize);
        cx.push(format!("<{kind} {address:#x}>"))
    }
}

fn new_mplib(cx: &mut CallbackLua<'_>, metas: &Metas) -> LuaResult<usize> {
    let mut job_name = "mpout".to_string();
    let mut find_file = None;
    let mut batch = false;
    if let Some(LuaValueKind::Table) = cx.arg_kind(1) {
        let options: LuaTable = cx.arg(1)?;
        let pairs: Vec<(Value, Value)> = options.pairs()?;
        for (key, value) in pairs {
            let Some(key) = key.as_string() else { continue };
            let text = || match value.kind() {
                LuaValueKind::String | LuaValueKind::Integer | LuaValueKind::Float => value.to_string_lossy(),
                _ => String::new(),
            };
            match key.as_str() {
                "interaction" => {
                    let s = text();
                    if !["batch", "nonstop", "scroll", "errorstop"].contains(&s.as_str()) {
                        return Err(cx.arg_error_at(-1, &format!("invalid option '{s}'")));
                    }
                    batch = s == "batch";
                }
                "math_mode" => {
                    let s = text();
                    if !["scaled", "double", "binary", "decimal"].contains(&s.as_str()) {
                        return Err(cx.arg_error_at(-1, &format!("invalid option '{s}'")));
                    }
                }
                "job_name" => {
                    if matches!(value.kind(), LuaValueKind::String | LuaValueKind::Integer | LuaValueKind::Float) {
                        job_name = text();
                    }
                }
                "find_file" | "run_script" | "make_text" | "script_error" => {
                    match value.as_function() {
                        Some(f) if key == "find_file" => find_file = Some(f),
                        Some(_) => {}
                        None => warn(&format!("mplib warning: function expected for '{key}'\n")),
                    }
                }
                _ => {}
            }
        }
    }
    let instance = Instance {
        interp: Interpreter::new(),
        job_name,
        find_file,
        batch,
        started: false,
        log_opened: false,
        history: 0,
        stopped: false,
        finished: false,
    };
    let ud = cx.create_userdata(MpUd(Rc::new(RefCell::new(instance))))?;
    let value = cx.pack(&ud)?;
    value.set_metatable(Some(&metas.mp))?;
    cx.push(value)
}

macro_rules! method {
    ($lua:expr, $name:literal, [$($table:expr),+], $f:expr) => {{
        let f = $lua.create_callback($f).map_err(err)?;
        $( $table.set($name, &f).map_err(err)?; )+
    }};
}

pub(crate) fn install(lua: &mut Lua) -> Result<(), String> {
    let module: LuaTable = lua.create_table().map_err(err)?;
    let metas = Metas {
        mp: lua.create_table().map_err(err)?,
        fig: lua.create_table().map_err(err)?,
        gr: lua.create_table().map_err(err)?,
    };
    for (table, name) in [(&metas.mp, "MPlib.meta"), (&metas.fig, "MPlib.fig"), (&metas.gr, "MPlib.gr")] {
        table.set("__name", name).map_err(err)?;
        lua.registry_set(name, table).map_err(err)?;
    }
    let gc = lua.create_callback(|_cx| Ok(0)).map_err(err)?;
    for table in [&metas.mp, &metas.fig, &metas.gr] {
        table.set("__gc", &gc).map_err(err)?;
    }
    metas.mp.set("__index", &metas.mp).map_err(err)?;
    metas.fig.set("__index", &metas.fig).map_err(err)?;
    method!(lua, "__tostring", [metas.mp], tostring_of("MP"));
    method!(lua, "__tostring", [metas.fig], tostring_of("figure"));
    method!(lua, "__tostring", [metas.gr], tostring_of("object"));

    // ---- instance methods (members of the metatable and of the module)
    let (mp, md) = (&metas.mp, &module);
    {
        let metas = metas.clone();
        method!(lua, "execute", [mp, md], move |cx| mp_execute(cx, &metas));
    }
    method!(lua, "finish", [mp, md], mp_finish);
    method!(lua, "statistics", [mp, md], mp_statistics);
    method!(lua, "get_numeric", [mp, md], |cx| mp_get(cx, "numeric"));
    method!(lua, "get_number", [mp, md], |cx| mp_get(cx, "numeric"));
    method!(lua, "get_boolean", [mp, md], |cx| mp_get(cx, "boolean"));
    method!(lua, "get_string", [mp, md], |cx| mp_get(cx, "string"));
    method!(lua, "get_path", [mp, md], |cx| mp_get(cx, "path"));
    method!(lua, "solve_path", [mp, md], mp_solve_path);
    method!(lua, "char_width", [mp, md], mp_char_dimension);
    method!(lua, "char_height", [mp, md], mp_char_dimension);
    method!(lua, "char_depth", [mp, md], mp_char_dimension);

    // ---- module functions
    method!(lua, "version", [md], |cx| cx.push("3.00"));
    {
        let metas = metas.clone();
        method!(lua, "new", [md], move |cx| new_mplib(cx, &metas));
    }
    method!(lua, "fields", [md], |cx| {
        let ud = ud_arg::<GrUd>(cx, 1, "MPlib.gr")?;
        let object = ud.borrow()?.0.clone();
        let t = cx.create_table()?;
        for (i, name) in object_fields(&object).iter().enumerate() {
            t.raw_seti(i as i64 + 1, *name)?;
        }
        cx.push(t)
    });
    method!(lua, "pen_info", [md], |cx| {
        let top = cx.arg_count();
        if top == 0 {
            return Err(cx.arg_error_at(-1, "MPlib.gr expected, got function"));
        }
        let value: Value = cx.arg(top)?;
        let Some(ud) = value.as_userdata::<GrUd>() else {
            let got = value.type_name();
            return Err(cx.arg_error_at(-1, &format!("MPlib.gr expected, got {got}")));
        };
        let object = ud.borrow()?.0.clone();
        match &*object {
            MpObject::Stroke { width, .. } => {
                let t = cx.create_table()?;
                for (k, v) in [("rx", 0.0), ("ry", 0.0), ("sx", 1.0), ("sy", 1.0), ("tx", 0.0), ("ty", 0.0), ("width", *width)] {
                    t.set(k, v)?;
                }
                cx.push(t)
            }
            _ => cx.push(Option::<i64>::None),
        }
    });

    // ---- figures
    let fig = &metas.fig;
    method!(lua, "boundingbox", [fig], |cx| {
        let f = figure_arg(cx)?;
        let bounds = figure_bounds(&f.borrow()?.figure);
        let t = float_table(cx, &bounds)?;
        cx.push(t)
    });
    method!(lua, "charcode", [fig], |cx| {
        let f = figure_arg(cx)?;
        let code = f64::from(f.borrow()?.figure.charcode);
        cx.push(code)
    });
    method!(lua, "filename", [fig], |cx| {
        let f = figure_arg(cx)?;
        let f = f.borrow()?;
        cx.push(figure_filename(&f.figure, &f.job_name))
    });
    // The character dimensions of a figure are the `charwd`/`charht`/`chardp`/
    // `charic` internals, which the interpreter does not have.
    method!(lua, "width", [fig], |cx| {
        figure_arg(cx)?;
        cx.push(0.0_f64)
    });
    method!(lua, "height", [fig], |cx| {
        figure_arg(cx)?;
        cx.push(0.0_f64)
    });
    method!(lua, "depth", [fig], |cx| {
        figure_arg(cx)?;
        cx.push(0.0_f64)
    });
    method!(lua, "italcorr", [fig], |cx| {
        figure_arg(cx)?;
        cx.push(0.0_f64)
    });
    {
        let metas = metas.clone();
        method!(lua, "objects", [fig], move |cx| {
            let f = figure_arg(cx)?;
            let body = f.borrow()?.body.borrow_mut().take();
            let t = cx.create_table()?;
            if let Some(body) = body {
                for (i, object) in body.iter().enumerate() {
                    let value = new_object(cx, &metas, Rc::new(object.clone()))?;
                    t.raw_seti(i as i64 + 1, value)?;
                }
            }
            cx.push(t)
        });
    }
    {
        let metas = metas.clone();
        method!(lua, "copy_objects", [fig], move |cx| {
            let f = figure_arg(cx)?;
            let body = f.borrow()?.body.borrow().clone();
            let t = cx.create_table()?;
            if let Some(body) = body {
                for (i, object) in body.iter().enumerate() {
                    let value = new_object(cx, &metas, Rc::new(object.clone()))?;
                    t.raw_seti(i as i64 + 1, value)?;
                }
            }
            cx.push(t)
        });
    }
    method!(lua, "postscript", [fig], |cx| {
        let f = figure_arg(cx)?;
        for i in 2..=cx.arg_count() {
            if !matches!(cx.arg_kind(i), Some(LuaValueKind::Integer | LuaValueKind::Float | LuaValueKind::Nil)) {
                return Err(cx.type_error(i, "number"));
            }
        }
        let ps = f.borrow()?.figure.to_postscript();
        cx.push(ps)
    });
    method!(lua, "svg", [fig], |cx| {
        figure_arg(cx)?;
        warn("\nwarning  (mplib): svg bakend not available.\n");
        cx.push(Option::<i64>::None)
    });
    method!(lua, "png", [fig], |cx| {
        figure_arg(cx)?;
        warn("\nwarning  (mplib): png backend not available.\n");
        cx.push(Option::<i64>::None)
    });

    // ---- objects: `object.<field>`
    method!(lua, "__index", [metas.gr], |cx| {
        let ud = ud_arg::<GrUd>(cx, 1, "MPlib.gr")?;
        let object = ud.borrow()?.0.clone();
        let key = match cx.arg_kind(2) {
            Some(LuaValueKind::String) => cx.arg::<String>(2)?,
            _ => return cx.push(Option::<i64>::None),
        };
        let value = object_field(cx, &object, &key)?;
        cx.push(value)
    });

    lua.set_global("mplib", &module).map_err(err)?;
    Ok(())
}
