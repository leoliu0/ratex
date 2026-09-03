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
    if let Some(dir) = std::path::Path::new(&file).parent() {
        if !dir.as_os_str().is_empty() {
            eng.main_dir = Some(dir.to_path_buf());
        }
    }
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
                        // Sanity: a dump taken from a broken boot (zeroed
                        // catcodes etc.) silently poisons every later run.
                        // Detect and fall through to a fresh boot.
                        if eng.eqtb.cat[b'd' as usize] != 11 || eng.eqtb.cat[b'@' as usize] == 0 {
                            eprintln!("PROG: format at {} is insane (bad catcodes); booting fresh", cand.display());
                            eng = Engine::new(ini || !plain);
                            eng.init_primitives();
                            eng.out_dir = out_dir.clone();
                            if let Some(dir) = std::path::Path::new(&file).parent() {
                                if !dir.as_os_str().is_empty() {
                                    eng.main_dir = Some(dir.to_path_buf());
                                }
                            }
                            eng.job_name = job.clone();
                            break;
                        }
                        loaded = true;
                        // Compat shim for formats dumped before the
                        // active-char namespace split: their boot wrote the
                        // kernel tie to the hash slot, where encoding
                        // defaults (\DeclareTextAccentDefault) later
                        // clobbered it, so no usable tie survives on either
                        // slot. Synthesize latex.ltx:9414's protected tie
                        // directly on the active slot; no-op when the format
                        // already carries one (post-split dumps).
                        let act = eng.cs.intern(&tex_core::engine::Engine::active_cs_name(b'~'));
                        if eng.eqtb.get(act).is_none() {
                            let id_of = |eng: &tex_core::engine::Engine, name: &[u8]| eng.cs.lookup(name);
                            let tie: Option<tex_core::eqtb::Equiv> = {
                                let ifincs = id_of(&eng, b"ifincsname");
                                let expafter = id_of(&eng, b"expandafter");
                                let nobreak = id_of(&eng, b"nobreakspace");
                                let fi = id_of(&eng, b"fi");
                                match (ifincs, expafter, nobreak, fi) {
                                    (Some(a), Some(b), Some(c), Some(d)) => {
                                        let body = vec![
                                            tex_core::token::Token::from_cs(a),
                                            tex_core::token::Token::from_cs(b),
                                            tex_core::token::Token::char(13, b'~' as u32),
                                            tex_core::token::Token::from_cs(d),
                                            tex_core::token::Token::from_cs(b),
                                            tex_core::token::Token::from_cs(c),
                                            tex_core::token::Token::from_cs(d),
                                        ];
                                        Some(tex_core::eqtb::Equiv::Macro(std::rc::Rc::new(tex_core::eqtb::Macro {
                                            num_params: 0,
                                            params: Vec::new(),
                                            prefix: Vec::new(),
                                            body,
                                            long: false,
                                            outer: false,
                                            protected: true,
                                        })))
                                    }
                                    _ => None,
                                }
                            };
                            if let Some(eq) = tie {
                                eng.eqtb.assign(act, eq, true);
                            }
                        }
                        // tex.web §240: period is the null delimiter (code 0)
                        eng.eqtb.del_code[b'.' as usize] = 0;
                        // tex.web §1014: page_goal starts at max_dimen
                        eng.eqtb.dim_params[tex_core::prim::DimParam::PageGoal.idx() as usize] = 0x3FFF_FFFF;
                        eng.page_goal_set = false;
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
                // Only persist a clean boot: a dump from a degraded boot
                // silently poisons every later run through the exe-dir fmt.
                if eng.error_count == 0 {
                    let dump_target = exe_fmt.unwrap_or_else(|| std::path::PathBuf::from("pdflatex.fmt"));
                    match tex_core::format::save_format(&eng, &dump_target) {
                        Ok(n) => eprintln!("PROG: format dumped to {} ({} bytes)", dump_target.display(), n),
                        Err(e) => eprintln!("PROG: format dump skipped: {}", e),
                    }
                } else {
                    eprintln!("PROG: boot had {} error(s); not dumping format", eng.error_count);
                }
            } else {
                eprintln!("LaTeX format boot failed; last file {} line {}", eng.input.current_file_name(), eng.input.current_file_line());
                print!("{}", eng.term);
                std::process::exit(1);
            }
        }
        // The format is settled (loaded or just dumped); the user file runs in
        // production mode, so a stray \dump cannot end the job silently.
        eng.ini_mode = false;
        eng.end_occurred = false;
        eng.input.stack.clear();
        // \\csname luatexversion\\endcsname poisons the name as \\relax
        // (tex.web §372). color.cfg then takes the luatex branch.
        // pdfTeX identity: those names must compare \\ifx-equal \\@undefined.
        for name in [
            b"luatexversion" as &[u8],
            b"luatexrevision",
            b"luatexbanner",
            b"directlua",
            b"outputmode",
            b"tex_luatexversion:D",
            b"tex_directlua:D",
        ] {
            if let Some(id) = eng.cs.lookup(name) {
                if matches!(
                    eng.eqtb.get(id),
                    Some(tex_core::eqtb::Equiv::Prim(tex_core::prim::Prim::Relax)) | None
                ) {
                    eng.eqtb.undefine(id, true);
                }
            }
        }
        let ov = eng.cs.intern(b"overline");
        eng.eqtb.assign(ov, tex_core::eqtb::Equiv::Prim(tex_core::prim::Prim::Overline), true);
        let un = eng.cs.intern(b"underline");
        eng.eqtb.assign(un, tex_core::eqtb::Equiv::Prim(tex_core::prim::Prim::Underline), true);
        // \\the\\spacefactor is a Knuth special integer; missing it
        // THESCAN-fails inside newtx .fd files and explodes pushback.
        let sf = eng.cs.intern(b"spacefactor");
        eng.eqtb.assign(sf, tex_core::eqtb::Equiv::CountReg(250), true);
        if eng.eqtb.count.len() > 250 {
            eng.eqtb.count[250] = 1000;
        }

    } else {
        let _ = eng.hyphen_trie.load_hyphen_file(std::path::Path::new("/usr/share/texmf-dist/tex/generic/hyphen/hyphen.tex"));
        eng.eqtb.dim_params[DimParam::HSize.idx() as usize] = (6.25 * 72.27 * 65536.0) as i32;
        eng.eqtb.dim_params[DimParam::VSize.idx() as usize] = 0;
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
    if std::env::var("CATTRACE").map(|v| v == "1").unwrap_or(false) {
        eprintln!(
            "CATS d={} o={} c={} a={} _={} @={} :={} ~={} space={}",
            eng.eqtb.cat[b'd' as usize],
            eng.eqtb.cat[b'o' as usize],
            eng.eqtb.cat[b'c' as usize],
            eng.eqtb.cat[b'a' as usize],
            eng.eqtb.cat[b'_' as usize],
            eng.eqtb.cat[b'@' as usize],
            eng.eqtb.cat[b':' as usize],
            eng.eqtb.cat[b'~' as usize],
            eng.eqtb.cat[b' ' as usize]
        );
    }
    if eng.input_file(&file) {
        // Knuth: everyjob is inserted on top of the * file so it runs first.
        if !plain && !ini {
            // Format \\everyjob contains \\directlua{...}. Install the
            // swallow-group stub for that, then \\let it to \\@undefined
            // so color.cfg / iftex see a pdfTeX engine.
            let dl = eng.cs.intern(b"directlua");
            eng.eqtb.assign(
                dl,
                tex_core::eqtb::Equiv::Prim(tex_core::prim::Prim::DirectLua),
                true,
            );
            if let Some(let_id) = eng.cs.lookup(b"let") {
                let undef = eng.cs.intern(b"@undefined");
                eng.input.push_toks(
                    vec![
                        tex_core::token::Token::from_cs(let_id),
                        tex_core::token::Token::from_cs(dl),
                        tex_core::token::Token::from_cs(undef),
                    ],
                    "<pdftex-not-luatex>",
                );
            }
            if let Some(def_id) = eng.cs.lookup(b"def") {
                // xcolor `\\providecommand*\\rangeRGB{255}` is a no-op if
                // the name was csname-poisoned to \\relax; force pdfTeX
                // defaults so \\ifnum\\rangeRGB=255 takes the RGB driver.
                for (name, body) in [
                    (b"rangeRGB" as &[u8], b"255" as &[u8]),
                    (b"rangeHSB", b"240"),
                    (b"rangeHsb", b"360"),
                    (b"rangeGray", b"15"),
                ] {
                    let id = eng.cs.intern(name);
                    let mut toks = vec![
                        tex_core::token::Token::from_cs(def_id),
                        tex_core::token::Token::from_cs(id),
                        tex_core::token::Token::char(1, b'{' as u32),
                    ];
                    for &b in body {
                        toks.push(tex_core::token::Token::char(12, b as u32));
                    }
                    toks.push(tex_core::token::Token::char(2, b'}' as u32));
                    eng.input.push_toks(toks, "<pdftex-range>");
                }
            }
            let ej = (*eng.eqtb.tok_params[tex_core::prim::ToksParam::EveryJob.idx() as usize]).clone();
            if !ej.is_empty() {
                eng.input.push_toks(ej, "<everyjob>");
            }
        }
        eng.run();
    }
    if std::env::var("MATHFAMDUMP").is_ok_and(|v| !v.is_empty() && v != "0") {
        for sz in 0..3 {
            for fam in 0..8 {
                let fid = eng.eqtb.style_fonts[sz][fam];
                if fid != 0 {
                    let (nm, tfm) = eng.eqtb.fonts.get(fid as usize).map(|f| (f.name.clone(), f.tfm_name.clone())).unwrap_or_default();
                    eprintln!("FAM sz={} fam={} -> fid={} name={} tfm={}", sz, fam, fid, nm, tfm);
                }
            }
        }
        eprintln!("MATHCODE of %: {:#06x}", eng.eqtb.math_code[b'%' as usize]);
    }
    // -ini mode: the file ended in \dump — write the format and exit,
    // like initex does.
    if ini && eng.format_done {
        match tex_core::format::save_format(&eng, std::path::Path::new("pdflatex.fmt")) {
            Ok(n) => eprintln!("PROG: format dumped to pdflatex.fmt ({} bytes)", n),
            Err(e) => {
                print!("{}", eng.term);
                eprintln!("PROG: format dump failed: {}", e);
                std::process::exit(1);
            }
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
        if !eng.out_dir.is_empty() {
            let _ = std::fs::create_dir_all(eng.out_dir.trim_end_matches('/'));
        }
        std::fs::write(&out, &pdf).expect("write pdf");
        println!("\nOutput written on {} ({} bytes).", out, pdf.len());
        std::process::exit(if eng.error_count > 0 { 1 } else { 0 });
    } else {
        eprintln!("No pages of output.");
        std::process::exit(if eng.error_count > 0 { 1 } else { 0 });
    }
}
