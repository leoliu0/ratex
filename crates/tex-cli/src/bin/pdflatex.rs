use tex_core::engine::Engine;
use tex_core::prim::{DimParam, IntParam};
use tex_core::pdffile;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut file: Option<String> = None;
    let mut out_dir = String::new();
    let mut jobname: Option<String> = None;
    let mut ini = false;
    let mut plain = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-output-directory" => { i += 1; out_dir = format!("{}/", args.get(i).cloned().unwrap_or_default()); }
            "-jobname" => { i += 1; jobname = Some(args.get(i).cloned().unwrap_or_default()); }
            "-ini" => ini = true,
            "-plain" => plain = true,
            "-interaction=nonstopmode" | "-interaction=batchmode" | "-interaction=scrollmode" => {}
            "-halt-on-error" => {}
            "-v" | "-version" => { println!("pdfTeX-2h 1.40.29-rs (TeX Live 2026/Rust)"); return; }
            other => {
                if !other.starts_with('-') { file = Some(other.to_string()); }
            }
        }
        i += 1;
    }
    let Some(file) = file else { eprintln!("usage: pdflatex [-ini] [-output-directory dir] file.tex"); std::process::exit(2); };

    let mut eng = Engine::new(ini || !plain);
    eng.init_primitives();
    eng.out_dir = out_dir.clone();
    let job = jobname.unwrap_or_else(|| {
        std::path::Path::new(&file)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "texput".to_string())
    });
    eng.job_name = job.clone();
    if !plain && !ini {
        let exe_fmt = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("pdflatex.fmt")));
        let cand_paths = [
            Some(std::path::PathBuf::from("pdflatex.fmt")),
            exe_fmt.clone(),
            Some(std::path::PathBuf::from("/tmp/pdflatex.fmt")),
        ];
        let mut loaded = false;
        for cand in cand_paths.into_iter().flatten() {
            if cand.exists() {
                let t0 = std::time::Instant::now();
                // load in place: the engine (and its kpse/ls-R setup) is reused
                match tex_core::format::load_format_into(&cand, &mut eng) {
                    Ok(()) => {
                        loaded = true;
                        eprintln!(
                            "PROG: format loaded from {} in {:.1} ms",
                            cand.display(),
                            t0.elapsed().as_secs_f64() * 1000.0
                        );
                        break;
                    }
                    Err(e) => eprintln!("PROG: format at {} unusable ({})", cand.display(), e),
                }
            }
        }
        if !loaded {
            eprintln!("PROG: booting latex.ltx");
            // Hyphenation for raw boot
            let _ = eng.hyphen_trie.load_hyphen_file(std::path::Path::new("/usr/share/texmf-dist/tex/generic/hyphen/hyphen.tex"));
            eng.add_nullfont();
            eng.input_file("latex.ltx");
            eng.run();
            if eng.format_done {
                let dump_target = exe_fmt.unwrap_or_else(|| std::path::PathBuf::from("pdflatex.fmt"));
                match tex_core::format::save_format(&eng, &dump_target) {
                    Ok(n) => eprintln!("PROG: format dumped to {} ({} bytes)", dump_target.display(), n),
                    Err(e) => eprintln!("PROG: format dump skipped: {}", e),
                }
            } else {
                eprintln!("LaTeX format boot failed; last file {} line {}", eng.input.current_file_name(), eng.input.current_file_line());
                print!("{}", eng.term);
                std::process::exit(1);
            }
        }
        eng.end_occurred = false;
        eng.input.stack.clear();
        let ej = (*eng.eqtb.tok_params[tex_core::prim::ToksParam::EveryJob.idx() as usize]).clone();
        if !ej.is_empty() {
            eng.input.push_toks(ej, "<everyjob>");
        }
    } else {
        let _ = eng.hyphen_trie.load_hyphen_file(std::path::Path::new("/usr/share/texmf-dist/tex/generic/hyphen/hyphen.tex"));
        eng.eqtb.dim_params[DimParam::HSize.idx() as usize] = (6.25 * 72.27 * 65536.0) as i32;
        eng.eqtb.dim_params[DimParam::VSize.idx() as usize] = (8.75 * 72.27 * 65536.0) as i32;
        eng.eqtb.dim_params[DimParam::MaxDepth.idx() as usize] = (4.0 * 65536.0) as i32;
        eng.eqtb.dim_params[DimParam::ParIndent.idx() as usize] = (1.5 * 65536.0 * 10.0) as i32;
        eng.eqtb.int_params[IntParam::EndLineChar.idx() as usize] = 13;
        eng.eqtb.int_params[IntParam::EscapeChar.idx() as usize] = 92;
        eng.eqtb.int_params[IntParam::NewLineChar.idx() as usize] = 10;
        eng.eqtb.int_params[IntParam::MaxDeadCycles.idx() as usize] = 25;
        eng.eqtb.int_params[tex_core::prim::IntParam::EtxVersion.idx() as usize] = 2;
        eng.eqtb.int_params[IntParam::LeftHyphenMin.idx() as usize] = 2;
        eng.eqtb.int_params[IntParam::RightHyphenMin.idx() as usize] = 3;
        eng.eqtb.int_params[IntParam::Tolerance.idx() as usize] = 200;
        eng.eqtb.int_params[IntParam::Pretolerance.idx() as usize] = 100;
        eng.eqtb.int_params[IntParam::LinePenalty.idx() as usize] = 10;
        eng.eqtb.dim_params[DimParam::Hfuzz.idx() as usize] = 6554;
        eng.eqtb.dim_params[DimParam::Vfuzz.idx() as usize] = 6554;
        eng.eqtb.dim_params[DimParam::OverfullRule.idx() as usize] = 327_680;
        eng.eqtb.dim_params[DimParam::BoxMaxDepth.idx() as usize] = 262_144;
        eng.eqtb.int_params[IntParam::HBadness.idx() as usize] = 1000;
        eng.eqtb.int_params[IntParam::VBadness.idx() as usize] = 1000;
        eng.eqtb.int_params[IntParam::HyphenPenalty.idx() as usize] = 50;
        eng.eqtb.int_params[IntParam::ExHyphenPenalty.idx() as usize] = 50;
        eng.eqtb.int_params[IntParam::ClubPenalty.idx() as usize] = 150;
        eng.eqtb.int_params[IntParam::WidowPenalty.idx() as usize] = 150;
        eng.add_nullfont();
    }
    eprintln!("PROG: running user file");
    if eng.input_file(&file) {
        eng.run();
    }
    // -ini mode: the file ended in \dump — write the format and exit,
    // like initex does.
    if ini && eng.format_done {
        match tex_core::format::save_format(&eng, std::path::Path::new("pdflatex.fmt")) {
            Ok(n) => eprintln!("PROG: format dumped to pdflatex.fmt ({} bytes)", n),
            Err(e) => eprintln!("PROG: format dump failed: {}", e),
        }
        print!("{}", eng.term);
        std::process::exit(if eng.error_count > 0 { 1 } else { 0 });
    }
    print!("{}", eng.term);
    // write PDF if pages were shipped
    if !eng.pdf_doc.pages.is_empty() {
        // embed fonts used
        use std::collections::BTreeSet;
        let mut used: BTreeSet<u16> = BTreeSet::new();
        for p in &eng.pdf_doc.pages {
            for (fid, _) in &p.fonts {
                used.insert(*fid as u16);
            }
        }
        let mut fidx: Vec<(u16, usize)> = Vec::new();
        for (n, fid) in used.iter().enumerate() {
            if let Some(font) = eng.eqtb.fonts.get(*fid as usize) {
                let pfb_bytes = font
                    .type1_path
                    .as_ref()
                    .and_then(|name| eng.font_loader.kpse.find(name, tex_kpse::Format::Type1))
                    .and_then(|p| std::fs::read(p).ok());
                let widths = (0..=255u8)
                    .map(|c| {
                        let w = font.char_width(c);
                        (w as i64 * 1000 / font.at_size as i64) as i32
                    })
                    .collect();
                let ef = pdffile::make_embed_font(
                    font.map_fontname.clone().unwrap_or_else(|| font.tfm_name.clone()),
                    pfb_bytes.as_deref(),
                    font.encoding.as_deref(),
                    0,
                    255,
                    widths,
                );
                eng.pdf_doc.fonts.push(ef);
                fidx.insert(n, (*fid, n));
            }
        }
        // remap page font indices: pages reference engine font ids; convert to doc font index
        for p in eng.pdf_doc.pages.iter_mut() {
            for pf in p.fonts.iter_mut() {
                if let Some(pos) = fidx.iter().find(|(fid, _)| *fid as u16 as u16 == pf.0 as u16) {
                    pf.0 = pos.1;
                }
            }
        }
        let pdf = pdffile::write_pdf(&eng.pdf_doc);
        let out = format!("{}{}.pdf", eng.out_dir, job);
        std::fs::write(&out, &pdf).expect("write pdf");
        println!("\nOutput written on {} ({} bytes).", out, pdf.len());
        std::process::exit(if eng.error_count > 0 { 1 } else { 0 });
    } else {
        eprintln!("No pages of output.");
        std::process::exit(if eng.error_count > 0 { 1 } else { 0 });
    }
}
