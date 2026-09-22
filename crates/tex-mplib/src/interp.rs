//! MetaPost language lexer, parser and runtime interpreter.

use std::collections::HashMap;

use crate::curves::{solve_path, KnotSpec};
use crate::solver::{LinearExpr, LinearSolver};
use crate::types::{Color, Dash, MpFigure, MpObject, Pair, Path, Pen, Transform};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Numeric(f64),
    Pair(Pair),
    Color(Color),
    Path(Path),
    Transform(Transform),
    Pen(Pen),
    String(String),
    Boolean(bool),
    Unknown(usize), // var_id in LinearSolver
    UnknownPair(usize, usize), // (x_var_id, y_var_id)
}

impl Value {
    pub fn as_numeric(&self, solver: &LinearSolver) -> Option<f64> {
        match self {
            Self::Numeric(n) => Some(*n),
            Self::Unknown(id) => solver.get_expr(*id).as_known(),
            _ => None,
        }
    }

    pub fn as_pair(&self, solver: &LinearSolver) -> Option<Pair> {
        match self {
            Self::Pair(p) => Some(*p),
            Self::UnknownPair(x_id, y_id) => {
                let x = solver.get_expr(*x_id).as_known()?;
                let y = solver.get_expr(*y_id).as_known()?;
                Some(Pair::new(x, y))
            }
            _ => None,
        }
    }

    pub fn as_path(&self, solver: &LinearSolver) -> Option<Path> {
        match self {
            Self::Path(p) => Some(p.clone()),
            Self::Pair(p) => Some(Path::circle(*p, 0.5)),
            Self::UnknownPair(_x_id, _y_id) => {
                let pt = self.as_pair(solver)?;
                Some(Path::circle(pt, 0.5))
            }
            _ => None,
        }
    }
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
    Equal,
    Assign, // :=
    Plus,
    Minus,
    Star,
    Slash,
    DotDot, // ..
    DashDash, // --
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

        if c == ';' {
            tokens.push(Token::Semi);
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
        if c == ',' {
            tokens.push(Token::Comma);
            i += 1;
            continue;
        }
        if c == '(' {
            tokens.push(Token::LParen);
            i += 1;
            continue;
        }
        if c == ')' {
            tokens.push(Token::RParen);
            i += 1;
            continue;
        }
        if c == '[' {
            tokens.push(Token::LBracket);
            i += 1;
            continue;
        }
        if c == ']' {
            tokens.push(Token::RBracket);
            i += 1;
            continue;
        }
        if c == '=' {
            tokens.push(Token::Equal);
            i += 1;
            continue;
        }
        if c == '+' {
            tokens.push(Token::Plus);
            i += 1;
            continue;
        }
        if c == '-' {
            if i + 1 < len && chars[i + 1] == '-' {
                tokens.push(Token::DashDash);
                i += 2;
            } else {
                tokens.push(Token::Minus);
                i += 1;
            }
            continue;
        }
        if c == '*' {
            tokens.push(Token::Star);
            i += 1;
            continue;
        }
        if c == '/' {
            tokens.push(Token::Slash);
            i += 1;
            continue;
        }
        if c == '&' {
            tokens.push(Token::Ampersand);
            i += 1;
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
        if c == '.' {
            if i + 1 < len && chars[i + 1] == '.' {
                tokens.push(Token::DotDot);
                i += 2;
                continue;
            }
            if i + 1 < len && chars[i + 1].is_ascii_digit() {
                // Decimal number like .5
                let mut num_str = String::from("0.");
                i += 1;
                while i < len && chars[i].is_ascii_digit() {
                    num_str.push(chars[i]);
                    i += 1;
                }
                if let Ok(n) = num_str.parse::<f64>() {
                    tokens.push(Token::Number(n));
                    continue;
                }
            }
        }
        if c.is_ascii_digit() {
            let mut num_str = String::new();
            while i < len && (chars[i].is_ascii_digit() || chars[i] == '.') {
                if chars[i] == '.' && i + 1 < len && chars[i + 1] == '.' {
                    break;
                }
                num_str.push(chars[i]);
                i += 1;
            }
            if let Ok(n) = num_str.parse::<f64>() {
                tokens.push(Token::Number(n));
                continue;
            }
        }
        if c == '"' {
            i += 1;
            let mut s = String::new();
            while i < len && chars[i] != '"' {
                s.push(chars[i]);
                i += 1;
            }
            if i < len && chars[i] == '"' {
                i += 1;
            }
            tokens.push(Token::String(s));
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let mut ident = String::new();
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                ident.push(chars[i]);
                i += 1;
            }
            tokens.push(Token::Ident(ident));
            continue;
        }

        i += 1;
    }

    Ok(tokens)
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

        // Default pen
        self.vars.insert("currentpen".into(), Value::Pen(Pen::default_pen()));
    }

    pub fn run(&mut self, code: &str) -> Result<(), String> {
        let tokens = tokenize(code)?;
        let mut pos = 0;

        while pos < tokens.len() {
            self.parse_statement(&tokens, &mut pos)?;
        }

        Ok(())
    }

    fn parse_statement(&mut self, tokens: &[Token], pos: &mut usize) -> Result<(), String> {
        if *pos >= tokens.len() {
            return Ok(());
        }

        if let Token::Semi = &tokens[*pos] {
            *pos += 1;
            return Ok(());
        }

        if let Token::Ident(name) = &tokens[*pos] {
            match name.as_str() {
                "beginfig" => {
                    *pos += 1;
                    let num = self.expect_number_in_parens(tokens, pos)?;
                    self.current_fig = Some(num as i32);
                    self.current_objects.clear();
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
                    if *pos < tokens.len() && matches!(&tokens[*pos], Token::Ident(id) if id == "currentpicture") {
                        *pos += 1;
                    }
                    if *pos < tokens.len() && matches!(&tokens[*pos], Token::Ident(id) if id == "to") {
                        *pos += 1;
                    }
                    let mut path = self.parse_path_expression(tokens, pos)?;
                    path.closed = true;
                    self.current_objects.push(MpObject::StartClip { path });
                    self.expect_semi(tokens, pos)?;
                    return Ok(());
                }
                "message" => {
                    *pos += 1;
                    if *pos < tokens.len() {
                        if let Token::String(s) = &tokens[*pos] {
                            self.log.push_str(s);
                            self.log.push('\n');
                            self.term.push_str(s);
                            self.term.push('\n');
                            *pos += 1;
                        }
                    }
                    self.expect_semi(tokens, pos)?;
                    return Ok(());
                }
                "numeric" | "pair" | "path" | "color" | "transform" | "string" | "boolean" => {
                    *pos += 1;
                    // Declarations: numeric a, b, c;
                    while *pos < tokens.len() {
                        if let Token::Ident(var_name) = &tokens[*pos] {
                            let name_clone = var_name.clone();
                            *pos += 1;
                            if name.as_str() == "numeric" {
                                let id = self.solver.get_var_by_name(&name_clone);
                                self.vars.insert(name_clone, Value::Unknown(id));
                            } else if name.as_str() == "pair" {
                                let x_id = self.solver.get_var_by_name(&format!("{name_clone}.x"));
                                let y_id = self.solver.get_var_by_name(&format!("{name_clone}.y"));
                                self.vars.insert(name_clone, Value::UnknownPair(x_id, y_id));
                            }
                        }
                        if *pos < tokens.len() && matches!(&tokens[*pos], Token::Comma) {
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
                    self.execute_for_loop(tokens, pos)?;
                    return Ok(());
                }
                _ => {}
            }
        }

        // Assignment or equation:
        // x := expr; OR expr = expr;
        let left_expr = self.parse_expression(tokens, pos)?;

        if *pos < tokens.len() {
            match &tokens[*pos] {
                Token::Assign => {
                    *pos += 1;
                    let right_expr = self.parse_expression(tokens, pos)?;
                    if let Value::String(var_name) = left_expr {
                        self.vars.insert(var_name, right_expr);
                    }
                    self.expect_semi(tokens, pos)?;
                    return Ok(());
                }
                Token::Equal => {
                    *pos += 1;
                    let right_expr = self.parse_expression(tokens, pos)?;
                    self.equate_values(&left_expr, &right_expr)?;
                    self.expect_semi(tokens, pos)?;
                    return Ok(());
                }
                _ => {}
            }
        }

        // Consume until next semicolon
        while *pos < tokens.len() && !matches!(&tokens[*pos], Token::Semi) {
            *pos += 1;
        }
        if *pos < tokens.len() {
            *pos += 1;
        }
        Ok(())
    }

    fn execute_for_loop(&mut self, tokens: &[Token], pos: &mut usize) -> Result<(), String> {
        let var_name = if *pos < tokens.len() {
            if let Token::Ident(id) = &tokens[*pos] {
                let s = id.clone();
                *pos += 1;
                s
            } else {
                return Err("Expected loop variable name".into());
            }
        } else {
            return Err("Unexpected EOF in loop".into());
        };

        if *pos < tokens.len() && matches!(&tokens[*pos], Token::Equal) {
            *pos += 1;
        } else {
            return Err("Expected '=' after loop variable".into());
        }

        let start_val = self.parse_number(tokens, pos)?;
        let mut step_val = 1.0;

        if *pos < tokens.len() && matches!(&tokens[*pos], Token::Ident(id) if id == "step") {
            *pos += 1;
            step_val = self.parse_number(tokens, pos)?;
        }

        let end_val = if *pos < tokens.len()
            && matches!(&tokens[*pos], Token::Ident(id) if id == "upto" || id == "until")
        {
            *pos += 1;
            self.parse_number(tokens, pos)?
        } else {
            start_val
        };

        if *pos < tokens.len() && matches!(&tokens[*pos], Token::Colon) {
            *pos += 1;
        }

        // Collect body tokens until matching endfor
        let mut body = Vec::new();
        let mut depth = 1;
        while *pos < tokens.len() {
            if let Token::Ident(id) = &tokens[*pos] {
                if id == "for" {
                    depth += 1;
                } else if id == "endfor" {
                    depth -= 1;
                    if depth == 0 {
                        *pos += 1;
                        self.expect_semi(tokens, pos)?;
                        break;
                    }
                }
            }
            body.push(tokens[*pos].clone());
            *pos += 1;
        }

        let mut cur = start_val;
        while (step_val > 0.0 && cur <= end_val + 1e-9) || (step_val < 0.0 && cur >= end_val - 1e-9) {
            self.vars.insert(var_name.clone(), Value::Numeric(cur));
            let mut sub_pos = 0;
            while sub_pos < body.len() {
                self.parse_statement(&body, &mut sub_pos)?;
            }
            cur += step_val;
        }

        Ok(())
    }

    fn equate_values(&mut self, left: &Value, right: &Value) -> Result<(), String> {
        let left_num = left.as_numeric(&self.solver);
        let right_num = right.as_numeric(&self.solver);
        if let (Some(l), Some(r)) = (left_num, right_num) {
            let el = LinearExpr::constant(l);
            let er = LinearExpr::constant(r);
            return self.solver.equate(&el, &er);
        }

        let left_pair = left.as_pair(&self.solver);
        let right_pair = right.as_pair(&self.solver);
        if let (Some(lp), Some(rp)) = (left_pair, right_pair) {
            let el_x = LinearExpr::constant(lp.x);
            let er_x = LinearExpr::constant(rp.x);
            self.solver.equate(&el_x, &er_x)?;
            let el_y = LinearExpr::constant(lp.y);
            let er_y = LinearExpr::constant(rp.y);
            return self.solver.equate(&el_y, &er_y);
        }

        Ok(())
    }

    fn parse_draw_options(
        &mut self,
        tokens: &[Token],
        pos: &mut usize,
    ) -> Result<(Option<Color>, Option<Pen>, Option<Dash>), String> {
        let mut color = None;
        let mut pen = None;
        let mut dash = None;

        while *pos < tokens.len() {
            if let Token::Ident(opt) = &tokens[*pos] {
                match opt.as_str() {
                    "withcolor" => {
                        *pos += 1;
                        let c = self.parse_color_expression(tokens, pos)?;
                        color = Some(c);
                        continue;
                    }
                    "withpen" => {
                        *pos += 1;
                        let p = self.parse_pen_expression(tokens, pos)?;
                        pen = Some(p);
                        continue;
                    }
                    "dashed" => {
                        *pos += 1;
                        // dashed evenly or dashed dashpattern
                        dash = Some(Dash {
                            pattern: vec![3.0, 3.0],
                            offset: 0.0,
                        });
                        if *pos < tokens.len() && matches!(&tokens[*pos], Token::Ident(_)) {
                            *pos += 1;
                        }
                        continue;
                    }
                    _ => break,
                }
            } else {
                break;
            }
        }

        Ok((color, pen, dash))
    }

    fn parse_expression(&mut self, tokens: &[Token], pos: &mut usize) -> Result<Value, String> {
        if *pos >= tokens.len() {
            return Err("Unexpected EOF in expression".into());
        }

        // Check for (x, y) pair or path
        if let Token::LParen = &tokens[*pos] {
            *pos += 1;
            let first = self.parse_number(tokens, pos)?;
            if *pos < tokens.len() && matches!(&tokens[*pos], Token::Comma) {
                *pos += 1;
                let second = self.parse_number(tokens, pos)?;
                if *pos < tokens.len() && matches!(&tokens[*pos], Token::RParen) {
                    *pos += 1;
                    return Ok(Value::Pair(Pair::new(first, second)));
                }
            }
        }

        if let Token::Number(n) = &tokens[*pos] {
            let mut val = *n;
            *pos += 1;
            // Check for units like 2cm or 10pt
            if *pos < tokens.len() {
                if let Token::Ident(unit) = &tokens[*pos] {
                    if let Some(scale) = self.vars.get(unit).and_then(|v| v.as_numeric(&self.solver)) {
                        val *= scale;
                        *pos += 1;
                    }
                }
            }
            return Ok(Value::Numeric(val));
        }

        if let Token::String(s) = &tokens[*pos] {
            let s_val = s.clone();
            *pos += 1;
            return Ok(Value::String(s_val));
        }

        if let Token::Ident(name) = &tokens[*pos] {
            let name_str = name.clone();
            *pos += 1;

            // Check if name is z0, z1, etc.
            if name_str.starts_with('z') && name_str.len() > 1 && name_str[1..].chars().all(|c| c.is_ascii_digit()) {
                let idx = &name_str[1..];
                let x_id = self.solver.get_var_by_name(&format!("x{idx}"));
                let y_id = self.solver.get_var_by_name(&format!("y{idx}"));
                return Ok(Value::UnknownPair(x_id, y_id));
            }

            if let Some(val) = self.vars.get(&name_str).cloned() {
                return Ok(val);
            }

            return Ok(Value::String(name_str));
        }

        Err(format!("Cannot parse expression starting at {:?}", &tokens[*pos]))
    }

    fn parse_number(&mut self, tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
        let val = self.parse_expression(tokens, pos)?;
        val.as_numeric(&self.solver)
            .ok_or_else(|| format!("Expected numeric value, got {:?}", val))
    }

    fn parse_pair_expression(&mut self, tokens: &[Token], pos: &mut usize) -> Result<Pair, String> {
        let val = self.parse_expression(tokens, pos)?;
        val.as_pair(&self.solver)
            .ok_or_else(|| format!("Expected pair, got {:?}", val))
    }

    fn parse_path_expression(&mut self, tokens: &[Token], pos: &mut usize) -> Result<Path, String> {
        // Paths can be:
        // 1. A variable (e.g. unitsquare, fullcircle)
        // 2. A sequence of knot specs: p0 -- p1 .. p2 .. cycle
        let first_val = self.parse_expression(tokens, pos)?;

        // Check if there are path connectors: --, .., &
        let has_connector = *pos < tokens.len()
            && matches!(&tokens[*pos], Token::DashDash | Token::DotDot | Token::Ampersand);

        if !has_connector {
            if let Some(p) = first_val.as_path(&self.solver) {
                return self.parse_path_transforms(tokens, pos, p);
            }
        }

        let first_pt = first_val.as_pair(&self.solver)
            .ok_or_else(|| format!("Expected pair at start of path, got {:?}", first_val))?;

        let mut specs = vec![KnotSpec::new(first_pt)];
        let mut closed = false;

        while *pos < tokens.len() {
            if matches!(&tokens[*pos], Token::DashDash) {
                *pos += 1;
                if *pos < tokens.len() && matches!(&tokens[*pos], Token::Ident(id) if id == "cycle") {
                    *pos += 1;
                    closed = true;
                    break;
                }
                let pt = self.parse_pair_expression(tokens, pos)?;
                // Straight segment: high tension or line
                let mut spec = KnotSpec::new(pt);
                spec.tension_in = 1000.0;
                specs.push(spec);
            } else if matches!(&tokens[*pos], Token::DotDot) {
                *pos += 1;
                if *pos < tokens.len() && matches!(&tokens[*pos], Token::Ident(id) if id == "cycle") {
                    *pos += 1;
                    closed = true;
                    break;
                }
                let pt = self.parse_pair_expression(tokens, pos)?;
                specs.push(KnotSpec::new(pt));
            } else {
                break;
            }
        }

        let path = solve_path(&specs, closed);
        self.parse_path_transforms(tokens, pos, path)
    }

    fn parse_path_transforms(&mut self, tokens: &[Token], pos: &mut usize, mut path: Path) -> Result<Path, String> {
        while *pos < tokens.len() {
            if let Token::Ident(t) = &tokens[*pos] {
                match t.as_str() {
                    "scaled" => {
                        *pos += 1;
                        let s = self.parse_number(tokens, pos)?;
                        path = path.transformed(&Transform::scaled(s));
                        continue;
                    }
                    "xscaled" => {
                        *pos += 1;
                        let s = self.parse_number(tokens, pos)?;
                        path = path.transformed(&Transform::xscaled(s));
                        continue;
                    }
                    "yscaled" => {
                        *pos += 1;
                        let s = self.parse_number(tokens, pos)?;
                        path = path.transformed(&Transform::yscaled(s));
                        continue;
                    }
                    "shifted" => {
                        *pos += 1;
                        let pt = self.parse_pair_expression(tokens, pos)?;
                        path = path.transformed(&Transform::shifted(pt.x, pt.y));
                        continue;
                    }
                    "rotated" => {
                        *pos += 1;
                        let deg = self.parse_number(tokens, pos)?;
                        path = path.transformed(&Transform::rotated(deg));
                        continue;
                    }
                    "slanted" => {
                        *pos += 1;
                        let s = self.parse_number(tokens, pos)?;
                        path = path.transformed(&Transform::slanted(s));
                        continue;
                    }
                    _ => break,
                }
            } else {
                break;
            }
        }
        Ok(path)
    }

    fn parse_color_expression(&mut self, tokens: &[Token], pos: &mut usize) -> Result<Color, String> {
        let val = self.parse_expression(tokens, pos)?;
        match val {
            Value::Color(c) => Ok(c),
            Value::Pair(p) => Ok(Color::Rgb(p.x, p.y, 0.0)),
            Value::Numeric(g) => Ok(Color::Gray(g)),
            _ => Ok(Color::BLACK),
        }
    }

    fn parse_pen_expression(&mut self, tokens: &[Token], pos: &mut usize) -> Result<Pen, String> {
        if *pos < tokens.len() && matches!(&tokens[*pos], Token::Ident(id) if id == "pencircle") {
            *pos += 1;
            let mut pen = Pen::circle(1.0);
            if *pos < tokens.len() && matches!(&tokens[*pos], Token::Ident(id) if id == "scaled") {
                *pos += 1;
                let s = self.parse_number(tokens, pos)?;
                pen.width = s;
                pen.height = s;
            }
            return Ok(pen);
        }

        let val = self.parse_expression(tokens, pos)?;
        if let Value::Pen(p) = val {
            Ok(p)
        } else {
            Ok(Pen::default_pen())
        }
    }

    fn expect_number_in_parens(&mut self, tokens: &[Token], pos: &mut usize) -> Result<f64, String> {
        if *pos < tokens.len() && matches!(&tokens[*pos], Token::LParen) {
            *pos += 1;
        } else {
            return Err("Expected '('".into());
        }
        let num = self.parse_number(tokens, pos)?;
        if *pos < tokens.len() && matches!(&tokens[*pos], Token::RParen) {
            *pos += 1;
        } else {
            return Err("Expected ')'".into());
        }
        Ok(num)
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
