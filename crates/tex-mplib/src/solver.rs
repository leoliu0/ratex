//! MetaPost linear equation solver for scalar and pair unknowns.

use std::collections::HashMap;

/// Formats a number like MetaPost's `print_number` (at most 5 decimals).
pub(crate) fn fmt_number(x: f64) -> String {
    let s = format!("{x:.5}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

/// A linear expression: c + sum_i (a_i * x_i)
#[derive(Clone, Debug, PartialEq)]
pub struct LinearExpr {
    pub constant: f64,
    pub terms: HashMap<usize, f64>,
}

impl LinearExpr {
    pub fn constant(c: f64) -> Self {
        Self {
            constant: c,
            terms: HashMap::new(),
        }
    }

    pub fn variable(var_id: usize) -> Self {
        let mut terms = HashMap::new();
        terms.insert(var_id, 1.0);
        Self {
            constant: 0.0,
            terms,
        }
    }

    pub fn is_known(&self) -> bool {
        self.terms.is_empty()
    }

    pub fn as_known(&self) -> Option<f64> {
        if self.is_known() {
            Some(self.constant)
        } else {
            None
        }
    }

    pub fn add(&self, other: &Self) -> Self {
        let mut terms = self.terms.clone();
        for (&var, &coeff) in &other.terms {
            *terms.entry(var).or_insert(0.0) += coeff;
        }
        terms.retain(|_, v| v.abs() > 1e-12);
        Self {
            constant: self.constant + other.constant,
            terms,
        }
    }

    pub fn sub(&self, other: &Self) -> Self {
        let mut terms = self.terms.clone();
        for (&var, &coeff) in &other.terms {
            *terms.entry(var).or_insert(0.0) -= coeff;
        }
        terms.retain(|_, v| v.abs() > 1e-12);
        Self {
            constant: self.constant - other.constant,
            terms,
        }
    }

    pub fn mul_scalar(&self, s: f64) -> Self {
        if s == 0.0 {
            return Self::constant(0.0);
        }
        let terms = self
            .terms
            .iter()
            .map(|(&var, &coeff)| (var, coeff * s))
            .collect();
        Self {
            constant: self.constant * s,
            terms,
        }
    }

    /// Substitute `var_id` with `replacement`.
    pub fn substitute(&mut self, var_id: usize, replacement: &LinearExpr) {
        if let Some(coeff) = self.terms.remove(&var_id) {
            self.constant += coeff * replacement.constant;
            for (&v, &c) in &replacement.terms {
                *self.terms.entry(v).or_insert(0.0) += coeff * c;
            }
            self.terms.retain(|_, v| v.abs() > 1e-12);
        }
    }
}

/// The linear dependency solver maintaining variable definitions.
#[derive(Default, Debug)]
pub struct LinearSolver {
    next_var_id: usize,
    /// Maps variable ID to its current simplified expression.
    pub vars: HashMap<usize, LinearExpr>,
    /// Name to variable ID mapping.
    pub name_to_id: HashMap<String, usize>,
}

impl LinearSolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_var(&mut self, name: Option<&str>) -> usize {
        let id = self.next_var_id;
        self.next_var_id += 1;
        self.vars.insert(id, LinearExpr::variable(id));
        if let Some(n) = name {
            self.name_to_id.insert(n.to_string(), id);
        }
        id
    }

    pub fn get_var_by_name(&mut self, name: &str) -> usize {
        if let Some(&id) = self.name_to_id.get(name) {
            id
        } else {
            self.new_var(Some(name))
        }
    }

    pub fn get_expr(&self, var_id: usize) -> LinearExpr {
        self.vars
            .get(&var_id)
            .cloned()
            .unwrap_or_else(|| LinearExpr::variable(var_id))
    }

    pub fn resolve(&self, expr: &LinearExpr) -> LinearExpr {
        let mut res = LinearExpr::constant(expr.constant);
        for (&var, &coeff) in &expr.terms {
            let var_expr = self.get_expr(var);
            res = res.add(&var_expr.mul_scalar(coeff));
        }
        res
    }

    /// Asserts that `e1 = e2`.
    /// Returns Ok(()) if consistent (or redundant), Err(message) if inconsistent.
    pub fn equate(&mut self, e1: &LinearExpr, e2: &LinearExpr) -> Result<(), String> {
        let resolved_e1 = self.resolve(e1);
        let resolved_e2 = self.resolve(e2);
        let diff = resolved_e1.sub(&resolved_e2);

        if diff.terms.is_empty() {
            // MetaPost (scaled) treats differences up to 64/65536 as redundant.
            if diff.constant.abs() <= 64.0 / 65536.0 {
                return Ok(());
            }
            return Err(format!(
                "Inconsistent equation (off by {})",
                fmt_number(-diff.constant)
            ));
        }

        // Choose pivot with largest absolute coefficient (lowest id on ties).
        let (pivot_var, pivot_coeff) = diff
            .terms
            .iter()
            .map(|(&var, &coeff)| (var, coeff))
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()).then(b.0.cmp(&a.0)))
            .expect("non-empty terms");

        // Solve for pivot_var:
        // pivot_coeff * pivot_var + other_terms + c = 0
        // => pivot_var = -(c + other_terms) / pivot_coeff
        let mut subst = LinearExpr::constant(-diff.constant / pivot_coeff);
        for (&var, &coeff) in &diff.terms {
            if var != pivot_var {
                *subst.terms.entry(var).or_insert(0.0) -= coeff / pivot_coeff;
            }
        }
        subst.terms.retain(|_, v| v.abs() > 1e-12);

        // Substitute into all existing variables
        for expr in self.vars.values_mut() {
            expr.substitute(pivot_var, &subst);
        }

        self.vars.insert(pivot_var, subst);
        Ok(())
    }
}
