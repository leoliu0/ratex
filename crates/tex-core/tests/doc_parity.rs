//! Private-document raster gate. Prepare isolated `*-rust` and `*-reference`
//! directories, then run with TEX_PARITY_ROOT and --ignored. This test never
//! modifies manuscript sources or substitutes a reference PDF for Rust output.

use std::process::Command;

#[test]
#[ignore = "requires private benchmark PDFs, PyMuPDF, and NumPy; set TEX_PARITY_ROOT"]
fn all_documents_exceed_99_percent_exact_pixel_parity() {
    let root = std::env::var("TEX_PARITY_ROOT")
        .expect("set TEX_PARITY_ROOT to isolated benchmark artifacts");
    let output = Command::new("python3")
        .args([
            "-c",
            r#"
import json
import pathlib
import sys
import numpy as np
import pymupdf

root = pathlib.Path(sys.argv[1])
manifest = json.loads((root / 'inputs.json').read_text())
reports = []
failures = []
for name, job in (
    ('trust', 'main'),
    ('beamer', 'ch12_trading_strategies'),
    ('ai', 'main'),
    ('cluster_ceo', 'main'),
):
    expected_pages = manifest[name]['pages']
    ours = pymupdf.open(root / (name + '-rust') / (job + '.pdf'))
    reference = pymupdf.open(root / (name + '-reference') / (job + '.pdf'))
    assert ours.metadata.get('producer') == 'tex-rs', (name, ours.metadata)
    assert not ours.is_repaired and not reference.is_repaired, name
    assert len(ours) == len(reference) == expected_pages, (name, len(ours), len(reference))
    differing = total = 0
    page_scores = []
    for number, (left, right) in enumerate(zip(ours, reference), 1):
        pymupdf.TOOLS.mupdf_warnings(reset=True)
        a = left.get_pixmap(dpi=150, colorspace=pymupdf.csRGB, alpha=False)
        b = right.get_pixmap(dpi=150, colorspace=pymupdf.csRGB, alpha=False)
        warnings = pymupdf.TOOLS.mupdf_warnings(reset=True)
        assert not warnings, (name, number, warnings)
        assert (a.width, a.height) == (b.width, b.height), (name, number)
        aa = np.frombuffer(a.samples, dtype=np.uint8).reshape(-1, 3)
        bb = np.frombuffer(b.samples, dtype=np.uint8).reshape(-1, 3)
        difference = int(np.count_nonzero(np.any(aa != bb, axis=1)))
        pixels = a.width * a.height
        differing += difference
        total += pixels
        page_scores.append(100.0 * (1.0 - difference / pixels))
    score = 100.0 * (1.0 - differing / total)
    # the document aggregate must never hide a bad page: every page clears
    # the same 99% exact-RGB floor (identical geometry, no tolerance).
    if score < 99.0 or min(page_scores) < 99.0:
        failures.append((name, score, min(page_scores)))
    reports.append(dict(document=name, pages=len(ours), dpi=150,
                        exact_pixel_parity=score,
                        worst_page_parity=min(page_scores),
                        worst_page_number=page_scores.index(min(page_scores)) + 1,
                        differing_pixels=differing, total_pixels=total))
print(json.dumps(reports, indent=2))
assert not failures, failures
"#,
            &root,
        ])
        .output()
        .expect("run Python raster gate");
    eprintln!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "raster gate failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
