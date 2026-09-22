use tex_mplib::{
    Color, LinearExpr, LinearSolver, MpConfig, MpObject, MpSession, Pair, Path, MPLIB_VERSION,
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
