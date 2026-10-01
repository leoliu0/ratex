use tex_mplib::{
    Color, LinearExpr, LinearSolver, MpConfig, MpFigure, MpObject, MpResult, MpSession, Pair, Path,
    Transform, MPLIB_VERSION,
};

#[test]
fn test_mplib_version() {
    assert_eq!(MPLIB_VERSION, "3.00");
}

#[test]
fn test_linear_solver() {
    let mut solver = LinearSolver::new();
    let x0 = solver.get_var_by_name("x0");
    let x1 = solver.get_var_by_name("x1");

    // x0 = 10
    solver
        .equate(&LinearExpr::variable(x0), &LinearExpr::constant(10.0))
        .unwrap();
    assert_eq!(solver.get_expr(x0).as_known(), Some(10.0));

    // x1 - x0 = 20  =>  x1 = x0 + 20 = 30
    let expr = LinearExpr::variable(x1).sub(&LinearExpr::variable(x0));
    solver
        .equate(&expr, &LinearExpr::constant(20.0))
        .unwrap();

    assert_eq!(solver.get_expr(x1).as_known(), Some(30.0));
}

#[test]
fn test_mpsession_figure_generation() {
    let mut session = MpSession::new(MpConfig::default());
    let code = r#"
        beginfig(1);
        draw (0, 0) -- (100, 100) withcolor red;
        fill fullcircle scaled 50 withcolor blue;
        endfig;
    "#;

    let res = session.execute(code);
    assert_eq!(res.status, 0, "log: {}, term: {}", res.log, res.term);
    assert_eq!(res.fig.len(), 1);

    let fig = &res.fig[0];
    assert_eq!(fig.charcode, 1);
    assert_eq!(fig.objects.len(), 2);

    // First object is stroke of line (0,0) to (100,100)
    match &fig.objects[0] {
        MpObject::Stroke { color, .. } => {
            assert_eq!(*color, Color::RED);
        }
        _ => panic!("Expected Stroke object"),
    }

    // Second object is fill of circle
    match &fig.objects[1] {
        MpObject::Fill { color, .. } => {
            assert_eq!(*color, Color::BLUE);
        }
        _ => panic!("Expected Fill object"),
    }

    let ps = fig.to_postscript();
    assert!(ps.contains("%!PS-Adobe-3.0 EPSF-3.0"));
    assert!(ps.contains("setrgbcolor"));
    assert!(ps.contains("stroke"));
    assert!(ps.contains("fill"));

    let svg = fig.to_svg();
    assert!(svg.contains("<svg"));
    assert!(svg.contains("<path"));
}

#[test]
fn test_path_bounding_box() {
    let path = Path::rectangle(Pair::new(10.0, 20.0), Pair::new(50.0, 80.0));
    let bbox = path.bounding_box();
    assert_eq!(bbox, (10.0, 20.0, 50.0, 80.0));
}

fn run(code: &str) -> MpResult {
    MpSession::new(MpConfig::default()).execute(code)
}

fn run_ok(code: &str) -> MpResult {
    let res = run(code);
    assert_eq!(res.status, 0, "log: {}", res.log);
    res
}

fn assert_error(code: &str, message: &str) {
    let res = run(code);
    assert_eq!(res.status, 2, "expected an error for {code:?}");
    assert!(res.log.contains(message), "log {:?} lacks {message:?}", res.log);
}

/// Points of the `index`-th object of the first figure in PostScript order:
/// `p0`, then `right, left, p` per segment.
fn object_points(res: &MpResult, index: usize) -> Vec<(f64, f64)> {
    let path = match &res.fig[0].objects[index] {
        MpObject::Stroke { path, .. } | MpObject::Fill { path, .. } => path,
        other => panic!("not a path object: {other:?}"),
    };
    let n = path.knots.len();
    let segments = if path.closed { n } else { n - 1 };
    let mut points = vec![(path.knots[0].p.x, path.knots[0].p.y)];
    for i in 0..segments {
        let (a, b) = (path.knots[i], path.knots[(i + 1) % n]);
        points.extend([a.right_control, b.left_control, b.p].map(|q| (q.x, q.y)));
    }
    points
}

/// Compares against coordinates written by `mpost -numbersystem=double`.
fn assert_points(got: &[(f64, f64)], expected: &[(f64, f64)]) {
    assert_eq!(got.len(), expected.len(), "got {got:?}");
    for (g, e) in got.iter().zip(expected) {
        assert!(
            (g.0 - e.0).abs() < 1e-5 && (g.1 - e.1).abs() < 1e-5,
            "got {got:?}\nexpected {expected:?}"
        );
    }
}

#[test]
fn polyline_segments_are_straight() {
    let res = run_ok("beginfig(1); draw (0,0)--(10,0)--(10,10); draw (0,0)..(10,5)--(20,0)..(30,5); endfig;");
    let third = 10.0 / 3.0;
    assert_points(
        &object_points(&res, 0),
        &[(0.0, 0.0), (third, 0.0), (2.0 * third, 0.0), (10.0, 0.0), (10.0, third), (10.0, 2.0 * third), (10.0, 10.0)],
    );
    // `--` is `{curl 1}..{curl 1}`: the neighbouring `..` segments are straight too.
    assert_points(
        &object_points(&res, 1),
        &[(0.0, 0.0), (3.333333, 1.666667), (6.666667, 3.333333), (10.0, 5.0), (13.333333, 3.333333), (16.666667, 1.666667), (20.0, 0.0), (23.333333, 1.666667), (26.666667, 3.333333), (30.0, 5.0)],
    );
    let ps = res.fig[0].to_postscript();
    assert!(ps.contains("10.0000 0.0000 lineto\n10.0000 10.0000 lineto\n"), "{ps}");
}

#[test]
fn path_direction_and_tension_syntax() {
    let res = run_ok(
        "beginfig(1);
         draw (0,0){1,1}..(10,0);
         draw (0,0)..{curl 2}(10,0)..(20,5)..(25,0);
         draw (0,0){dir 80}...{dir -80}(10,0);
         draw (0,0)..(10,10)..tension 3..(20,0)..(30,10);
         draw (0,0)..(10,0){up}..(0,10)..{down}(-10,0)..cycle;
         draw (0,0)..(10,0) shifted (5,5)..(20,0);
         endfig;",
    );
    assert_points(&object_points(&res, 0), &[(0.0, 0.0), (2.761424, 2.761424), (7.238576, 2.761424), (10.0, 0.0)]);
    assert_points(
        &object_points(&res, 1),
        &[(0.0, 0.0), (3.333333, 0.0), (6.666667, 0.0), (10.0, 0.0), (10.214565, 4.451558), (15.36735, 6.869389), (20.0, 5.0), (22.276449, 4.081397), (24.081397, 2.276449), (25.0, 0.0)],
    );
    assert_points(&object_points(&res, 2), &[(0.0, 0.0), (0.986373, 5.593998), (9.013627, 5.593998), (10.0, 0.0)]);
    assert_points(
        &object_points(&res, 3),
        &[(0.0, 0.0), (-5.948823, 6.656736), (3.343264, 15.948823), (10.0, 10.0), (11.172586, 8.952113), (18.827414, 1.047887), (20.0, 0.0), (26.656736, -5.948823), (35.948823, 3.343264), (30.0, 10.0)],
    );
    assert_points(
        &object_points(&res, 4),
        &[(0.0, 0.0), (3.938137, 0.0), (10.0, -5.03724), (10.0, 0.0), (10.0, 5.522847), (5.522847, 10.0), (0.0, 10.0), (-5.522847, 10.0), (-10.0, 5.522847), (-10.0, 0.0), (-10.0, -5.03724), (-3.938137, 0.0), (0.0, 0.0)],
    );
    // `shifted` is a secondary operator: it moves only the knot it follows.
    assert_points(
        &object_points(&res, 5),
        &[(0.0, 0.0), (2.849578, 5.420155), (9.468245, 7.626377), (15.0, 5.0), (17.192769, 3.958913), (18.958913, 2.192769), (20.0, 0.0)],
    );
}

#[test]
fn equations_with_unknowns() {
    let res = run_ok(
        "beginfig(1); z0=(0,0); z1=z0+(10,0); x2 = x1; y2 - y1 = 2*3; numeric a; a + 1 = x1/2;
         draw z0..z1..z2 shifted (a,0); endfig;",
    );
    assert_points(
        &object_points(&res, 0),
        &[(0.0, 0.0), (3.046132, -1.956874), (6.953868, -1.956874), (10.0, 0.0), (12.110557, 1.355849), (13.5602, 3.530313), (14.0, 6.0)],
    );
}

#[test]
fn inconsistent_and_nonlinear_equations() {
    assert_error("x1 = 10; x1 = 11;", "Inconsistent equation (off by 1)");
    assert_error("x1 = 10; 11 = x1;", "Inconsistent equation (off by -1)");
    assert_error("(1,2) = (1,3);", "Inconsistent equation (off by 1)");
    assert_error("numeric a; a*a = 4;", "Not implemented: (unknown numeric)*(unknown numeric)");
    // MetaPost treats differences up to 64/65536 as redundant.
    run_ok("x1 = 1; x1 = 1.0001;");
}

#[test]
fn assignment_replaces_value() {
    let res = run_ok("a := 1; a := 2; beginfig(1); draw (a,0)--(0,0); endfig;");
    assert_eq!(object_points(&res, 0)[0], (2.0, 0.0));
}

#[test]
fn for_loops_run_and_are_bounded() {
    let res = run_ok("beginfig(1); for i=3 downto 1: draw (i,0)--(i,1); endfor endfig;");
    let xs: Vec<f64> = (0..3).map(|i| object_points(&res, i)[0].0).collect();
    assert_eq!(xs, [3.0, 2.0, 1.0]);

    assert_error("for i=0 step 0 until 1: endfor", "never end");
    assert_error("for i=0 step 0.000001 until 1000000: endfor", "Loop would run");
    assert_error(
        "for i=1 upto 1000: for j=1 upto 1000: for k=1 upto 1000: endfor endfor endfor",
        "Execution limit",
    );
    assert_error("for i=0 upto 3: draw (0,0)--(1,1);", "Missing endfor");
}

#[test]
fn malformed_input_is_rejected() {
    assert_error("message \"abc", "Incomplete string token");
    assert_error("beginfig(", "Unexpected EOF");
    assert_error("x = 1/0;", "Division by zero");
    assert_error("x = 2/(3-3);", "Division by zero");
    assert_error(&format!("x = {};", "9".repeat(400)), "Number too large");
    assert_error("a = 1; b = 10000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000; c = b*b;", "Arithmetic overflow");
    assert_error("beginfig(1); draw (0,0)..tension 0.5..(1,1); endfig;", "Improper tension");
    assert_error("beginfig(1); draw (0,0){curl -1}..(1,1); endfig;", "Improper curl");
}

#[test]
fn nesting_is_bounded() {
    let nested = |depth: usize| format!("x = {}1{};", "(".repeat(depth), ")".repeat(depth));
    run_ok(&nested(90));
    assert_error(&nested(5000), "Nesting deeper than");
    let loops = |depth: usize| format!("{}{}", "for i=0 upto 0: ".repeat(depth), "endfor ".repeat(depth));
    run_ok(&loops(90));
    assert_error(&loops(5000), "Nesting deeper than");
}

#[test]
fn beginfig_resets_xy_and_pen() {
    let res = run_ok(
        "beginfig(1); pickup pencircle scaled 3; z1 = (0,0); draw z1--(1,1); endfig;
         beginfig(2); z1 = (5,5); draw z1--(1,1); endfig;",
    );
    let width = |fig: &MpFigure| match &fig.objects[0] {
        MpObject::Stroke { width, .. } => *width,
        other => panic!("not a stroke: {other:?}"),
    };
    assert_eq!(width(&res.fig[0]), 3.0);
    assert_eq!(width(&res.fig[1]), 0.5);
    match &res.fig[1].objects[0] {
        MpObject::Stroke { path, .. } => assert_eq!(path.knots[0].p, Pair::new(5.0, 5.0)),
        other => panic!("not a stroke: {other:?}"),
    }
}

#[test]
fn clip_applies_to_earlier_drawing() {
    let res = run_ok(
        "beginfig(1); draw (0,0)--(20,20); clip currentpicture to unitsquare scaled 5; draw (0,0)--(30,0); endfig;",
    );
    let objects = &res.fig[0].objects;
    assert!(matches!(objects[0], MpObject::StartClip { .. }));
    assert!(matches!(objects[1], MpObject::Stroke { .. }));
    assert!(matches!(objects[2], MpObject::StopClip));
    assert!(matches!(objects[3], MpObject::Stroke { .. }));
    let ps = res.fig[0].to_postscript();
    assert_eq!(ps.matches("gsave").count(), ps.matches("grestore").count());
}

#[test]
fn svg_text_is_escaped() {
    let fig = MpFigure::new(
        1,
        vec![MpObject::Text {
            text: "a<b & c>".into(),
            font: "cmr10".into(),
            scale: 1.0,
            transform: Transform::shifted(0.0, 0.0),
            color: Color::BLACK,
        }],
    );
    let svg = fig.to_svg();
    assert!(svg.contains(">a&lt;b &amp; c&gt;</text>"), "{svg}");
}

#[test]
fn tiny_factors_are_not_flushed_to_zero() {
    let x = LinearExpr::constant(5.0).mul_scalar(1e-13);
    assert_eq!(x.as_known(), Some(5e-13));
    let res = run_ok("beginfig(1); a = 1/1000000; b = a*a; c = b/10; draw c*(5,0)*10000000000000--(0,0); endfig;");
    let x0 = object_points(&res, 0)[0].0;
    assert!((x0 - 5.0).abs() < 1e-9, "{x0}");
}
