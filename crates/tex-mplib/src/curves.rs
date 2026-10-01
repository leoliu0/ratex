//! John Hobby's smooth interpolating spline algorithm for MetaPost paths
//! (`make_choices` / `solve_choices` / `set_controls` in mp.w).

use std::f64::consts::PI;

use crate::types::{Knot, Pair, Path};

/// Constraint on one side of a knot before control points are chosen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KnotSide {
    /// No constraint: the direction is chosen by Hobby's algorithm.
    Open,
    /// `{curl c}`.
    Curl(f64),
    /// `{dir d}`: direction of travel in degrees.
    Given(f64),
}

/// Knot specification before control points are resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct KnotSpec {
    pub p: Pair,
    /// Constraint on the incoming side.
    pub left: KnotSide,
    /// Constraint on the outgoing side.
    pub right: KnotSide,
    /// Tension on the incoming side; negative means `tension atleast |t|`.
    pub left_tension: f64,
    /// Tension on the outgoing side; negative means `tension atleast |t|`.
    pub right_tension: f64,
}

impl KnotSpec {
    pub fn new(p: Pair) -> Self {
        Self {
            p,
            left: KnotSide::Open,
            right: KnotSide::Open,
            left_tension: 1.0,
            right_tension: 1.0,
        }
    }
}

/// Knot side types of mp.w, with given directions in radians.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Side {
    Endpoint,
    Explicit,
    Given(f64),
    Curl(f64),
    Open,
}

impl From<KnotSide> for Side {
    fn from(s: KnotSide) -> Self {
        match s {
            KnotSide::Open => Self::Open,
            KnotSide::Curl(c) => Self::Curl(c),
            KnotSide::Given(d) => Self::Given(normalize_angle(d.to_radians())),
        }
    }
}

/// Boundary condition of a run between two breakpoints.
enum Boundary {
    Given(f64),
    Curl(f64),
}

impl Side {
    fn boundary(self) -> Boundary {
        match self {
            Self::Given(g) => Boundary::Given(g),
            Self::Curl(c) => Boundary::Curl(c),
            // Runs start and end only at knots whose side is given or curl:
            // one-sided constraints are copied to the other side, open path
            // ends become curls and zero-length segments make their open
            // neighbours curls.
            _ => unreachable!("run boundary must be given or curl, got {self:?}"),
        }
    }
}

/// `reduce_angle`: brings an angle difference into [-pi, pi].
fn reduce_angle(a: f64) -> f64 {
    if a > PI {
        a - 2.0 * PI
    } else if a < -PI {
        a + 2.0 * PI
    } else {
        a
    }
}

/// Normalizes an arbitrary angle into (-pi, pi] like `n_arg`.
fn normalize_angle(a: f64) -> f64 {
    let r = a - 2.0 * PI * (a / (2.0 * PI)).round();
    if r <= -PI {
        r + 2.0 * PI
    } else {
        r
    }
}

/// `curl_ratio(gamma, a_tension, b_tension)`, capped at 4.
fn curl_ratio(gamma: f64, a_tension: f64, b_tension: f64) -> f64 {
    let alpha = 1.0 / a_tension;
    let beta = 1.0 / b_tension;
    let num = (3.0 - alpha) * alpha * alpha * gamma + beta * beta * beta;
    let den = alpha * alpha * alpha * gamma + (3.0 - beta) * beta * beta;
    if num >= 4.0 * den {
        4.0
    } else {
        num / den
    }
}

/// Hobby's velocity function, divided by the tension and capped at 4.
fn velocity(st: f64, ct: f64, sf: f64, cf: f64, tension: f64) -> f64 {
    // mp.w's 28-bit fixed-point constants for sqrt(2), 3(sqrt(5)-1)/2 and 3(3-sqrt(5))/2.
    const SQRT2: f64 = 379625062.0 / 268435456.0;
    const CT_COEFF: f64 = 497706707.0 / 268435456.0;
    const CF_COEFF: f64 = 307599661.0 / 268435456.0;
    let acc = (st - sf / 16.0) * (sf - st / 16.0) * (ct - cf);
    let num = (2.0 + SQRT2 * acc) / tension;
    let denom = 3.0 + CT_COEFF * ct + CF_COEFF * cf;
    if num / 4.0 >= denom {
        4.0
    } else {
        num / denom
    }
}

/// Solves `a[k] x[k-1] + b[k] x[k] + c[k] x[k+1] = d[k]` (Thomas algorithm).
fn solve_tridiagonal(a: &[f64], b: &[f64], c: &[f64], d: &[f64]) -> Vec<f64> {
    let n = b.len();
    let mut cp = vec![0.0; n];
    let mut dp = vec![0.0; n];
    for k in 0..n {
        let (sub_c, sub_d) = if k > 0 { (a[k] * cp[k - 1], a[k] * dp[k - 1]) } else { (0.0, 0.0) };
        let denom = b[k] - sub_c;
        cp[k] = c[k] / denom;
        dp[k] = (d[k] - sub_d) / denom;
    }
    let mut x = dp;
    for k in (0..n - 1).rev() {
        x[k] -= cp[k] * x[k + 1];
    }
    x
}

/// Solves the cyclic system `a[k] x[k-1] + b[k] x[k] + c[k] x[k+1] = d[k]`
/// with indices taken modulo `n >= 2`, exactly.
fn solve_cyclic(a: &[f64], b: &[f64], c: &[f64], d: &[f64]) -> Vec<f64> {
    let n = b.len();
    // Eliminate rows 1..n with x[0] kept symbolic: x[k] = p[k] + q[k] x[0].
    let mut cp = vec![0.0; n];
    let mut dc = vec![0.0; n];
    let mut dq = vec![0.0; n];
    for k in 1..n {
        let mut x0_coeff = 0.0;
        if k == 1 {
            x0_coeff += a[1];
        }
        if k == n - 1 {
            x0_coeff += c[n - 1];
        }
        let (sub_c, sub_d, sub_q) = if k > 1 {
            (a[k] * cp[k - 1], a[k] * dc[k - 1], a[k] * dq[k - 1])
        } else {
            (0.0, 0.0, 0.0)
        };
        let denom = b[k] - sub_c;
        cp[k] = if k < n - 1 { c[k] / denom } else { 0.0 };
        dc[k] = (d[k] - sub_d) / denom;
        dq[k] = (-x0_coeff - sub_q) / denom;
    }
    for k in (1..n - 1).rev() {
        dc[k] -= cp[k] * dc[k + 1];
        dq[k] -= cp[k] * dq[k + 1];
    }
    let coeff = b[0] + a[0] * dq[n - 1] + c[0] * dq[1];
    let x0 = (d[0] - a[0] * dc[n - 1] - c[0] * dc[1]) / coeff;
    let mut x = vec![x0; n];
    for k in 1..n {
        x[k] = dc[k] + dq[k] * x0;
    }
    x
}

/// Coefficients of the mock-curvature equation at an interior knot `s` with
/// predecessor `r` and successor `t`:
/// `A theta[k-1] + (B + C) theta[k] + D theta[k+1] = -B psi[k] - D psi[k+1]`.
fn mock_curvature_row(
    r: &KnotSpec,
    s: &KnotSpec,
    t: &KnotSpec,
    dist_prev: f64,
    dist_next: f64,
    psi_k: f64,
    psi_next: f64,
) -> (f64, f64, f64, f64) {
    let alpha_r = 1.0 / r.right_tension.abs();
    let beta_s = 1.0 / s.left_tension.abs();
    let alpha_s = 1.0 / s.right_tension.abs();
    let beta_t = 1.0 / t.left_tension.abs();
    let aa = alpha_r / (beta_s * beta_s * dist_prev);
    let bb = (3.0 - alpha_r) / (beta_s * beta_s * dist_prev);
    let cc = (3.0 - beta_t) / (alpha_s * alpha_s * dist_next);
    let dd = beta_t / (alpha_s * alpha_s * dist_next);
    (aa, bb + cc, dd, -bb * psi_k - dd * psi_next)
}

/// Turning angle at a knot from chord `prev` to chord `next` (`n_arg` of the
/// rotated chord, as computed in mp.w).
fn turning_angle(prev: Pair, next: Pair) -> f64 {
    let len = prev.len();
    let (sine, cosine) = (prev.y / len, prev.x / len);
    (next.y * cosine - next.x * sine).atan2(next.x * cosine + next.y * sine)
}

/// `set_controls`: places the control points of the segment `s -> t` with
/// chord `delta`, given the angles `theta` (at `s`) and `phi` (at `t`).
fn set_controls(s: &KnotSpec, t: &KnotSpec, delta: Pair, theta: f64, phi: f64) -> (Pair, Pair) {
    let (st, ct) = theta.sin_cos();
    let (sf, cf) = phi.sin_cos();
    let mut rr = velocity(st, ct, sf, cf, s.right_tension.abs());
    let mut ss = velocity(sf, cf, st, ct, t.left_tension.abs());
    // `tension atleast`: keep the control points inside the triangle formed
    // by the chord and the two tangents.
    if (s.right_tension < 0.0 || t.left_tension < 0.0)
        && ((st >= 0.0 && sf >= 0.0) || (st <= 0.0 && sf <= 0.0))
    {
        let sine = st.abs() * cf + sf.abs() * ct;
        if sine > 0.0 {
            let sine = sine * (1.0 + 1.0 / 65536.0);
            if s.right_tension < 0.0 && sf.abs() < rr * sine {
                rr = sf.abs() / sine;
            }
            if t.left_tension < 0.0 && st.abs() < ss * sine {
                ss = st.abs() / sine;
            }
        }
    }
    let right = s.p + Pair::new(delta.x * ct - delta.y * st, delta.y * ct + delta.x * st) * rr;
    let left = t.p - Pair::new(delta.x * cf + delta.y * sf, delta.y * cf - delta.x * sf) * ss;
    (right, left)
}

/// Computes the Bézier control points of a path through `specs` using
/// Hobby's algorithm, following MetaPost's `make_choices`.
pub fn solve_path(specs: &[KnotSpec], closed: bool) -> Path {
    let n = specs.len();
    let mut knots: Vec<Knot> = specs.iter().map(|s| Knot::new(s.p)).collect();
    if n <= 1 {
        return Path { knots, closed };
    }

    let mut left: Vec<Side> = specs.iter().map(|s| s.left.into()).collect();
    let mut right: Vec<Side> = specs.iter().map(|s| s.right.into()).collect();
    // A direction or curl given on one side of a knot applies to both sides.
    for i in 0..n {
        if left[i] == Side::Open {
            left[i] = right[i];
        } else if right[i] == Side::Open {
            right[i] = left[i];
        }
    }
    if !closed {
        left[0] = Side::Endpoint;
        if right[0] == Side::Open {
            right[0] = Side::Curl(1.0);
        }
        right[n - 1] = Side::Endpoint;
        if left[n - 1] == Side::Open {
            left[n - 1] = Side::Curl(1.0);
        }
    }

    // Zero-length segments become explicit, with control points at the knot.
    let segments = if closed { n } else { n - 1 };
    for i in 0..segments {
        let j = (i + 1) % n;
        if specs[i].p == specs[j].p && !matches!(right[i], Side::Endpoint | Side::Explicit) {
            right[i] = Side::Explicit;
            if left[i] == Side::Open {
                left[i] = Side::Curl(1.0);
            }
            left[j] = Side::Explicit;
            if right[j] == Side::Open {
                right[j] = Side::Curl(1.0);
            }
        }
    }

    let Some(h) = (0..n).find(|&i| left[i] != Side::Open || right[i] != Side::Open) else {
        solve_cycle(specs, &mut knots);
        return Path { knots, closed };
    };
    let mut p = h;
    loop {
        let mut q = (p + 1) % n;
        if matches!(right[p], Side::Given(_) | Side::Curl(_)) {
            let mut segs = 1;
            while left[q] == Side::Open && right[q] == Side::Open {
                q = (q + 1) % n;
                segs += 1;
            }
            solve_run(specs, &mut knots, p, segs, right[p].boundary(), left[q].boundary());
        }
        p = q;
        if p == h {
            break;
        }
    }
    Path { knots, closed }
}

/// Chooses control points for the `segs` segments starting at knot `p`
/// (indices wrap around) between two breakpoints.
fn solve_run(specs: &[KnotSpec], knots: &mut [Knot], p: usize, segs: usize, start: Boundary, end: Boundary) {
    let n = specs.len();
    let idx = |k: usize| (p + k) % n;
    let delta: Vec<Pair> = (0..segs).map(|k| specs[idx(k + 1)].p - specs[idx(k)].p).collect();
    let dist: Vec<f64> = delta.iter().map(|d| d.len()).collect();
    // psi[k]: turning angle at knot k of the run; zero at both ends.
    let mut psi = vec![0.0; segs + 1];
    for k in 1..segs {
        psi[k] = turning_angle(delta[k - 1], delta[k]);
    }

    let (s0, s1) = (&specs[idx(0)], &specs[idx(1)]);
    if segs == 1 {
        match (&start, &end) {
            (Boundary::Given(g0), Boundary::Given(g1)) => {
                let chord = delta[0].angle_rad();
                let (r, l) = set_controls(s0, s1, delta[0], g0 - chord, chord - g1);
                knots[idx(0)].right_control = r;
                knots[idx(1)].left_control = l;
                return;
            }
            (Boundary::Curl(_), Boundary::Curl(_)) => {
                knots[idx(0)].right_control = s0.p + delta[0] / (3.0 * s0.right_tension.abs());
                knots[idx(1)].left_control = s1.p - delta[0] / (3.0 * s1.left_tension.abs());
                return;
            }
            _ => {}
        }
    }

    let mut a = vec![0.0; segs + 1];
    let mut b = vec![1.0; segs + 1];
    let mut c = vec![0.0; segs + 1];
    let mut d = vec![0.0; segs + 1];
    match start {
        Boundary::Given(g) => d[0] = reduce_angle(g - delta[0].angle_rad()),
        Boundary::Curl(gamma) => {
            let ratio = curl_ratio(gamma, s0.right_tension.abs(), s1.left_tension.abs());
            c[0] = ratio;
            d[0] = -ratio * psi[1];
        }
    }
    for k in 1..segs {
        (a[k], b[k], c[k], d[k]) = mock_curvature_row(
            &specs[idx(k - 1)],
            &specs[idx(k)],
            &specs[idx(k + 1)],
            dist[k - 1],
            dist[k],
            psi[k],
            psi[k + 1],
        );
    }
    match end {
        Boundary::Given(g) => d[segs] = reduce_angle(g - delta[segs - 1].angle_rad()),
        Boundary::Curl(gamma) => {
            let (r, s) = (&specs[idx(segs - 1)], &specs[idx(segs)]);
            a[segs] = curl_ratio(gamma, s.left_tension.abs(), r.right_tension.abs());
        }
    }
    let theta = solve_tridiagonal(&a, &b, &c, &d);

    for k in 0..segs {
        let phi = -psi[k + 1] - theta[k + 1];
        let (r, l) = set_controls(&specs[idx(k)], &specs[idx(k + 1)], delta[k], theta[k], phi);
        knots[idx(k)].right_control = r;
        knots[idx(k + 1)].left_control = l;
    }
}

/// Chooses control points for a cycle without breakpoints.
fn solve_cycle(specs: &[KnotSpec], knots: &mut [Knot]) {
    let n = specs.len();
    let delta: Vec<Pair> = (0..n).map(|k| specs[(k + 1) % n].p - specs[k].p).collect();
    let dist: Vec<f64> = delta.iter().map(|d| d.len()).collect();
    let psi: Vec<f64> = (0..n).map(|k| turning_angle(delta[(k + n - 1) % n], delta[k])).collect();

    let mut a = vec![0.0; n];
    let mut b = vec![0.0; n];
    let mut c = vec![0.0; n];
    let mut d = vec![0.0; n];
    for k in 0..n {
        let prev = (k + n - 1) % n;
        let next = (k + 1) % n;
        (a[k], b[k], c[k], d[k]) = mock_curvature_row(
            &specs[prev],
            &specs[k],
            &specs[next],
            dist[prev],
            dist[k],
            psi[k],
            psi[next],
        );
    }
    let theta = solve_cyclic(&a, &b, &c, &d);

    for k in 0..n {
        let next = (k + 1) % n;
        let phi = -psi[next] - theta[next];
        let (r, l) = set_controls(&specs[k], &specs[next], delta[k], theta[k], phi);
        knots[k].right_control = r;
        knots[next].left_control = l;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specs(points: &[(f64, f64)]) -> Vec<KnotSpec> {
        points.iter().map(|&(x, y)| KnotSpec::new(Pair::new(x, y))).collect()
    }

    /// Asserts the path's points in PostScript order (`p0`, then
    /// `right, left, p` per segment) match `mpost -numbersystem=double`.
    fn assert_path(path: &Path, expected: &[(f64, f64)]) {
        let n = path.knots.len();
        let segments = if path.closed { n } else { n - 1 };
        let mut got = vec![(path.knots[0].p.x, path.knots[0].p.y)];
        for i in 0..segments {
            let (a, b) = (path.knots[i], path.knots[(i + 1) % n]);
            got.extend([a.right_control, b.left_control, b.p].map(|q| (q.x, q.y)));
        }
        assert_eq!(got.len(), expected.len(), "got {got:?}");
        for (g, e) in got.iter().zip(expected) {
            assert!(
                (g.0 - e.0).abs() < 1e-5 && (g.1 - e.1).abs() < 1e-5,
                "got {got:?}\nexpected {expected:?}"
            );
        }
    }

    #[test]
    fn open_path_end_direction() {
        // (0,0)..(10,5)..(20,0)..(30,8){down}
        let mut s = specs(&[(0.0, 0.0), (10.0, 5.0), (20.0, 0.0), (30.0, 8.0)]);
        s[3].left = KnotSide::Given(-90.0);
        assert_path(
            &solve_path(&s, false),
            &[(0.0, 0.0), (0.635091, 4.728436), (5.836196, 7.328989), (10.0, 5.0), (13.600307, 2.986198), (16.335839, -2.521154), (20.0, 0.0), (24.549189, 3.130104), (30.0, 17.515277), (30.0, 8.0)],
        );
    }

    #[test]
    fn open_path_control_lengths() {
        // (0,0)..(10,5)..(20,0)..(30,8)
        let s = specs(&[(0.0, 0.0), (10.0, 5.0), (20.0, 0.0), (30.0, 8.0)]);
        assert_path(
            &solve_path(&s, false),
            &[(0.0, 0.0), (1.645939, 3.903038), (5.890007, 6.025072), (10.0, 5.0), (13.666379, 4.08557), (16.282589, 0.730466), (20.0, 0.0), (25.090207, -1.000218), (29.85846, 2.814385), (30.0, 8.0)],
        );
    }

    #[test]
    fn given_direction_is_reduced_relative_to_chord() {
        // (0,0){up}..(-10,-1.7633)..(-20,0): the chord points at -170 degrees.
        let mut s = specs(&[(0.0, 0.0), (-10.0, -1.7633), (-20.0, 0.0)]);
        s[0].right = KnotSide::Given(90.0);
        assert_path(
            &solve_path(&s, false),
            &[(0.0, 0.0), (0.0, 3.951877), (-5.324079, 1.717808), (-10.0, -1.7633), (-13.220172, -4.160639), (-17.793986, -3.354138), (-20.0, 0.0)],
        );
    }

    #[test]
    fn two_knot_direction_couples_with_end_curl() {
        // (0,0){up}..(10,0)
        let mut s = specs(&[(0.0, 0.0), (10.0, 0.0)]);
        s[0].right = KnotSide::Given(90.0);
        assert_path(&solve_path(&s, false), &[(0.0, 0.0), (0.0, 6.666667), (10.0, 6.666667), (10.0, 0.0)]);
    }

    #[test]
    fn one_sided_direction_applies_to_both_sides() {
        // (0,0)..{up}(10,0)..(20,0)
        let mut s = specs(&[(0.0, 0.0), (10.0, 0.0), (20.0, 0.0)]);
        s[1].left = KnotSide::Given(90.0);
        assert_path(
            &solve_path(&s, false),
            &[(0.0, 0.0), (0.0, -6.666667), (10.0, -6.666667), (10.0, 0.0), (10.0, 6.666667), (20.0, 6.666667), (20.0, 0.0)],
        );
    }

    #[test]
    fn open_cycle_is_solved_exactly() {
        // (0,0)..(30,10)..(20,40)..(-10,20)..(5,5)..cycle
        let s = specs(&[(0.0, 0.0), (30.0, 10.0), (20.0, 40.0), (-10.0, 20.0), (5.0, 5.0)]);
        assert_path(
            &solve_path(&s, true),
            &[(0.0, 0.0), (-9.760906, -16.521741), (30.331173, -22.929482), (30.0, 10.0), (29.888497, 21.087048), (28.303394, 32.751153), (20.0, 40.0), (-0.936985, 58.277948), (-22.534884, 32.867084), (-10.0, 20.0), (-4.496135, 14.350271), (9.364251, 13.271937), (5.0, 5.0), (3.877033, 2.871545), (1.220277, 2.065495), (0.0, 0.0)],
        );
    }

    #[test]
    fn cycle_honours_given_direction() {
        // (0,0)..(10,10)..{down}cycle
        let mut s = specs(&[(0.0, 0.0), (10.0, 10.0)]);
        s[0].left = KnotSide::Given(-90.0);
        assert_path(
            &solve_path(&s, true),
            &[(0.0, 0.0), (0.0, -19.655407), (29.967368, 1.729245), (10.0, 10.0), (4.976124, 12.080958), (0.0, 6.735506), (0.0, 0.0)],
        );
    }

    #[test]
    fn tension_and_atleast() {
        // (0,0)..(10,10)..tension 3..(20,0)..(30,10)
        let mut s = specs(&[(0.0, 0.0), (10.0, 10.0), (20.0, 0.0), (30.0, 10.0)]);
        s[1].right_tension = 3.0;
        s[2].left_tension = 3.0;
        assert_path(
            &solve_path(&s, false),
            &[(0.0, 0.0), (-5.948823, 6.656736), (3.343264, 15.948823), (10.0, 10.0), (11.172586, 8.952113), (18.827414, 1.047887), (20.0, 0.0), (26.656736, -5.948823), (35.948823, 3.343264), (30.0, 10.0)],
        );
        // (0,0){dir 80}...{dir -80}(10,0): `atleast` bounds the velocities.
        let mut s = specs(&[(0.0, 0.0), (10.0, 0.0)]);
        s[0].right = KnotSide::Given(80.0);
        s[1].left = KnotSide::Given(-80.0);
        s[0].right_tension = -1.0;
        s[1].left_tension = -1.0;
        assert_path(&solve_path(&s, false), &[(0.0, 0.0), (0.986373, 5.593998), (9.013627, 5.593998), (10.0, 0.0)]);
    }

    #[test]
    fn zero_length_segment_splits_path() {
        // (0,0)..(10,0)..(10,0)..(20,5)
        let s = specs(&[(0.0, 0.0), (10.0, 0.0), (10.0, 0.0), (20.0, 5.0)]);
        assert_path(
            &solve_path(&s, false),
            &[(0.0, 0.0), (3.333333, 0.0), (6.666667, 0.0), (10.0, 0.0), (10.0, 0.0), (10.0, 0.0), (10.0, 0.0), (13.333333, 1.666667), (16.666667, 3.333333), (20.0, 5.0)],
        );
    }
}
