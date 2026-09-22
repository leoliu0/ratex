//! John Hobby's smooth interpolating spline algorithm for MetaPost paths.

use crate::types::{Knot, Pair, Path};

/// Knot specification before control points are resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct KnotSpec {
    pub p: Pair,
    /// Explicit outgoing direction in degrees (None = auto).
    pub dir_out: Option<f64>,
    /// Explicit incoming direction in degrees (None = auto).
    pub dir_in: Option<f64>,
    /// Outgoing tension (default 1.0).
    pub tension_out: f64,
    /// Incoming tension (default 1.0).
    pub tension_in: f64,
    /// Curl at start/end (default 1.0).
    pub curl: f64,
}

impl KnotSpec {
    pub fn new(p: Pair) -> Self {
        Self {
            p,
            dir_out: None,
            dir_in: None,
            tension_out: 1.0,
            tension_in: 1.0,
            curl: 1.0,
        }
    }

    pub fn with_dir(p: Pair, dir_deg: f64) -> Self {
        Self {
            p,
            dir_out: Some(dir_deg),
            dir_in: Some(dir_deg),
            tension_out: 1.0,
            tension_in: 1.0,
            curl: 1.0,
        }
    }
}

/// John Hobby's velocity function rho(theta, phi).
fn hobby_rho(theta: f64, phi: f64) -> f64 {
    let st = theta.sin();
    let ct = theta.cos();
    let sp = phi.sin();
    let cp = phi.cos();

    let a = 2.0_f64.sqrt();
    let alpha = a * (st - sp / 16.0) * (sp - st / 16.0) * (ct - cp);
    let num = 2.0 + alpha;
    let den = 1.0 + 0.5 * (5.0_f64.sqrt() - 1.0) * ct + 0.5 * (3.0 - 5.0_f64.sqrt()) * cp;
    num / (3.0 * den)
}

/// Computes the smooth Bézier path passing through knot specs using Hobby's algorithm.
pub fn solve_path(specs: &[KnotSpec], closed: bool) -> Path {
    let n = specs.len();
    if n == 0 {
        return Path::empty();
    }
    if n == 1 {
        return Path {
            knots: vec![Knot::new(specs[0].p)],
            closed,
        };
    }
    if n == 2 && !closed {
        // Single segment
        let p0 = specs[0].p;
        let p1 = specs[1].p;
        let d = (p1 - p0).len();
        if d < 1e-12 {
            return Path {
                knots: vec![Knot::new(p0), Knot::new(p1)],
                closed: false,
            };
        }

        let chord_angle = (p1 - p0).angle_rad();
        let theta = specs[0].dir_out.map_or(0.0, |d| d.to_radians() - chord_angle);
        let phi = specs[1].dir_in.map_or(0.0, |d| chord_angle - d.to_radians());

        let rho = hobby_rho(theta, phi);
        let sigma = hobby_rho(phi, theta);

        let r_u = (d / (3.0 * specs[0].tension_out)) * rho;
        let r_v = (d / (3.0 * specs[1].tension_in)) * sigma;

        let u = p0 + Pair::from_polar(r_u, (chord_angle + theta).to_degrees());
        let v = p1 - Pair::from_polar(r_v, (chord_angle - phi).to_degrees());

        return Path {
            knots: vec![
                Knot::with_controls(p0, p0, u),
                Knot::with_controls(v, p1, p1),
            ],
            closed: false,
        };
    }

    let m = if closed { n } else { n - 1 };
    let mut chord_len = vec![0.0; m];
    let mut chord_ang = vec![0.0; m];

    for i in 0..m {
        let next_i = (i + 1) % n;
        let delta = specs[next_i].p - specs[i].p;
        chord_len[i] = delta.len();
        chord_ang[i] = delta.angle_rad();
    }

    // Turning angles psi between consecutive chords
    let mut psi = vec![0.0; n];
    for i in 1..m {
        let mut diff = chord_ang[i] - chord_ang[i - 1];
        while diff > std::f64::consts::PI {
            diff -= 2.0 * std::f64::consts::PI;
        }
        while diff < -std::f64::consts::PI {
            diff += 2.0 * std::f64::consts::PI;
        }
        psi[i] = diff;
    }
    if closed {
        let mut diff = chord_ang[0] - chord_ang[m - 1];
        while diff > std::f64::consts::PI {
            diff -= 2.0 * std::f64::consts::PI;
        }
        while diff < -std::f64::consts::PI {
            diff += 2.0 * std::f64::consts::PI;
        }
        psi[0] = diff;
    }

    // Solve tridiagonal system for theta
    let mut theta = vec![0.0; n];
    let mut phi = vec![0.0; n];

    if !closed {
        // Open curve tridiagonal system:
        // A[i] * theta[i-1] + B[i] * theta[i] + C[i] * theta[i+1] = D[i]
        let mut a = vec![0.0; n];
        let mut b = vec![0.0; n];
        let mut c = vec![0.0; n];
        let mut d = vec![0.0; n];

        // Boundary at i = 0
        if let Some(dir) = specs[0].dir_out {
            let th = dir.to_radians() - chord_ang[0];
            b[0] = 1.0;
            d[0] = th;
        } else {
            let curl = specs[0].curl;
            let alpha = 1.0 / specs[0].tension_out;
            let beta = 1.0 / specs[1].tension_in;
            let c0 = (alpha * alpha * curl) / (beta * beta);
            b[0] = c0 * alpha + 3.0 - beta;
            c[0] = (3.0 - alpha) * c0 + beta;
            d[0] = -c[0] * psi[1];
        }

        // Interior nodes
        for i in 1..n - 1 {
            if let Some(dir) = specs[i].dir_out {
                b[i] = 1.0;
                d[i] = dir.to_radians() - chord_ang[i];
                continue;
            }

            let d_prev = chord_len[i - 1].max(1e-6);
            let d_curr = chord_len[i].max(1e-6);

            let alpha = 1.0 / specs[i - 1].tension_out;
            let beta = 1.0 / specs[i].tension_in;
            let gamma = 1.0 / specs[i].tension_out;
            let delta = 1.0 / specs[i + 1].tension_in;

            let coeff_a = alpha / (beta * beta * d_prev);
            let coeff_b = (3.0 - alpha) / (beta * beta * d_prev);
            let coeff_c = (3.0 - delta) / (gamma * gamma * d_curr);
            let coeff_d = delta / (gamma * gamma * d_curr);

            a[i] = coeff_a;
            b[i] = coeff_b + coeff_c;
            c[i] = coeff_d;
            d[i] = -coeff_b * psi[i] - coeff_d * psi[i + 1].min(std::f64::consts::PI);
        }

        // Boundary at i = n - 1
        let last = n - 1;
        if let Some(dir) = specs[last].dir_in {
            let ph = chord_ang[last - 1] - dir.to_radians();
            b[last] = 1.0;
            d[last] = -ph - psi[last - 1];
        } else {
            let curl = specs[last].curl;
            let alpha = 1.0 / specs[last - 1].tension_out;
            let beta = 1.0 / specs[last].tension_in;
            let cn = (beta * beta * curl) / (alpha * alpha);
            a[last] = (3.0 - beta) * cn + alpha;
            b[last] = cn * beta + 3.0 - alpha;
            d[last] = 0.0;
        }

        // Forward elimination
        for i in 1..n {
            if b[i - 1].abs() > 1e-12 {
                let factor = a[i] / b[i - 1];
                b[i] -= factor * c[i - 1];
                d[i] -= factor * d[i - 1];
            }
        }

        // Back substitution
        if b[last].abs() > 1e-12 {
            theta[last] = d[last] / b[last];
        }
        for i in (0..last).rev() {
            if b[i].abs() > 1e-12 {
                theta[i] = (d[i] - c[i] * theta[i + 1]) / b[i];
            }
        }

        for i in 0..m {
            phi[i + 1] = -psi[i + 1] - theta[i + 1];
        }
    } else {
        // Closed curve: iterative relaxation (converges rapidly in 3-5 iterations)
        for _iter in 0..10 {
            for i in 0..n {
                let prev_i = if i == 0 { n - 1 } else { i - 1 };
                let next_i = (i + 1) % n;

                let d_prev = chord_len[prev_i].max(1e-6);
                let d_curr = chord_len[i].max(1e-6);

                let alpha = 1.0 / specs[prev_i].tension_out;
                let beta = 1.0 / specs[i].tension_in;
                let gamma = 1.0 / specs[i].tension_out;
                let delta = 1.0 / specs[next_i].tension_in;

                let coeff_a = alpha / (beta * beta * d_prev);
                let coeff_b = (3.0 - alpha) / (beta * beta * d_prev);
                let coeff_c = (3.0 - delta) / (gamma * gamma * d_curr);
                let coeff_d = delta / (gamma * gamma * d_curr);

                let rhs = -coeff_b * psi[i] - coeff_d * psi[next_i]
                    - coeff_a * theta[prev_i]
                    - coeff_d * theta[next_i];
                let denom = coeff_b + coeff_c;
                if denom.abs() > 1e-12 {
                    theta[i] = rhs / denom;
                }
            }
        }
        for i in 0..n {
            let next_i = (i + 1) % n;
            phi[next_i] = -psi[next_i] - theta[next_i];
        }
    }

    // Build knots with control points
    let mut knots = Vec::with_capacity(n);
    for i in 0..n {
        knots.push(Knot::new(specs[i].p));
    }

    for i in 0..m {
        let next_i = (i + 1) % n;
        let p0 = specs[i].p;
        let p1 = specs[next_i].p;
        let d = chord_len[i];
        if d < 1e-12 {
            knots[i].right_control = p0;
            knots[next_i].left_control = p1;
            continue;
        }

        let th = theta[i];
        let ph = phi[next_i];

        let rho = hobby_rho(th, ph);
        let sigma = hobby_rho(ph, th);

        let r_u = (d / (3.0 * specs[i].tension_out)) * rho;
        let r_v = (d / (3.0 * specs[next_i].tension_in)) * sigma;

        let ang = chord_ang[i];
        let u = p0 + Pair::from_polar(r_u, (ang + th).to_degrees());
        let v = p1 - Pair::from_polar(r_v, (ang - ph).to_degrees());

        knots[i].right_control = u;
        knots[next_i].left_control = v;
    }

    Path { knots, closed }
}
