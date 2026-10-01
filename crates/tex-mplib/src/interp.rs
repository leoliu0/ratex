//! MetaPost language lexer, parser and runtime interpreter.

use std::collections::HashMap;

use crate::curves::{solve_path, KnotSide, KnotSpec};
use crate::solver::{fmt_number, LinearExpr, LinearSolver};
use crate::types::{Color, Dash, MpFigure, MpObject, Pair, Path, Pen, Transform};

/// Maximum nesting of parenthesized expressions, directions and `for` loops.
const MAX_DEPTH: usize = 100;
/// Maximum number of statements and loop iterations executed per run.
const MAX_STEPS: usize = 1_000_000;
/// Plain MetaPost's `infinity`, the tension of `---`.
const INFINITY_TENSION: f64 = 4095.99998;

/// Identifiers that are commands or operators, never variables.
const KEYWORDS: &[&str] = &[
    "and", "atleast", "beginfig", "boolean", "bye", "clip", "color", "controls", "curl", "cycle",
    "dashed", "dir", "downto", "draw", "drawdot", "end", "endfig", "endfor", "fill", "filldraw",
    "for", "message", "numeric", "pair", "path", "pickup", "rotated", "scaled", "shifted",
    "slanted", "step", "string", "tension", "to", "transform", "until", "upto", "withcolor",
    "withpen", "xscaled", "yscaled",
];

/// Binary operators of secondary precedence that transform their left operand.
const TRANSFORMERS: &[&str] = &["scaled", "xscaled", "yscaled", "shifted", "rotated", "slanted"];

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Numeric(f64),
    Pair(Pair),
    Color(Color),
    Path(Path),
    Pen(Pen),
    String(String),
    /// Numeric depending linearly on unknowns of the `LinearSolver`.
    Linear(LinearExpr),
    /// Pair whose coordinates depend linearly on unknowns of the `LinearSolver`.
    LinearPair(LinearExpr, LinearExpr),
}

/// Tokenizer for MetaPost.
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Ident(String),
    Number(f64),
    String(String),
    Semi,
    Colon,
    Comma,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Equal,
    Assign, // :=
    Plus,
    Minus,
    Star,
    Slash,
    DotDot, // ..
    TripleDot, // ...
    DashDash, // --
    TripleDash, // ---
    Ampersand, // &
    Less,
    Greater,
    LessEqual,
    GreaterEqual,
    NotEqual,
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '%' {
            // Comment until newline
            while i < len && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }

        let single = match c {
            ';' => Some(Token::Semi),
            ',' => Some(Token::Comma),
            '(' => Some(Token::LParen),
            ')' => Some(Token::RParen),
            '[' => Some(Token::LBracket),
            ']' => Some(Token::RBracket),
            '{' => Some(Token::LBrace),
            '}' => Some(Token::RBrace),
            '=' => Some(Token::Equal),
            '*' => Some(Token::Star),
            '/' => Some(Token::Slash),
            '&' => Some(Token::Ampersand),
            _ => None,
        };
        if let Some(tok) = single {
            tokens.push(tok);
            i += 1;
            continue;
        }
        if c == ':' {
            if i + 1 < len && chars[i + 1] == '=' {
                tokens.push(Token::Assign);
                i += 2;
            } else {
                tokens.push(Token::Colon);
                i += 1;
            }
            continue;
        }
        if c == '+' || c == '-' {
            // `+` and `-` form one symbolic token per run, as in MetaPost.
            let start = i;
            while i < len && (chars[i] == '+' || chars[i] == '-') {
                i += 1;
            }
            let run: String = chars[start..i].iter().collect();
            tokens.push(match run.as_str() {
                "+" => Token::Plus,
                "-" => Token::Minus,
                "--" => Token::DashDash,
                "---" => Token::TripleDash,
                _ => return Err(format!("Unsupported operator `{run}`")),
            });
            continue;
        }
        if c == '<' {
            if i + 1 < len && chars[i + 1] == '=' {
                tokens.push(Token::LessEqual);
                i += 2;
            } else if i + 1 < len && chars[i + 1] == '>' {
                tokens.push(Token::NotEqual);
                i += 2;
            } else {
                tokens.push(Token::Less);
                i += 1;
            }
            continue;
        }
        if c == '>' {
            if i + 1 < len && chars[i + 1] == '=' {
                tokens.push(Token::GreaterEqual);
                i += 2;
            } else {
                tokens.push(Token::Greater);
                i += 1;
            }
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && i + 1 < len && chars[i + 1].is_ascii_digit()) {
            // digits [ '.' digits ]: a decimal point belongs to the number only
            // when a digit follows it.
            let start = i;
            while i < len && chars[i].is_ascii_digit() {
                i += 1;
            }
            if i + 1 < len && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                i += 1;
                while i < len && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let text: String = chars[start..i].iter().collect();
            let n = text
                .parse::<f64>()
                .map_err(|e| format!("Invalid number `{text}`: {e}"))?;
            if !n.is_finite() {
                return Err(format!("Number too large: {text}"));
            }
            tokens.push(Token::Number(n));
            continue;
        }
        if c == '.' {
            let start = i;
            while i < len && chars[i] == '.' {
                i += 1;
            }
            match i - start {
                // A lone period separates suffixes (`label.top`) and is ignored.
                1 => {}
                2 => tokens.push(Token::DotDot),
                3 => tokens.push(Token::TripleDot),
                n => return Err(format!("Unsupported token `{}`", ".".repeat(n))),
            }
            continue;
        }
        if c == '"' {
            i += 1;
            let start = i;
            while i < len && chars[i] != '"' && chars[i] != '\n' {
                i += 1;
            }
            if i >= len || chars[i] != '"' {
                return Err("Incomplete string token".into());
            }
            tokens.push(Token::String(chars[start..i].iter().collect()));
            i += 1;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            tokens.push(Token::Ident(chars[start..i].iter().collect()));
            continue;
        }

        i += 1;
    }

    Ok(tokens)
}

/// `x<digits>` / `y<digits>`: the coordinates saved by `clearxy` at `beginfig`.
fn is_xy_name(name: &str) -> bool {
    (name.starts_with('x') || name.starts_with('y'))
        && name.len() > 1
        && name[1..].bytes().all(|b| b.is_ascii_digit())
}

fn is_ident(tok: Option<&Token>, name: &str) -> bool {
    matches!(tok, Some(Token::Ident(id)) if id == name)
}

fn check_finite(e: LinearExpr) -> Result<LinearExpr, String> {
    if e.constant.is_finite() && e.terms.values().all(|c| c.is_finite()) {
        Ok(e)
    } else {
        Err("Arithmetic overflow".into())
    }
}

fn num_value(e: LinearExpr) -> Result<Value, String> {
    let e = check_finite(e)?;
    Ok(match e.as_known() {
        Some(c) => Value::Numeric(c),
        None => Value::Linear(e),
    })
}

fn pair_value(x: LinearExpr, y: LinearExpr) -> Result<Value, String> {
    let (x, y) = (check_finite(x)?, check_finite(y)?);
    Ok(match (x.as_known(), y.as_known()) {
        (Some(px), Some(py)) => Value::Pair(Pair::new(px, py)),
        _ => Value::LinearPair(x, y),
    })
}

fn path_value(p: Path) -> Result<Value, String> {
    let finite = |q: Pair| q.x.is_finite() && q.y.is_finite();
    if p.knots.iter().all(|k| finite(k.p) && finite(k.left_control) && finite(k.right_control)) {
        Ok(Value::Path(p))
    } else {
        Err("Arithmetic overflow".into())
    }
}

fn scale_color(c: Color, s: f64) -> Color {
    match c {
        Color::None => Color::None,
        Color::Gray(g) => Color::Gray(g * s),
        Color::Rgb(r, g, b) => Color::Rgb(r * s, g * s, b * s),
        Color::Cmyk(cc, m, y, k) => Color::Cmyk(cc * s, m * s, y * s, k * s),
    }
}

/// MetaPost execution state.
pub struct Interpreter {
    pub solver: LinearSolver,
    pub vars: HashMap<String, Value>,
    pub current_pen: Pen,
    pub current_color: Color,
    pub current_objects: Vec<MpObject>,
    pub figures: Vec<MpFigure>,
    pub current_fig: Option<i32>,
    pub log: String,
    pub term: String,
    depth: usize,
    steps: usize,
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

impl Interpreter {
    pub fn new() -> Self {
        let mut interp = Self {
            solver: LinearSolver::new(),
            vars: HashMap::new(),
            current_pen: Pen::default_pen(),
            current_color: Color::BLACK,
            current_objects: Vec::new(),
            figures: Vec::new(),
            current_fig: None,
            log: String::new(),
            term: String::new(),
            depth: 0,
            steps: 0,
        };
        interp.init_builtins();
        interp
    }

    fn init_builtins(&mut self) {
        // Builtin colors
        self.vars.insert("black".into(), Value::Color(Color::BLACK));
        self.vars.insert("white".into(), Value::Color(Color::WHITE));
        self.vars.insert("red".into(), Value::Color(Color::RED));
        self.vars.insert("green".into(), Value::Color(Color::GREEN));
        self.vars.insert("blue".into(), Value::Color(Color::BLUE));

        // Builtin pairs
        self.vars.insert("origin".into(), Value::Pair(Pair::ZERO));
        self.vars.insert("right".into(), Value::Pair(Pair::new(1.0, 0.0)));
        self.vars.insert("left".into(), Value::Pair(Pair::new(-1.0, 0.0)));
        self.vars.insert("up".into(), Value::Pair(Pair::new(0.0, 1.0)));
        self.vars.insert("down".into(), Value::Pair(Pair::new(0.0, -1.0)));

        // Builtin paths
        self.vars.insert(
            "fullcircle".into(),
            Value::Path(Path::circle(Pair::ZERO, 0.5)),
        );
        self.vars.insert(
            "unitsquare".into(),
            Value::Path(Path::rectangle(Pair::ZERO, Pair::new(1.0, 1.0))),
        );

        // Builtin units (in bp, 1/72 inch)
        self.vars.insert("bp".into(), Value::Numeric(1.0));
        self.vars.insert("pt".into(), Value::Numeric(72.0 / 72.27));
        self.vars.insert("in".into(), Value::Numeric(72.0));
        self.vars.insert("inch".into(), Value::Numeric(72.0));
        self.vars.insert("cm".into(), Value::Numeric(72.0 / 2.54));
        self.vars.insert("mm".into(), Value::Numeric(7.2 / 2.54));

        // Pens
        self.vars.insert("pencircle".into(), Value::Pen(Pen::circle(1.0)));
        self.vars.insert("currentpen".into(), Value::Pen(Pen::default_pen()));
    }

    pub fn run(&mut self, code: &str) -> Result<(), String> {
        let tokens = tokenize(code)?;
        let mut pos = 0;
        self.depth = 0;
        self.steps = 0;

        while pos < tokens.len() {
            self.parse_statement(&tokens, &mut pos)?;
        }

        Ok(())
    }

    fn step(&mut self) -> Result<(), String> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return Err(format!("Execution limit of {MAX_STEPS} statements exceeded"));
        }
        Ok(())
    }

    fn enter(&mut self) -> Result<(), String> {
        if self.depth >= MAX_DEPTH {
            return Err(format!("Nesting deeper than {MAX_DEPTH} levels"));
        }
        self.depth += 1;
        Ok(())
    }

    fn parse_statement(&mut self, tokens: &[Token], pos: &mut usize) -> Result<(), String> {
        if *pos >= tokens.len() {
            return Ok(());
        }
        self.step()?;

        if let Token::Semi = &tokens[*pos] {
            *pos += 1;
            return Ok(());
        }

        if let Token::Ident(name) = &tokens[*pos] {
            match name.as_str() {
                "beginfig" => {
                    *pos += 1;
                    let num = self.parse_known_numeric(tokens, pos)?;
                    self.current_fig = Some(num as i32);
                    self.current_objects.clear();
                    // plain.mp: `clearxy; pickup defaultpen`.
                    self.solver.name_to_id.retain(|n, _| !is_xy_name(n));
                    self.vars.retain(|n, _| !is_xy_name(n));
                    self.current_pen = Pen::default_pen();
                    self.expect_semi(tokens, pos)?;
                    return Ok(());
                }
                "endfig" => {
                    *pos += 1;
                    let charcode = self.current_fig.unwrap_or(0);
                    let objects = std::mem::take(&mut self.current_objects);
                    self.figures.push(MpFigure::new(charcode, objects));
                    self.current_fig = None;
                    self.expect_semi(tokens, pos)?;
                    return Ok(());
                }
                "draw" => {
                    *pos += 1;
                    let path = self.parse_path_expression(tokens, pos)?;
                    let (color, pen, dash) = self.parse_draw_options(tokens, pos)?;
                    self.current_objects.push(MpObject::Stroke {
                        path,
                        color: color.unwrap_or(self.current_color),
                        width: pen.unwrap_or_else(|| self.current_pen.clone()).width,
                        dash,
                        line_cap: 1,
                        line_join: 1,
                        miter_limit: 10.0,
                    });
                    self.expect_semi(tokens, pos)?;
                    return Ok(());
                }
                "fill" => {
                    *pos += 1;
                    let mut path = self.parse_path_expression(tokens, pos)?;
                    path.closed = true;
                    let (color, _, _) = self.parse_draw_options(tokens, pos)?;
                    self.current_objects.push(MpObject::Fill {
                        path,
                        color: color.unwrap_or(self.current_color),
                    });
                    self.expect_semi(tokens, pos)?;
                    return Ok(());
                }
                "filldraw" => {
                    *pos += 1;
                    let mut path = self.parse_path_expression(tokens, pos)?;
                    path.closed = true;
                    let (color, pen, dash) = self.parse_draw_options(tokens, pos)?;
                    let c = color.unwrap_or(self.current_color);
                    let p = pen.unwrap_or_else(|| self.current_pen.clone());
                    self.current_objects.push(MpObject::Fill {
                        path: path.clone(),
                        color: c,
                    });
                    self.current_objects.push(MpObject::Stroke {
                        path,
                        color: c,
                        width: p.width,
                        dash,
                        line_cap: 1,
                        line_join: 1,
                        miter_limit: 10.0,
                    });
                    self.expect_semi(tokens, pos)?;
                    return Ok(());
                }
                "drawdot" => {
                    *pos += 1;
                    let pt = self.parse_pair_expression(tokens, pos)?;
                    let (color, pen, _) = self.parse_draw_options(tokens, pos)?;
                    let c = color.unwrap_or(self.current_color);
                    let p = pen.unwrap_or_else(|| self.current_pen.clone());
                    let dot_path = Path::circle(pt, p.width * 0.5);
                    self.current_objects.push(MpObject::Fill {
                        path: dot_path,
                        color: c,
                    });
                    self.expect_semi(tokens, pos)?;
                    return Ok(());
                }
                "pickup" => {
                    *pos += 1;
                    let pen = self.parse_pen_expression(tokens, pos)?;
                    self.current_pen = pen;
                    self.expect_semi(tokens, pos)?;
                    return Ok(());
                }
                "clip" => {
                    *pos += 1;
                    // clip currentpicture to <path>;
                    if is_ident(tokens.get(*pos), "currentpicture") {
                        *pos += 1;
                    }
                    if is_ident(tokens.get(*pos), "to") {
                        *pos += 1;
                    }
                    let mut path = self.parse_path_expression(tokens, pos)?;
                    path.closed = true;
                    // Clipping applies to what has been drawn so far.
                    self.current_objects.insert(0, MpObject::StartClip { path });
                    self.current_objects.push(MpObject::StopClip);
                    self.expect_semi(tokens, pos)?;
                    return Ok(());
                }
                "message" => {
                    *pos += 1;
                    if let Some(Token::String(s)) = tokens.get(*pos) {
                        self.log.push_str(s);
                        self.log.push('\n');
                        self.term.push_str(s);
                        self.term.push('\n');
                        *pos += 1;
                    }
                    self.expect_semi(tokens, pos)?;
                    return Ok(());
                }
                "numeric" | "pair" | "path" | "color" | "transform" | "string" | "boolean" => {
                    *pos += 1;
                    // Declarations: numeric a, b, c;
                    while let Some(Token::Ident(var_name)) = tokens.get(*pos) {
                        let var_name = var_name.clone();
                        *pos += 1;
                        if name == "numeric" {
                            let id = self.solver.new_var(Some(&var_name));
                            self.vars.insert(var_name, Value::Linear(LinearExpr::variable(id)));
                        } else if name == "pair" {
                            let x_id = self.solver.new_var(None);
                            let y_id = self.solver.new_var(None);
                            self.vars.insert(
                                var_name,
                                Value::LinearPair(LinearExpr::variable(x_id), LinearExpr::variable(y_id)),
                            );
                        }
                        if matches!(tokens.get(*pos), Some(Token::Comma)) {
                            *pos += 1;
                        } else {
                            break;
                        }
                    }
                    self.expect_semi(tokens, pos)?;
                    return Ok(());
                }
                "for" => {
                    *pos += 1;
                    self.enter()?;
                    let res = self.execute_for_loop(tokens, pos);
                    self.depth -= 1;
                    return res;
                }
                "end" | "bye" => {
                    *pos = tokens.len();
                    return Ok(());
                }
                n if n != "dir" && KEYWORDS.contains(&n) => {
                    Self::skip_statement(tokens, pos);
                    return Ok(());
                }
                _ => {}
            }

            // Assignment: name := expr;
            if matches!(tokens.get(*pos + 1), Some(Token::Assign)) {
                let name = name.clone();
                *pos += 2;
                let value = self.parse_expression(tokens, pos)?;
                self.vars.insert(name, value);
                self.expect_semi(tokens, pos)?;
                return Ok(());
            }
        }

        // Equation(s): expr = expr [= expr ...];
        let mut lhs = self.parse_expression(tokens, pos)?;
        if matches!(tokens.get(*pos), Some(Token::Equal)) {
            while matches!(tokens.get(*pos), Some(Token::Equal)) {
                *pos += 1;
                let rhs = self.parse_expression(tokens, pos)?;
                self.equate(&lhs, &rhs)?;
                lhs = rhs;
            }
            self.expect_semi(tokens, pos)?;
            return Ok(());
        }

        // Unsupported statement
        Self::skip_statement(tokens, pos);
        Ok(())
    }

    /// Consumes tokens up to and including the next semicolon.
    fn skip_statement(tokens: &[Token], pos: &mut usize) {
        while *pos < tokens.len() && !matches!(&tokens[*pos], Token::Semi) {
            *pos += 1;
        }
        if *pos < tokens.len() {
            *pos += 1;
        }
    }

    fn execute_for_loop(&mut self, tokens: &[Token], pos: &mut usize) -> Result<(), String> {
        let var_name = match tokens.get(*pos) {
            Some(Token::Ident(id)) if !KEYWORDS.contains(&id.as_str()) => id.clone(),
            Some(_) => return Err("Expected loop variable name".into()),
            None => return Err("Unexpected EOF in loop".into()),
        };
        *pos += 1;

        if matches!(tokens.get(*pos), Some(Token::Equal | Token::Assign)) {
            *pos += 1;
        } else {
            return Err("Expected '=' after loop variable".into());
        }

        let start_val = self.parse_known_numeric(tokens, pos)?;
        let (step_val, end_val) = match tokens.get(*pos) {
            Some(Token::Ident(id)) if id == "step" => {
                *pos += 1;
                let step = self.parse_known_numeric(tokens, pos)?;
                if !is_ident(tokens.get(*pos), "until") {
                    return Err("Missing `until` in for loop".into());
                }
                *pos += 1;
                (step, self.parse_known_numeric(tokens, pos)?)
            }
            Some(Token::Ident(id)) if id == "upto" || id == "downto" => {
                let step = if id == "upto" { 1.0 } else { -1.0 };
                *pos += 1;
                (step, self.parse_known_numeric(tokens, pos)?)
            }
            _ => (1.0, start_val),
        };

        match tokens.get(*pos) {
            Some(Token::Colon) => *pos += 1,
            Some(Token::Comma) => return Err("Unsupported for-loop value list".into()),
            _ => return Err("Missing ':' in for loop".into()),
        }

        // Collect body tokens until matching endfor
        let body_start = *pos;
        let mut depth = 1;
        loop {
            match tokens.get(*pos) {
                None => return Err("Missing endfor".into()),
                Some(Token::Ident(id)) if id == "for" => depth += 1,
                Some(Token::Ident(id)) if id == "endfor" => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            *pos += 1;
        }
        let body = &tokens[body_start..*pos];
        *pos += 1;

        if step_val == 0.0 {
            return Err("Loop step is zero, so the loop would never end".into());
        }
        let iterations = ((end_val - start_val) / step_val).floor() + 1.0;
        if iterations > MAX_STEPS as f64 {
            return Err(format!("Loop would run {iterations} times (limit {MAX_STEPS})"));
        }

        let mut cur = start_val;
        while (step_val > 0.0 && cur <= end_val + 1e-9) || (step_val < 0.0 && cur >= end_val - 1e-9) {
            self.step()?;
            self.vars.insert(var_name.clone(), Value::Numeric(cur));
            let mut sub_pos = 0;
            while sub_pos < body.len() {
                self.parse_statement(body, &mut sub_pos)?;
            }
            cur += step_val;
        }

        Ok(())
    }

    fn type_name(&self, v: &Value) -> &'static str {
        match v {
            Value::Numeric(_) => "known numeric",
            Value::Linear(_) => "unknown numeric",
            Value::Pair(_) => "known pair",
            Value::LinearPair(..) => "unknown pair",
            Value::Color(_) => "color",
            Value::Path(_) => "path",
            Value::Pen(_) => "pen",
            Value::String(_) => "string",
        }
    }

    /// The numeric `v` with dependencies resolved, if `v` is numeric.
    fn numeric_expr(&self, v: &Value) -> Option<LinearExpr> {
        match v {
            Value::Numeric(n) => Some(LinearExpr::constant(*n)),
            Value::Linear(e) => Some(self.solver.resolve(e)),
            _ => None,
        }
    }

    /// The pair `v` with dependencies resolved, if `v` is a pair.
    fn pair_exprs(&self, v: &Value) -> Option<(LinearExpr, LinearExpr)> {
        match v {
            Value::Pair(p) => Some((LinearExpr::constant(p.x), LinearExpr::constant(p.y))),
            Value::LinearPair(x, y) => Some((self.solver.resolve(x), self.solver.resolve(y))),
            _ => None,
        }
    }

    fn known_numeric(&self, v: &Value) -> Option<f64> {
        self.numeric_expr(v)?.as_known()
    }

    fn known_pair(&self, v: &Value) -> Option<Pair> {
        let (x, y) = self.pair_exprs(v)?;
        Some(Pair::new(x.as_known()?, y.as_known()?))
    }

    fn equate(&mut self, lhs: &Value, rhs: &Value) -> Result<(), String> {
        if let (Some(l), Some(r)) = (self.numeric_expr(lhs), self.numeric_expr(rhs)) {
            return self.solver.equate(&l, &r);
        }
        if let (Some((lx, ly)), Some((rx, ry))) = (self.pair_exprs(lhs), self.pair_exprs(rhs)) {
            self.solver.equate(&lx, &rx)?;
            return self.solver.equate(&self.solver.resolve(&ly), &self.solver.resolve(&ry));
        }
        match (lhs, rhs) {
            (Value::Color(a), Value::Color(b)) if a == b => Ok(()),
            (Value::String(a), Value::String(b)) if a == b => Ok(()),
            (Value::Color(_), Value::Color(_)) | (Value::String(_), Value::String(_)) => {
                Err("Inconsistent equation".into())
            }
            _ => Err(format!(
                "Equation cannot be performed ({}={})",
                self.type_name(lhs),
                self.type_name(rhs)
            )),
        }
    }

    fn parse_draw_options(
        &mut self,
        tokens: &[Token],
        pos: &mut usize,
    ) -> Result<(Option<Color>, Option<Pen>, Option<Dash>), String> {
        let mut color = None;
        let mut pen = None;
        let mut dash = None;

        while let Some(Token::Ident(opt)) = tokens.get(*pos) {
            match opt.as_str() {
                "withcolor" => {
                    *pos += 1;
                    color = Some(self.parse_color_expression(tokens, pos)?);
                }
                "withpen" => {
                    *pos += 1;
                    pen = Some(self.parse_pen_expression(tokens, pos)?);
                }
                "dashed" => {
                    *pos += 1;
                    // dashed evenly or dashed dashpattern
                    dash = Some(Dash {
                        pattern: vec![3.0, 3.0],
                        offset: 0.0,
                    });
                    if matches!(tokens.get(*pos), Some(Token::Ident(_))) {
                        *pos += 1;
                    }
                }
                _ => break,
            }
        }

        Ok((color, pen, dash))
    }

    /// expression: tertiary, or a path built from tertiaries and path joins.
    fn parse_expression(&mut self, tokens: &[Token], pos: &mut usize) -> Result<Value, String> {
        let first = self.parse_tertiary(tokens, pos)?;
        if matches!(
            tokens.get(*pos),
            Some(Token::LBrace | Token::DotDot | Token::TripleDot | Token::DashDash | Token::TripleDash | Token::Ampersand)
        ) {
            return self.parse_path_joins(tokens, pos, first);
        }
        Ok(first)
    }

    fn parse_tertiary(&mut self, tokens: &[Token], pos: &mut usize) -> Result<Value, String> {
        let mut lhs = self.parse_secondary(tokens, pos)?;
        loop {
            let negate = match tokens.get(*pos) {
                Some(Token::Plus) => false,
                Some(Token::Minus) => true,
                _ => return Ok(lhs),
            };
            *pos += 1;
            let rhs = self.parse_secondary(tokens, pos)?;
            lhs = self.add(&lhs, &rhs, negate)?;
        }
    }

    fn parse_secondary(&mut self, tokens: &[Token], pos: &mut usize) -> Result<Value, String> {
        let mut lhs = self.parse_primary(tokens, pos)?;
        loop {
            match tokens.get(*pos) {
                Some(Token::Star) => {
                    *pos += 1;
                    let rhs = self.parse_primary(tokens, pos)?;
                    lhs = self.mul(&lhs, &rhs)?;
                }
                Some(Token::Slash) => {
                    *pos += 1;
                    let rhs = self.parse_primary(tokens, pos)?;
                    let divisor = self.known_numeric(&rhs).ok_or_else(|| {
                        format!("Not implemented: ({})/({})", self.type_name(&lhs), self.type_name(&rhs))
                    })?;
                    if divisor == 0.0 {
                        return Err("Division by zero".into());
                    }
                    lhs = self.mul(&lhs, &Value::Numeric(1.0 / divisor))?;
                }
                Some(Token::Ident(op)) if TRANSFORMERS.contains(&op.as_str()) => {
                    let op = op.clone();
                    *pos += 1;
                    let rhs = self.parse_primary(tokens, pos)?;
                    lhs = self.transform(&lhs, &op, &rhs)?;
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_primary(&mut self, tokens: &[Token], pos: &mut usize) -> Result<Value, String> {
        self.enter()?;
        let res = self.parse_primary_inner(tokens, pos);
        self.depth -= 1;
        res
    }

    fn parse_primary_inner(&mut self, tokens: &[Token], pos: &mut usize) -> Result<Value, String> {
        let tok = tokens
            .get(*pos)
            .ok_or_else(|| "Unexpected EOF in expression".to_string())?;
        *pos += 1;
        match tok {
            Token::Number(n) => {
                let mut val = *n;
                // Fraction of numeric tokens: `1/3`.
                if let (Some(Token::Slash), Some(Token::Number(d))) = (tokens.get(*pos), tokens.get(*pos + 1)) {
                    if *d == 0.0 {
                        return Err("Division by zero".into());
                    }
                    val /= d;
                    *pos += 2;
                }
                // Implicit multiplication: `2cm`, `.5white`, `3(1,2)`.
                let implicit = match tokens.get(*pos) {
                    Some(Token::LParen) => true,
                    Some(Token::Ident(id)) => !KEYWORDS.contains(&id.as_str()) || id == "dir",
                    _ => false,
                };
                if implicit {
                    let rhs = self.parse_primary(tokens, pos)?;
                    return self.mul(&Value::Numeric(val), &rhs);
                }
                Ok(Value::Numeric(val))
            }
            Token::Minus => {
                let v = self.parse_primary(tokens, pos)?;
                self.mul(&v, &Value::Numeric(-1.0))
            }
            Token::Plus => self.parse_primary(tokens, pos),
            Token::LParen => {
                let first = self.parse_expression(tokens, pos)?;
                let value = if matches!(tokens.get(*pos), Some(Token::Comma)) {
                    *pos += 1;
                    let second = self.parse_expression(tokens, pos)?;
                    if matches!(tokens.get(*pos), Some(Token::Comma)) {
                        *pos += 1;
                        let third = self.parse_expression(tokens, pos)?;
                        match (self.known_numeric(&first), self.known_numeric(&second), self.known_numeric(&third)) {
                            (Some(r), Some(g), Some(b)) => Value::Color(Color::Rgb(r, g, b)),
                            _ => return Err("Color components must be known numerics".into()),
                        }
                    } else {
                        match (self.numeric_expr(&first), self.numeric_expr(&second)) {
                            (Some(x), Some(y)) => pair_value(x, y)?,
                            _ => {
                                return Err(format!(
                                    "Pair components must be numeric, got ({}, {})",
                                    self.type_name(&first),
                                    self.type_name(&second)
                                ))
                            }
                        }
                    }
                } else {
                    first
                };
                if !matches!(tokens.get(*pos), Some(Token::RParen)) {
                    return Err(format!("Missing ')' at {:?}", tokens.get(*pos)));
                }
                *pos += 1;
                Ok(value)
            }
            Token::String(s) => Ok(Value::String(s.clone())),
            Token::Ident(name) if name == "dir" => {
                let v = self.parse_primary(tokens, pos)?;
                let deg = self
                    .known_numeric(&v)
                    .ok_or_else(|| format!("Expected known numeric after `dir`, got {}", self.type_name(&v)))?;
                Ok(Value::Pair(Pair::from_polar(1.0, deg)))
            }
            Token::Ident(name) if KEYWORDS.contains(&name.as_str()) => {
                Err(format!("Missing primary before `{name}`"))
            }
            Token::Ident(name) => self.lookup(name),
            other => Err(format!("Cannot parse expression starting at {other:?}")),
        }
    }

    /// Value of a variable; undefined names are numeric unknowns and `z<n>`
    /// is the pair `(x<n>, y<n>)`.
    fn lookup(&mut self, name: &str) -> Result<Value, String> {
        if let Some(val) = self.vars.get(name) {
            return Ok(val.clone());
        }
        if let Some(idx) = name.strip_prefix('z').filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())) {
            let x = self.lookup(&format!("x{idx}"))?;
            let y = self.lookup(&format!("y{idx}"))?;
            return match (self.numeric_expr(&x), self.numeric_expr(&y)) {
                (Some(x), Some(y)) => pair_value(x, y),
                _ => Err(format!("Coordinates of {name} are not numeric")),
            };
        }
        let id = self.solver.get_var_by_name(name);
        num_value(self.solver.get_expr(id))
    }

    fn add(&self, lhs: &Value, rhs: &Value, subtract: bool) -> Result<Value, String> {
        let combine = |a: &LinearExpr, b: &LinearExpr| if subtract { a.sub(b) } else { a.add(b) };
        if let (Some(a), Some(b)) = (self.numeric_expr(lhs), self.numeric_expr(rhs)) {
            return num_value(combine(&a, &b));
        }
        if let (Some((ax, ay)), Some((bx, by))) = (self.pair_exprs(lhs), self.pair_exprs(rhs)) {
            return pair_value(combine(&ax, &bx), combine(&ay, &by));
        }
        let sign = if subtract { -1.0 } else { 1.0 };
        match (lhs, rhs) {
            (Value::Color(Color::Rgb(r1, g1, b1)), Value::Color(Color::Rgb(r2, g2, b2))) => {
                Ok(Value::Color(Color::Rgb(r1 + sign * r2, g1 + sign * g2, b1 + sign * b2)))
            }
            (Value::Color(Color::Gray(g1)), Value::Color(Color::Gray(g2))) => {
                Ok(Value::Color(Color::Gray(g1 + sign * g2)))
            }
            _ => Err(format!(
                "Not implemented: ({}){}({})",
                self.type_name(lhs),
                if subtract { "-" } else { "+" },
                self.type_name(rhs)
            )),
        }
    }

    fn mul(&self, lhs: &Value, rhs: &Value) -> Result<Value, String> {
        let not_implemented = || format!("Not implemented: ({})*({})", self.type_name(lhs), self.type_name(rhs));
        // Order the operands so that `scalar` is the numeric factor.
        let (scalar, other) = if self.numeric_expr(lhs).is_some() { (lhs, rhs) } else { (rhs, lhs) };
        let s = self.numeric_expr(scalar).ok_or_else(not_implemented)?;
        if let Some(t) = self.numeric_expr(other) {
            return match (s.as_known(), t.as_known()) {
                (Some(k), _) => num_value(t.mul_scalar(k)),
                (_, Some(k)) => num_value(s.mul_scalar(k)),
                _ => Err(not_implemented()),
            };
        }
        if let Some((x, y)) = self.pair_exprs(other) {
            return match (s.as_known(), x.as_known(), y.as_known()) {
                (Some(k), _, _) => pair_value(x.mul_scalar(k), y.mul_scalar(k)),
                (None, Some(px), Some(py)) => pair_value(s.mul_scalar(px), s.mul_scalar(py)),
                _ => Err(not_implemented()),
            };
        }
        match (other, s.as_known()) {
            (Value::Color(c), Some(k)) => Ok(Value::Color(scale_color(*c, k))),
            _ => Err(not_implemented()),
        }
    }

    fn transform(&self, lhs: &Value, op: &str, rhs: &Value) -> Result<Value, String> {
        let not_implemented = || format!("Not implemented: ({}) {op} ({})", self.type_name(lhs), self.type_name(rhs));
        if op == "shifted" {
            if self.pair_exprs(lhs).is_some() {
                return self.add(lhs, rhs, false);
            }
            let by = self.known_pair(rhs).ok_or_else(not_implemented)?;
            return match lhs {
                Value::Path(p) => path_value(p.transformed(&Transform::shifted(by.x, by.y))),
                _ => Err(not_implemented()),
            };
        }
        let s = self.known_numeric(rhs).ok_or_else(not_implemented)?;
        if let Value::Pen(pen) = lhs {
            let mut pen = pen.clone();
            match op {
                "scaled" => {
                    pen.width *= s;
                    pen.height *= s;
                }
                "xscaled" => pen.width *= s,
                "yscaled" => pen.height *= s,
                _ => return Err(not_implemented()),
            }
            return Ok(Value::Pen(pen));
        }
        let t = match op {
            "scaled" => Transform::scaled(s),
            "xscaled" => Transform::xscaled(s),
            "yscaled" => Transform::yscaled(s),
            "rotated" => Transform::rotated(s),
            "slanted" => Transform::slanted(s),
            _ => unreachable!("not a transformer: {op}"),
        };
        if let Some((x, y)) = self.pair_exprs(lhs) {
            let nx = x.mul_scalar(t.xx).add(&y.mul_scalar(t.xy)).add(&LinearExpr::constant(t.x0));
            let ny = x.mul_scalar(t.yx).add(&y.mul_scalar(t.yy)).add(&LinearExpr::constant(t.y0));
            return pair_value(nx, ny);
        }
        match lhs {
            Value::Path(p) => path_value(p.transformed(&t)),
            _ => Err(not_implemented()),
        }
    }

    /// Path joins after the first knot: directions `{..}`, `..`, `...`,
    /// `--`, `---`, `..tension a [and b]..` and `cycle`.
    fn parse_path_joins(&mut self, tokens: &[Token], pos: &mut usize, first: Value) -> Result<Value, String> {
        let mut specs = vec![KnotSpec::new(self.knot_point(&first)?)];
        let mut closed = false;
        loop {
            let mut pre = None;
            if matches!(tokens.get(*pos), Some(Token::LBrace)) {
                pre = Some(self.parse_direction(tokens, pos)?);
            }
            let (mut right_tension, mut left_tension) = (1.0, 1.0);
            let mut post = KnotSide::Open;
            match tokens.get(*pos) {
                Some(Token::DotDot) => {
                    *pos += 1;
                    if is_ident(tokens.get(*pos), "tension") {
                        *pos += 1;
                        right_tension = self.parse_tension(tokens, pos)?;
                        left_tension = right_tension;
                        if is_ident(tokens.get(*pos), "and") {
                            *pos += 1;
                            left_tension = self.parse_tension(tokens, pos)?;
                        }
                        if !matches!(tokens.get(*pos), Some(Token::DotDot)) {
                            return Err("Missing `..` after tension".into());
                        }
                        *pos += 1;
                    } else if is_ident(tokens.get(*pos), "controls") {
                        return Err("Explicit `controls` in paths are not supported".into());
                    }
                }
                Some(Token::TripleDot) => {
                    *pos += 1;
                    (right_tension, left_tension) = (-1.0, -1.0);
                }
                Some(Token::DashDash) => {
                    *pos += 1;
                    pre = Some(KnotSide::Curl(1.0));
                    post = KnotSide::Curl(1.0);
                }
                Some(Token::TripleDash) => {
                    *pos += 1;
                    (right_tension, left_tension) = (INFINITY_TENSION, INFINITY_TENSION);
                }
                Some(Token::Ampersand) => return Err("Path concatenation with `&` is not supported".into()),
                _ => {
                    // A trailing `{dir}` constrains the last knot.
                    if let Some(side) = pre {
                        specs.last_mut().expect("path has a first knot").right = side;
                    }
                    break;
                }
            }
            if matches!(tokens.get(*pos), Some(Token::LBrace)) {
                post = self.parse_direction(tokens, pos)?;
            }

            let last = specs.last_mut().expect("path has a first knot");
            if let Some(side) = pre {
                last.right = side;
            }
            last.right_tension = right_tension;

            if is_ident(tokens.get(*pos), "cycle") {
                *pos += 1;
                specs[0].left = post;
                specs[0].left_tension = left_tension;
                closed = true;
                break;
            }
            let knot = self.parse_tertiary(tokens, pos)?;
            specs.push(KnotSpec {
                left: post,
                left_tension,
                ..KnotSpec::new(self.knot_point(&knot)?)
            });
        }
        path_value(solve_path(&specs, closed))
    }

    fn knot_point(&self, v: &Value) -> Result<Pair, String> {
        if let Some(p) = self.known_pair(v) {
            return Ok(p);
        }
        Err(match v {
            Value::Path(_) => "Joining path values is not supported".into(),
            Value::LinearPair(..) => "Undefined coordinates in path".into(),
            _ => format!("Expected pair as path knot, got {}", self.type_name(v)),
        })
    }

    /// `{curl c}`, `{dir d}`, `{pair}` or `{x, y}`; a zero vector means `{curl 1}`.
    fn parse_direction(&mut self, tokens: &[Token], pos: &mut usize) -> Result<KnotSide, String> {
        *pos += 1; // `{`
        self.enter()?;
        let res = self.parse_direction_inner(tokens, pos);
        self.depth -= 1;
        let side = res?;
        if !matches!(tokens.get(*pos), Some(Token::RBrace)) {
            return Err(format!("Missing '}}' at {:?}", tokens.get(*pos)));
        }
        *pos += 1;
        Ok(side)
    }

    fn parse_direction_inner(&mut self, tokens: &[Token], pos: &mut usize) -> Result<KnotSide, String> {
        if is_ident(tokens.get(*pos), "curl") {
            *pos += 1;
            let curl = self.parse_known_numeric(tokens, pos)?;
            if curl < 0.0 {
                return Err(format!("Improper curl ({})", fmt_number(curl)));
            }
            return Ok(KnotSide::Curl(curl));
        }
        let first = self.parse_expression(tokens, pos)?;
        let dir = if matches!(tokens.get(*pos), Some(Token::Comma)) {
            *pos += 1;
            let second = self.parse_expression(tokens, pos)?;
            match (self.known_numeric(&first), self.known_numeric(&second)) {
                (Some(x), Some(y)) => Pair::new(x, y),
                _ => return Err("Undefined direction".into()),
            }
        } else {
            self.known_pair(&first)
                .ok_or_else(|| format!("Expected known pair as direction, got {}", self.type_name(&first)))?
        };
        if dir == Pair::ZERO {
            Ok(KnotSide::Curl(1.0))
        } else {
            Ok(KnotSide::Given(dir.angle_deg()))
        }
    }

    /// `[atleast] <primary>`; returns a negative value for `atleast`.
    fn parse_tension(&mut self, tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
        let at_least = is_ident(tokens.get(*pos), "atleast");
        if at_least {
            *pos += 1;
        }
        let v = self.parse_primary(tokens, pos)?;
        let t = self
            .known_numeric(&v)
            .ok_or_else(|| format!("Expected known numeric tension, got {}", self.type_name(&v)))?;
        if t < 0.75 {
            return Err(format!("Improper tension ({})", fmt_number(t)));
        }
        Ok(if at_least { -t } else { t })
    }

    fn parse_known_numeric(&mut self, tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
        let val = self.parse_expression(tokens, pos)?;
        self.known_numeric(&val)
            .ok_or_else(|| format!("Expected known numeric value, got {}", self.type_name(&val)))
    }

    fn parse_pair_expression(&mut self, tokens: &[Token], pos: &mut usize) -> Result<Pair, String> {
        let val = self.parse_expression(tokens, pos)?;
        self.known_pair(&val)
            .ok_or_else(|| format!("Expected known pair, got {}", self.type_name(&val)))
    }

    fn parse_path_expression(&mut self, tokens: &[Token], pos: &mut usize) -> Result<Path, String> {
        let val = self.parse_expression(tokens, pos)?;
        if let Value::Path(p) = val {
            return Ok(p);
        }
        match self.known_pair(&val) {
            Some(p) => Ok(Path::circle(p, 0.5)),
            None => Err(format!("Expected path, got {}", self.type_name(&val))),
        }
    }

    fn parse_color_expression(&mut self, tokens: &[Token], pos: &mut usize) -> Result<Color, String> {
        let val = self.parse_expression(tokens, pos)?;
        Ok(match val {
            Value::Color(c) => c,
            Value::Pair(p) => Color::Rgb(p.x, p.y, 0.0),
            Value::Numeric(g) => Color::Gray(g),
            _ => Color::BLACK,
        })
    }

    fn parse_pen_expression(&mut self, tokens: &[Token], pos: &mut usize) -> Result<Pen, String> {
        let val = self.parse_expression(tokens, pos)?;
        if let Value::Pen(p) = val {
            Ok(p)
        } else {
            Ok(Pen::default_pen())
        }
    }

    fn expect_semi(&self, tokens: &[Token], pos: &mut usize) -> Result<(), String> {
        if *pos < tokens.len() && matches!(&tokens[*pos], Token::Semi) {
            *pos += 1;
            Ok(())
        } else {
            Err(format!("Expected ';' at {:?}", tokens.get(*pos)))
        }
    }
}
