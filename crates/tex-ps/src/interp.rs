//! PostScript Level 2/3 interpreter and PDF content stream compiler.

use std::collections::HashMap;
use std::io::Write;

use crate::lexer::{tokenize_ps, Token};
use crate::types::{EpsBoundingBox, GraphicsState, Matrix, PathOp, PsColor, PsValue};

/// Execution error in PostScript interpreter.
#[derive(Clone, Debug, PartialEq)]
pub struct PsError {
    pub error_type: String,
    pub message: String,
}

impl PsError {
    pub fn new(error_type: &str, message: &str) -> Self {
        Self {
            error_type: error_type.to_string(),
            message: message.to_string(),
        }
    }
}

impl std::fmt::Display for PsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.error_type, self.message)
    }
}

impl std::error::Error for PsError {}

/// The PostScript interpreter state.
pub struct PsInterpreter {
    pub operand_stack: Vec<PsValue>,
    pub dict_stack: Vec<HashMap<String, PsValue>>,
    pub gstate: GraphicsState,
    pub gstate_stack: Vec<GraphicsState>,
    pub pdf_stream: Vec<u8>,
    pub bbox: EpsBoundingBox,
    pub step_count: u64,
}

impl PsInterpreter {
    pub fn new(bbox: EpsBoundingBox) -> Self {
        let mut interp = Self {
            operand_stack: Vec::new(),
            dict_stack: vec![HashMap::new(), HashMap::new(), HashMap::new()], // systemdict, globaldict, userdict
            gstate: GraphicsState::default(),
            gstate_stack: Vec::new(),
            pdf_stream: Vec::new(),
            bbox,
            step_count: 0,
        };
        interp.init_systemdict();
        interp
    }

    fn init_systemdict(&mut self) {
        let userdict = self.dict_stack.last_mut().unwrap();
        userdict.insert("true".into(), PsValue::Boolean(true));
        userdict.insert("false".into(), PsValue::Boolean(false));
        userdict.insert("null".into(), PsValue::Null);
    }

    pub fn execute(&mut self, ps_bytes: &[u8]) -> Result<(), PsError> {
        let tokens = tokenize_ps(ps_bytes)
            .map_err(|e| PsError::new("syntaxerror", &e))?;

        let mut pos = 0;
        let parsed_values = self.parse_token_group(&tokens, &mut pos, None)?;

        for val in parsed_values {
            self.eval_value(val)?;
        }

        Ok(())
    }

    fn parse_token_group(
        &mut self,
        tokens: &[Token],
        pos: &mut usize,
        delimiter: Option<&Token>,
    ) -> Result<Vec<PsValue>, PsError> {
        let mut values = Vec::new();

        while *pos < tokens.len() {
            let tok = &tokens[*pos];

            if let Some(delim) = delimiter {
                if tok == delim {
                    *pos += 1;
                    return Ok(values);
                }
            }

            match tok {
                Token::Value(v) => {
                    values.push(v.clone());
                    *pos += 1;
                }
                Token::LBrace => {
                    *pos += 1;
                    let proc_values = self.parse_token_group(tokens, pos, Some(&Token::RBrace))?;
                    values.push(PsValue::Procedure(proc_values));
                }
                Token::RBrace => {
                    if delimiter != Some(&Token::RBrace) {
                        return Err(PsError::new("syntaxerror", "Unmatched '}'"));
                    }
                    *pos += 1;
                    return Ok(values);
                }
                Token::LBracket => {
                    *pos += 1;
                    values.push(PsValue::Mark);
                }
                Token::RBracket => {
                    *pos += 1;
                    values.push(PsValue::ExecutableName("]".into()));
                }
                Token::LDict => {
                    *pos += 1;
                    values.push(PsValue::Mark);
                }
                Token::RDict => {
                    *pos += 1;
                    values.push(PsValue::ExecutableName(">>".into()));
                }
            }
        }

        if delimiter.is_some() {
            return Err(PsError::new("syntaxerror", "Unexpected EOF inside group"));
        }

        Ok(values)
    }

    fn eval_value(&mut self, val: PsValue) -> Result<(), PsError> {
        self.step_count += 1;
        if self.step_count > 50_000_000 {
            return Err(PsError::new("limitcheck", "PostScript execution step limit exceeded"));
        }

        match val {
            PsValue::ExecutableName(name) => {
                // Check dictionary stack for procedure or definition
                if let Some(defined) = self.lookup_dict(&name) {
                    if let PsValue::Procedure(proc) = defined {
                        for sub_val in proc {
                            self.eval_value(sub_val)?;
                        }
                        return Ok(());
                    } else {
                        self.operand_stack.push(defined);
                        return Ok(());
                    }
                }

                // Builtin operator
                self.dispatch_operator(&name)?;
            }
            other => {
                self.operand_stack.push(other);
            }
        }

        Ok(())
    }

    fn lookup_dict(&self, name: &str) -> Option<PsValue> {
        for dict in self.dict_stack.iter().rev() {
            if let Some(val) = dict.get(name) {
                return Some(val.clone());
            }
        }
        None
    }

    fn dispatch_operator(&mut self, op: &str) -> Result<(), PsError> {
        match op {
            // Stack manipulation
            "pop" => {
                self.pop()?;
            }
            "exch" => {
                let a = self.pop()?;
                let b = self.pop()?;
                self.operand_stack.push(a);
                self.operand_stack.push(b);
            }
            "dup" => {
                let a = self.peek()?.clone();
                self.operand_stack.push(a);
            }
            "copy" => {
                let n = self.pop_i64()? as usize;
                let len = self.operand_stack.len();
                if len < n {
                    return Err(PsError::new("stackunderflow", "copy"));
                }
                let slice = self.operand_stack[len - n..].to_vec();
                self.operand_stack.extend(slice);
            }
            "index" => {
                let n = self.pop_i64()? as usize;
                let len = self.operand_stack.len();
                if n >= len {
                    return Err(PsError::new("stackunderflow", "index"));
                }
                let val = self.operand_stack[len - 1 - n].clone();
                self.operand_stack.push(val);
            }
            "roll" => {
                let j = self.pop_i64()?;
                let n = self.pop_i64()? as usize;
                let len = self.operand_stack.len();
                if n > len {
                    return Err(PsError::new("stackunderflow", "roll"));
                }
                if n > 0 {
                    let slice = &mut self.operand_stack[len - n..];
                    let rot = ((j % n as i64) + n as i64) as usize % n;
                    slice.rotate_right(rot);
                }
            }
            "clear" => {
                self.operand_stack.clear();
            }
            "count" => {
                let c = self.operand_stack.len() as i64;
                self.operand_stack.push(PsValue::Integer(c));
            }
            "mark" => {
                self.operand_stack.push(PsValue::Mark);
            }
            "cleartomark" => {
                while let Some(v) = self.operand_stack.pop() {
                    if matches!(v, PsValue::Mark) {
                        break;
                    }
                }
            }
            "counttomark" => {
                let mut count = 0;
                for v in self.operand_stack.iter().rev() {
                    if matches!(v, PsValue::Mark) {
                        break;
                    }
                    count += 1;
                }
                self.operand_stack.push(PsValue::Integer(count));
            }
            "]" => {
                let mut arr = Vec::new();
                while let Some(v) = self.operand_stack.pop() {
                    if matches!(v, PsValue::Mark) {
                        break;
                    }
                    arr.push(v);
                }
                arr.reverse();
                self.operand_stack.push(PsValue::Array(arr));
            }
            ">>" => {
                let mut map = HashMap::new();
                let mut items = Vec::new();
                while let Some(v) = self.operand_stack.pop() {
                    if matches!(v, PsValue::Mark) {
                        break;
                    }
                    items.push(v);
                }
                items.reverse();
                let mut i = 0;
                while i + 1 < items.len() {
                    if let Some(key) = items[i].as_str() {
                        map.insert(key.to_string(), items[i + 1].clone());
                    }
                    i += 2;
                }
                self.operand_stack.push(PsValue::Dict(map));
            }

            // Math
            "add" => {
                let b = self.pop_f64()?;
                let a = self.pop_f64()?;
                self.operand_stack.push(PsValue::Real(a + b));
            }
            "sub" => {
                let b = self.pop_f64()?;
                let a = self.pop_f64()?;
                self.operand_stack.push(PsValue::Real(a - b));
            }
            "mul" => {
                let b = self.pop_f64()?;
                let a = self.pop_f64()?;
                self.operand_stack.push(PsValue::Real(a * b));
            }
            "div" => {
                let b = self.pop_f64()?;
                let a = self.pop_f64()?;
                if b.abs() < 1e-15 {
                    return Err(PsError::new("undefinedresult", "Division by zero"));
                }
                self.operand_stack.push(PsValue::Real(a / b));
            }
            "idiv" => {
                let b = self.pop_i64()?;
                let a = self.pop_i64()?;
                if b == 0 {
                    return Err(PsError::new("undefinedresult", "Division by zero"));
                }
                self.operand_stack.push(PsValue::Integer(a / b));
            }
            "mod" => {
                let b = self.pop_i64()?;
                let a = self.pop_i64()?;
                if b == 0 {
                    return Err(PsError::new("undefinedresult", "Modulo by zero"));
                }
                self.operand_stack.push(PsValue::Integer(a % b));
            }
            "neg" => {
                let a = self.pop_f64()?;
                self.operand_stack.push(PsValue::Real(-a));
            }
            "abs" => {
                let a = self.pop_f64()?;
                self.operand_stack.push(PsValue::Real(a.abs()));
            }
            "ceiling" => {
                let a = self.pop_f64()?;
                self.operand_stack.push(PsValue::Real(a.ceil()));
            }
            "floor" => {
                let a = self.pop_f64()?;
                self.operand_stack.push(PsValue::Real(a.floor()));
            }
            "round" => {
                let a = self.pop_f64()?;
                self.operand_stack.push(PsValue::Real(a.round()));
            }
            "truncate" => {
                let a = self.pop_f64()?;
                self.operand_stack.push(PsValue::Real(a.trunc()));
            }
            "sqrt" => {
                let a = self.pop_f64()?;
                if a < 0.0 {
                    return Err(PsError::new("rangecheck", "sqrt of negative number"));
                }
                self.operand_stack.push(PsValue::Real(a.sqrt()));
            }
            "atan" => {
                let den = self.pop_f64()?;
                let num = self.pop_f64()?;
                let deg = num.atan2(den).to_degrees();
                let normalized = if deg < 0.0 { deg + 360.0 } else { deg };
                self.operand_stack.push(PsValue::Real(normalized));
            }
            "cos" => {
                let deg = self.pop_f64()?;
                self.operand_stack.push(PsValue::Real(deg.to_radians().cos()));
            }
            "sin" => {
                let deg = self.pop_f64()?;
                self.operand_stack.push(PsValue::Real(deg.to_radians().sin()));
            }
            "exp" => {
                let exp = self.pop_f64()?;
                let base = self.pop_f64()?;
                self.operand_stack.push(PsValue::Real(base.powf(exp)));
            }
            "ln" => {
                let a = self.pop_f64()?;
                self.operand_stack.push(PsValue::Real(a.ln()));
            }
            "log" => {
                let a = self.pop_f64()?;
                self.operand_stack.push(PsValue::Real(a.log10()));
            }

            // Boolean / Relational
            "eq" => {
                let b = self.pop()?;
                let a = self.pop()?;
                self.operand_stack.push(PsValue::Boolean(a == b));
            }
            "ne" => {
                let b = self.pop()?;
                let a = self.pop()?;
                self.operand_stack.push(PsValue::Boolean(a != b));
            }
            "ge" => {
                let b = self.pop_f64()?;
                let a = self.pop_f64()?;
                self.operand_stack.push(PsValue::Boolean(a >= b));
            }
            "gt" => {
                let b = self.pop_f64()?;
                let a = self.pop_f64()?;
                self.operand_stack.push(PsValue::Boolean(a > b));
            }
            "le" => {
                let b = self.pop_f64()?;
                let a = self.pop_f64()?;
                self.operand_stack.push(PsValue::Boolean(a <= b));
            }
            "lt" => {
                let b = self.pop_f64()?;
                let a = self.pop_f64()?;
                self.operand_stack.push(PsValue::Boolean(a < b));
            }
            "and" => {
                let b = self.pop_bool()?;
                let a = self.pop_bool()?;
                self.operand_stack.push(PsValue::Boolean(a && b));
            }
            "or" => {
                let b = self.pop_bool()?;
                let a = self.pop_bool()?;
                self.operand_stack.push(PsValue::Boolean(a || b));
            }
            "not" => {
                let a = self.pop_bool()?;
                self.operand_stack.push(PsValue::Boolean(!a));
            }
            "xor" => {
                let b = self.pop_bool()?;
                let a = self.pop_bool()?;
                self.operand_stack.push(PsValue::Boolean(a ^ b));
            }

            // Control flow
            "if" => {
                let proc = self.pop_procedure()?;
                let cond = self.pop_bool()?;
                if cond {
                    for val in proc {
                        self.eval_value(val)?;
                    }
                }
            }
            "ifelse" => {
                let proc2 = self.pop_procedure()?;
                let proc1 = self.pop_procedure()?;
                let cond = self.pop_bool()?;
                let target = if cond { proc1 } else { proc2 };
                for val in target {
                    self.eval_value(val)?;
                }
            }
            "for" => {
                let proc = self.pop_procedure()?;
                let limit = self.pop_f64()?;
                let step = self.pop_f64()?;
                let initial = self.pop_f64()?;

                let mut cur = initial;
                while (step > 0.0 && cur <= limit + 1e-9) || (step < 0.0 && cur >= limit - 1e-9) {
                    self.operand_stack.push(PsValue::Real(cur));
                    for val in &proc {
                        self.eval_value(val.clone())?;
                    }
                    cur += step;
                }
            }
            "repeat" => {
                let proc = self.pop_procedure()?;
                let count = self.pop_i64()?.max(0);
                for _ in 0..count {
                    for val in &proc {
                        self.eval_value(val.clone())?;
                    }
                }
            }

            // Dictionaries
            "dict" => {
                let _cap = self.pop_i64()?;
                self.operand_stack.push(PsValue::Dict(HashMap::new()));
            }
            "begin" => {
                let val = self.pop()?;
                if let PsValue::Dict(d) = val {
                    self.dict_stack.push(d);
                } else {
                    return Err(PsError::new("typecheck", "begin expects dict"));
                }
            }
            "end" => {
                if self.dict_stack.len() > 3 {
                    self.dict_stack.pop();
                }
            }
            "def" => {
                let val = self.pop()?;
                let key = self.pop()?.as_str().ok_or_else(|| {
                    PsError::new("typecheck", "def expects name key")
                })?.to_string();

                let current_dict = self.dict_stack.last_mut().unwrap();
                current_dict.insert(key, val);
            }
            "bind" => {
                // bind is an optimization operator; no-op in interpreter
            }

            // Arrays
            "array" => {
                let n = self.pop_i64()?.max(0) as usize;
                self.operand_stack.push(PsValue::Array(vec![PsValue::Null; n]));
            }
            "aload" => {
                let val = self.pop()?;
                if let PsValue::Array(arr) = val {
                    let clone = arr.clone();
                    for item in arr {
                        self.operand_stack.push(item);
                    }
                    self.operand_stack.push(PsValue::Array(clone));
                }
            }
            "astore" => {
                let val = self.pop()?;
                if let PsValue::Array(mut arr) = val {
                    for item in arr.iter_mut().rev() {
                        *item = self.pop()?;
                    }
                    self.operand_stack.push(PsValue::Array(arr));
                }
            }
            "length" => {
                let val = self.pop()?;
                match val {
                    PsValue::Array(a) => self.operand_stack.push(PsValue::Integer(a.len() as i64)),
                    PsValue::String(s) => self.operand_stack.push(PsValue::Integer(s.len() as i64)),
                    PsValue::Dict(d) => self.operand_stack.push(PsValue::Integer(d.len() as i64)),
                    _ => return Err(PsError::new("typecheck", "length expects array/string/dict")),
                }
            }

            // Coordinate system & Matrix
            "matrix" => {
                self.operand_stack.push(PsValue::Array(vec![
                    PsValue::Real(1.0),
                    PsValue::Real(0.0),
                    PsValue::Real(0.0),
                    PsValue::Real(1.0),
                    PsValue::Real(0.0),
                    PsValue::Real(0.0),
                ]));
            }
            "currentmatrix" => {
                let _ = self.pop()?; // array to receive matrix
                let ctm = self.gstate.ctm;
                self.operand_stack.push(PsValue::Array(vec![
                    PsValue::Real(ctm.a),
                    PsValue::Real(ctm.b),
                    PsValue::Real(ctm.c),
                    PsValue::Real(ctm.d),
                    PsValue::Real(ctm.tx),
                    PsValue::Real(ctm.ty),
                ]));
            }
            "setmatrix" => {
                let val = self.pop()?;
                if let PsValue::Array(a) = val {
                    if a.len() == 6 {
                        let m = Matrix {
                            a: a[0].to_f64().unwrap_or(1.0),
                            b: a[1].to_f64().unwrap_or(0.0),
                            c: a[2].to_f64().unwrap_or(0.0),
                            d: a[3].to_f64().unwrap_or(1.0),
                            tx: a[4].to_f64().unwrap_or(0.0),
                            ty: a[5].to_f64().unwrap_or(0.0),
                        };
                        self.gstate.ctm = m;
                    }
                }
            }
            "translate" => {
                let ty = self.pop_f64()?;
                let tx = self.pop_f64()?;
                let m = Matrix::translate(tx, ty);
                self.gstate.ctm = self.gstate.ctm.multiply(&m);
            }
            "scale" => {
                let sy = self.pop_f64()?;
                let sx = self.pop_f64()?;
                let m = Matrix::scale(sx, sy);
                self.gstate.ctm = self.gstate.ctm.multiply(&m);
            }
            "rotate" => {
                let deg = self.pop_f64()?;
                let m = Matrix::rotate(deg);
                self.gstate.ctm = self.gstate.ctm.multiply(&m);
            }
            "concat" => {
                let val = self.pop()?;
                if let PsValue::Array(a) = val {
                    if a.len() == 6 {
                        let m = Matrix {
                            a: a[0].to_f64().unwrap_or(1.0),
                            b: a[1].to_f64().unwrap_or(0.0),
                            c: a[2].to_f64().unwrap_or(0.0),
                            d: a[3].to_f64().unwrap_or(1.0),
                            tx: a[4].to_f64().unwrap_or(0.0),
                            ty: a[5].to_f64().unwrap_or(0.0),
                        };
                        self.gstate.ctm = self.gstate.ctm.multiply(&m);
                    }
                }
            }

            // Path construction
            "newpath" => {
                self.gstate.path.clear();
                self.gstate.current_point = None;
            }
            "currentpoint" => {
                if let Some((x, y)) = self.gstate.current_point {
                    let inv = self.gstate.ctm.invert().unwrap_or(Matrix::IDENTITY);
                    let (ux, uy) = inv.transform_point(x, y);
                    self.operand_stack.push(PsValue::Real(ux));
                    self.operand_stack.push(PsValue::Real(uy));
                } else {
                    return Err(PsError::new("nocurrentpoint", "currentpoint"));
                }
            }
            "moveto" => {
                let y = self.pop_f64()?;
                let x = self.pop_f64()?;
                let pt = self.gstate.ctm.transform_point(x, y);
                self.gstate.path.push(PathOp::MoveTo(pt.0, pt.1));
                self.gstate.current_point = Some(pt);
            }
            "rmoveto" => {
                let dy = self.pop_f64()?;
                let dx = self.pop_f64()?;
                if let Some((cx, cy)) = self.gstate.current_point {
                    let (tx, ty) = self.gstate.ctm.transform_delta(dx, dy);
                    let pt = (cx + tx, cy + ty);
                    self.gstate.path.push(PathOp::MoveTo(pt.0, pt.1));
                    self.gstate.current_point = Some(pt);
                } else {
                    return Err(PsError::new("nocurrentpoint", "rmoveto"));
                }
            }
            "lineto" => {
                let y = self.pop_f64()?;
                let x = self.pop_f64()?;
                let pt = self.gstate.ctm.transform_point(x, y);
                self.gstate.path.push(PathOp::LineTo(pt.0, pt.1));
                self.gstate.current_point = Some(pt);
            }
            "rlineto" => {
                let dy = self.pop_f64()?;
                let dx = self.pop_f64()?;
                if let Some((cx, cy)) = self.gstate.current_point {
                    let (tx, ty) = self.gstate.ctm.transform_delta(dx, dy);
                    let pt = (cx + tx, cy + ty);
                    self.gstate.path.push(PathOp::LineTo(pt.0, pt.1));
                    self.gstate.current_point = Some(pt);
                } else {
                    return Err(PsError::new("nocurrentpoint", "rlineto"));
                }
            }
            "curveto" => {
                let y3 = self.pop_f64()?;
                let x3 = self.pop_f64()?;
                let y2 = self.pop_f64()?;
                let x2 = self.pop_f64()?;
                let y1 = self.pop_f64()?;
                let x1 = self.pop_f64()?;

                let p1 = self.gstate.ctm.transform_point(x1, y1);
                let p2 = self.gstate.ctm.transform_point(x2, y2);
                let p3 = self.gstate.ctm.transform_point(x3, y3);

                self.gstate.path.push(PathOp::CurveTo(p1.0, p1.1, p2.0, p2.1, p3.0, p3.1));
                self.gstate.current_point = Some(p3);
            }
            "arc" | "arcn" => {
                let ang2 = self.pop_f64()?;
                let ang1 = self.pop_f64()?;
                let r = self.pop_f64()?;
                let cy = self.pop_f64()?;
                let cx = self.pop_f64()?;

                self.append_arc(cx, cy, r, ang1, ang2, op == "arcn");
            }
            "closepath" => {
                self.gstate.path.push(PathOp::Close);
            }

            // Painting & PDF stream generation
            "stroke" => {
                self.emit_path();
                self.emit_stroke();
                self.gstate.path.clear();
            }
            "fill" => {
                self.emit_path();
                self.emit_fill(false);
                self.gstate.path.clear();
            }
            "eofill" => {
                self.emit_path();
                self.emit_fill(true);
                self.gstate.path.clear();
            }
            "clip" => {
                self.emit_path();
                self.pdf_stream.extend_from_slice(b"W n\n");
            }
            "eoclip" => {
                self.emit_path();
                self.pdf_stream.extend_from_slice(b"W* n\n");
            }

            // Graphics state
            "gsave" => {
                self.gstate_stack.push(self.gstate.clone());
                self.pdf_stream.extend_from_slice(b"q\n");
            }
            "grestore" => {
                if let Some(saved) = self.gstate_stack.pop() {
                    self.gstate = saved;
                    self.pdf_stream.extend_from_slice(b"Q\n");
                }
            }
            "setlinewidth" => {
                let w = self.pop_f64()?.max(0.0);
                self.gstate.line_width = w;
                let _ = writeln!(self.pdf_stream, "{:.4} w", w);
            }
            "setlinecap" => {
                let cap = self.pop_i64()?.clamp(0, 2) as u8;
                self.gstate.line_cap = cap;
                let _ = writeln!(self.pdf_stream, "{} J", cap);
            }
            "setlinejoin" => {
                let join = self.pop_i64()?.clamp(0, 2) as u8;
                self.gstate.line_join = join;
                let _ = writeln!(self.pdf_stream, "{} j", join);
            }
            "setmiterlimit" => {
                let m = self.pop_f64()?.max(1.0);
                self.gstate.miter_limit = m;
                let _ = writeln!(self.pdf_stream, "{:.4} M", m);
            }
            "setdash" => {
                let offset = self.pop_f64()?;
                let val = self.pop()?;
                if let PsValue::Array(a) = val {
                    let mut pat = Vec::new();
                    for item in a {
                        if let Some(f) = item.to_f64() {
                            pat.push(f);
                        }
                    }
                    let pat_str = pat.iter().map(|v| format!("{v:.2}")).collect::<Vec<_>>().join(" ");
                    let _ = writeln!(self.pdf_stream, "[{pat_str}] {:.2} d", offset);
                    self.gstate.dash = Some((pat, offset));
                }
            }
            "setgray" => {
                let g = self.pop_f64()?.clamp(0.0, 1.0);
                self.gstate.color = PsColor::Gray(g);
                let _ = writeln!(self.pdf_stream, "{:.4} g {:.4} G", g, g);
            }
            "setrgbcolor" => {
                let b = self.pop_f64()?.clamp(0.0, 1.0);
                let g = self.pop_f64()?.clamp(0.0, 1.0);
                let r = self.pop_f64()?.clamp(0.0, 1.0);
                self.gstate.color = PsColor::Rgb(r, g, b);
                let _ = writeln!(
                    self.pdf_stream,
                    "{:.4} {:.4} {:.4} rg {:.4} {:.4} {:.4} RG",
                    r, g, b, r, g, b
                );
            }
            "setcmykcolor" => {
                let k = self.pop_f64()?.clamp(0.0, 1.0);
                let y = self.pop_f64()?.clamp(0.0, 1.0);
                let m = self.pop_f64()?.clamp(0.0, 1.0);
                let c = self.pop_f64()?.clamp(0.0, 1.0);
                self.gstate.color = PsColor::Cmyk(c, m, y, k);
                let _ = writeln!(
                    self.pdf_stream,
                    "{:.4} {:.4} {:.4} {:.4} k {:.4} {:.4} {:.4} {:.4} K",
                    c, m, y, k, c, m, y, k
                );
            }

            // Ignored / benign Level 2 ops
            "showpage" | "copypage" | "erasepage" | "initgraphics" => {}
            _ => {
                // Tolerant fallback for unknown operator names in typical EPS prologues
            }
        }

        Ok(())
    }

    fn emit_path(&mut self) {
        for op in &self.gstate.path {
            match op {
                PathOp::MoveTo(x, y) => {
                    let _ = writeln!(self.pdf_stream, "{:.4} {:.4} m", x, y);
                }
                PathOp::LineTo(x, y) => {
                    let _ = writeln!(self.pdf_stream, "{:.4} {:.4} l", x, y);
                }
                PathOp::CurveTo(x1, y1, x2, y2, x3, y3) => {
                    let _ = writeln!(
                        self.pdf_stream,
                        "{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} c",
                        x1, y1, x2, y2, x3, y3
                    );
                }
                PathOp::Close => {
                    let _ = writeln!(self.pdf_stream, "h");
                }
            }
        }
    }

    fn emit_stroke(&mut self) {
        self.pdf_stream.extend_from_slice(b"S\n");
    }

    fn emit_fill(&mut self, even_odd: bool) {
        if even_odd {
            self.pdf_stream.extend_from_slice(b"f*\n");
        } else {
            self.pdf_stream.extend_from_slice(b"f\n");
        }
    }

    fn append_arc(&mut self, cx: f64, cy: f64, r: f64, ang1: f64, mut ang2: f64, cw: bool) {
        // Approximate circle arc with cubic Bézier segments (<= 90 degrees each)
        if !cw {
            while ang2 < ang1 {
                ang2 += 360.0;
            }
        } else {
            while ang2 > ang1 {
                ang2 -= 360.0;
            }
        }

        let total_sweep = (ang2 - ang1).abs();
        let num_segments = ((total_sweep / 90.0).ceil() as usize).max(1);
        let sweep_per_seg = (ang2 - ang1) / (num_segments as f64);

        let mut cur_ang = ang1;
        for i in 0..num_segments {
            let next_ang = cur_ang + sweep_per_seg;
            let theta = sweep_per_seg.to_radians();
            let k = (4.0 / 3.0) * ((theta * 0.25).tan());

            let rad1 = cur_ang.to_radians();
            let rad2 = next_ang.to_radians();

            let cos1 = rad1.cos();
            let sin1 = rad1.sin();
            let cos2 = rad2.cos();
            let sin2 = rad2.sin();

            let p0 = (cx + r * cos1, cy + r * sin1);
            let p3 = (cx + r * cos2, cy + r * sin2);

            let p1 = (p0.0 - k * r * sin1, p0.1 + k * r * cos1);
            let p2 = (p3.0 + k * r * sin2, p3.1 - k * r * cos2);

            let pt0 = self.gstate.ctm.transform_point(p0.0, p0.1);
            let pt1 = self.gstate.ctm.transform_point(p1.0, p1.1);
            let pt2 = self.gstate.ctm.transform_point(p2.0, p2.1);
            let pt3 = self.gstate.ctm.transform_point(p3.0, p3.1);

            if i == 0 && self.gstate.current_point.is_none() {
                self.gstate.path.push(PathOp::MoveTo(pt0.0, pt0.1));
            } else if i == 0 {
                self.gstate.path.push(PathOp::LineTo(pt0.0, pt0.1));
            }

            self.gstate.path.push(PathOp::CurveTo(pt1.0, pt1.1, pt2.0, pt2.1, pt3.0, pt3.1));
            self.gstate.current_point = Some(pt3);

            cur_ang = next_ang;
        }
    }

    fn pop(&mut self) -> Result<PsValue, PsError> {
        self.operand_stack
            .pop()
            .ok_or_else(|| PsError::new("stackunderflow", "pop"))
    }

    fn peek(&self) -> Result<&PsValue, PsError> {
        self.operand_stack
            .last()
            .ok_or_else(|| PsError::new("stackunderflow", "peek"))
    }

    fn pop_f64(&mut self) -> Result<f64, PsError> {
        let v = self.pop()?;
        v.to_f64()
            .ok_or_else(|| PsError::new("typecheck", "Expected number"))
    }

    fn pop_i64(&mut self) -> Result<i64, PsError> {
        let v = self.pop()?;
        v.to_i64()
            .ok_or_else(|| PsError::new("typecheck", "Expected integer"))
    }

    fn pop_bool(&mut self) -> Result<bool, PsError> {
        let v = self.pop()?;
        v.to_bool()
            .ok_or_else(|| PsError::new("typecheck", "Expected boolean"))
    }

    fn pop_procedure(&mut self) -> Result<Vec<PsValue>, PsError> {
        let v = self.pop()?;
        if let PsValue::Procedure(p) = v {
            Ok(p)
        } else {
            Err(PsError::new("typecheck", "Expected procedure"))
        }
    }
}
