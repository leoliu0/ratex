//! Shared pdfLaTeX setup and complete font/image/PDF finalization.
use crate::prim::IntParam;
use crate::{pdffile, Engine};

pub fn png_embed_options(
    optimize_pdf_size: bool,
    requested_level: u32,
) -> crate::pdf_images::PngEmbedOptions {
    if optimize_pdf_size {
        crate::pdf_images::PngEmbedOptions::size(requested_level.min(9))
    } else {
        // Level 3 is the measured throughput/size knee. Higher levels belong
        // to the explicit size path, where the extra trials are intentional.
        crate::pdf_images::PngEmbedOptions::speed(requested_level.min(3))
    }
}

pub fn install_pdftex_config_registers(engine: &mut Engine) {
    for (name, register) in [
        (b"pdfdecimaldigits" as &[u8], 250),
        (b"pdfpkresolution" as &[u8], 251),
        (b"synctex" as &[u8], 252),
        (b"pdftracingfonts" as &[u8], 256),
        (b"pdfdraftmode" as &[u8], 257),
    ] {
        let id = engine.cs.intern(name);
        if matches!(
            engine.eqtb.get(id),
            None | Some(crate::eqtb::Equiv::Prim(crate::prim::Prim::Relax))
        ) {
            engine
                .eqtb
                .assign(id, crate::eqtb::Equiv::CountReg(register), true);
        }
    }
}

pub fn finalize_format_load(eng: &mut Engine) {
    install_pdftex_config_registers(eng);
    // Real LaTeX starts each document with \baselineskip=0pt (tex.web §224);
    // font sizes (\normalsize, etc.) set it when the document class is loaded.
    eng.eqtb.glue_params[crate::prim::GlueParam::BaselineSkip.idx() as usize] =
        crate::boxes::Glue::zero();
    // Format images predating the italic-correction primitive
    // stored LaTeX's `\@@italiccorr` as `\relax`. Rebind both
    // names so loaded and freshly bootstrapped formats agree.
    for name in [b"/" as &[u8], b"@@italiccorr"] {
        let id = eng.cs.intern(name);
        eng.eqtb.assign(
            id,
            crate::eqtb::Equiv::Prim(crate::prim::Prim::ItalicCorrection),
            true,
        );
    }
    for name in [
        b"pdfrandomseed" as &[u8],
        b"randomseed",
        b"tex_randomseed:D",
    ] {
        let id = eng.cs.intern(name);
        eng.eqtb.assign(
            id,
            crate::eqtb::Equiv::Prim(crate::prim::Prim::PdfRandomSeed),
            true,
        );
    }
    for name in [
        b"pdfsetrandomseed" as &[u8],
        b"setrandomseed",
        b"tex_setrandomseed:D",
    ] {
        let id = eng.cs.intern(name);
        eng.eqtb.assign(
            id,
            crate::eqtb::Equiv::Prim(crate::prim::Prim::PdfSetRandomSeed),
            true,
        );
    }
    // Compat shim for formats dumped before the
    // active-char namespace split: their boot wrote the
    // kernel tie to the hash slot, where encoding
    // defaults (\DeclareTextAccentDefault) later
    // clobbered it, so no usable tie survives on either
    // slot. Synthesize latex.ltx:9414's protected tie
    // directly on the active slot; no-op when the format
    // already carries one (post-split dumps).
    let act = eng.cs.intern(&crate::engine::Engine::active_cs_name(b'~'));
    if eng.eqtb.get(act).is_none() {
        let id_of = |eng: &crate::engine::Engine, name: &[u8]| eng.cs.lookup(name);
        let tie: Option<crate::eqtb::Equiv> = {
            let ifincs = id_of(eng, b"ifincsname");
            let expafter = id_of(eng, b"expandafter");
            let nobreak = id_of(eng, b"nobreakspace");
            let fi = id_of(eng, b"fi");
            match (ifincs, expafter, nobreak, fi) {
                (Some(a), Some(b), Some(c), Some(d)) => {
                    let body = vec![
                        crate::token::Token::from_cs(a),
                        crate::token::Token::from_cs(b),
                        crate::token::Token::char(13, b'~' as u32),
                        crate::token::Token::from_cs(d),
                        crate::token::Token::from_cs(b),
                        crate::token::Token::from_cs(c),
                        crate::token::Token::from_cs(d),
                    ];
                    Some(crate::eqtb::Equiv::Macro(std::rc::Rc::new(
                        crate::eqtb::Macro {
                            replacement: Default::default(),
                            num_params: 0,
                            has_param_refs: false,
                            params: Vec::new(),
                            prefix: Vec::new(),
                            body: body.into(),
                            long: false,
                            outer: false,
                            protected: true,
                        },
                    )))
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
    eng.eqtb.dim_params[crate::prim::DimParam::PageGoal.idx() as usize] = 0x3FFF_FFFF;
    eng.page_goal_set = false;
}

pub fn prepare_latex_job(eng: &mut Engine) {
    // The format is settled (loaded or just dumped); the user file runs in
    // production mode, so a stray \dump cannot end the job silently.
    eng.ini_mode = false;
    eng.end_occurred = false;
    eng.explicit_end_seen = false;
    eng.reset_job_diagnostics();
    eng.main_steps = 0;
    eng.expansion_steps = 0;
    eng.term.clear();
    eng.log.clear();
    eng.input.clear_sources();
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
        b"XeTeXversion",
        b"XeTeXrevision",
        b"XeTeXfonttype",
        b"XeTeXglyph",
        b"XeTeXglyphindex",
        b"XeTeXglyphname",
        b"XeTeXpicfile",
        b"XeTeXpdffile",
        b"xetexversion",
        b"xetexrevision",
    ] {
        if let Some(id) = eng.cs.lookup(name) {
            eng.eqtb.undefine(id, true);
        }
    }
    if let Some(id) = eng.cs.lookup(b"undefined") {
        eng.eqtb.undefine(id, true);
    }
    let lang = eng.cs.intern(b"languagename");
    if eng.eqtb.get(lang).is_none() {
        eng.eqtb.assign(
            lang,
            crate::eqtb::Equiv::Macro(std::rc::Rc::new(crate::eqtb::Macro {
                replacement: Default::default(),
                num_params: 0,
                has_param_refs: false,
                params: vec![],
                prefix: vec![],
                body: b"english"
                    .iter()
                    .map(|&c| crate::token::Token::letter(c))
                    .collect::<Vec<_>>()
                    .into(),
                long: false,
                outer: false,
                protected: false,
            })),
            true,
        );
    }
    let sp = eng.cs.intern(b" ");
    if eng.eqtb.get(sp).is_none() {
        eng.eqtb.assign(
            sp,
            crate::eqtb::Equiv::Prim(crate::prim::Prim::ExSpace),
            true,
        );
    }
    install_pdftex_config_registers(eng);
}

pub fn insert_everyjob(eng: &mut Engine) {
    // Format \\everyjob contains \\directlua{...}. Install the
    // swallow-group stub for that, then \\let it to \\@undefined
    // so color.cfg / iftex see a pdfTeX engine.
    let dl = eng.cs.intern(b"directlua");
    eng.eqtb.assign(
        dl,
        crate::eqtb::Equiv::Prim(crate::prim::Prim::DirectLua),
        true,
    );
    if let Some(let_id) = eng.cs.lookup(b"let") {
        let undef = eng.cs.intern(b"@undefined");
        eng.push_tokens_named(
            vec![
                crate::token::Token::from_cs(let_id),
                crate::token::Token::from_cs(dl),
                crate::token::Token::from_cs(undef),
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
                crate::token::Token::from_cs(def_id),
                crate::token::Token::from_cs(id),
                crate::token::Token::char(1, b'{' as u32),
            ];
            for &b in body {
                toks.push(crate::token::Token::char(12, b as u32));
            }
            toks.push(crate::token::Token::char(2, b'}' as u32));
            eng.push_tokens_named(toks, "<pdftex-range>");
        }
    }
    let ej = (*eng.eqtb.tok_params[crate::prim::ToksParam::EveryJob.idx() as usize]).clone();
    if !ej.is_empty() {
        eng.push_tokens_named(ej, "<everyjob>");
    }
}

pub fn finish_pdf(eng: &mut Engine, optimize_pdf_size: bool) -> Result<Vec<u8>, String> {
    // embed fonts used
    use std::collections::BTreeSet;
    let mut used: BTreeSet<u16> = BTreeSet::new();
    for fonts in eng
        .pdf_doc
        .pages
        .iter()
        .map(|p| &p.fonts)
        .chain(eng.pdf_doc.form_fonts.iter().map(|(_, fonts)| fonts))
    {
        for (fid, _) in fonts {
            used.insert(*fid as u16);
        }
    }
    let mut fidx: Vec<(u16, usize)> = Vec::new();
    for (n, fid) in used.iter().enumerate() {
        if let Some(font) = eng.eqtb.fonts.get(*fid as usize) {
            let pfb_bytes = font
                .type1_path
                .as_ref()
                .and_then(|name| eng.font_loader.kpse.read(name, tex_kpse::Format::Type1));
            let widths = (0..=255u8)
                .map(|c| {
                    let w = font.char_width(c);
                    if font.at_size != 0 {
                        ((w as i64 * 10_000 + font.at_size as i64 / 2) / font.at_size as i64) as i32
                    } else {
                        0
                    }
                })
                .collect();
            let mut ef = pdffile::make_embed_font(
                font.map_fontname
                    .clone()
                    .unwrap_or_else(|| font.tfm_name.clone()),
                pfb_bytes.as_deref(),
                font.encoding.as_deref(),
                0,
                255,
                widths,
            );
            pdffile::set_font_usage(
                &mut ef,
                eng.pdf_doc
                    .font_chars
                    .get(&(*fid as usize))
                    .copied()
                    .unwrap_or([0; 4]),
            );
            if font.at_size != 0 {
                let to_units =
                    |val: i32| -> f64 { (val as f64 * 1000.0 / font.at_size as f64).round() };
                let (ta, td, tc, ts) = crate::pdf_fonts::tfm_descriptor(font);
                let asc = to_units(font.char_height(b'd'));
                let cap = to_units(font.char_height(b'H'));
                let desc = -to_units(font.char_depth(b'p'));
                ef.ascent = if asc > 0.0 { asc } else { ta };
                ef.cap_height = if cap > 0.0 { cap } else { tc };
                ef.descent = if ef.ascent == 0.0 {
                    0.0
                } else if desc != 0.0 {
                    desc
                } else {
                    td
                };
                if ef.ascent - ef.descent > 3000.0 {
                    ef.descent = ef.ascent - 3000.0;
                }
                ef.stem_v = ts.max(100.0);
            }
            eng.pdf_doc.fonts.push(ef);
            fidx.insert(n, (*fid, n));
        }
    }
    // remap page font indices: pages reference engine font ids; convert to doc font index
    for fonts in eng
        .pdf_doc
        .pages
        .iter_mut()
        .map(|p| &mut p.fonts)
        .chain(eng.pdf_doc.form_fonts.iter_mut().map(|(_, fonts)| fonts))
    {
        for pf in fonts.iter_mut() {
            if let Some(pos) = fidx.iter().find(|(fid, _)| *fid == pf.0 as u16) {
                pf.0 = pos.1;
            }
        }
    }
    // embed image XObjects
    struct ImageJob<'a> {
        object: i32,
        mask: i32,
        bytes: Vec<u8>,
        path: &'a str,
    }
    fn embed_chunk(
        jobs: &[ImageJob<'_>],
        options: crate::pdf_images::PngEmbedOptions,
    ) -> Result<Vec<crate::pdf_images::EmbeddedImage>, String> {
        let mut objects = Vec::with_capacity(jobs.len().saturating_mul(2));
        for job in jobs {
            let mut next = job.mask;
            let embedded = if job.bytes.starts_with(&[0xff, 0xd8]) {
                crate::pdf_images::embed_jpeg(&job.bytes, job.object).map(|image| vec![image])
            } else if crate::pdf_svg::is_svg(&job.bytes) {
                if let Some(png_bytes) = crate::pdf_svg::svg_to_png(&job.bytes) {
                    crate::pdf_images::embed_png_with_options(
                        &png_bytes, job.object, &mut next, options,
                    )
                } else {
                    None
                }
            } else {
                crate::pdf_images::embed_png_with_options(
                    &job.bytes, job.object, &mut next, options,
                )
            }
            .ok_or_else(|| format!("Unsupported or invalid image: {}", job.path))?;
            objects.extend(embedded);
        }
        Ok(objects)
    }
    let mut used_images: Vec<_> = eng
        .pdf_images
        .iter()
        .filter(|(_, image)| image.used)
        .map(|(object, image)| (*object, image.clone()))
        .collect();
    used_images.sort_unstable_by_key(|(object, _)| *object);
    let compression_level =
        eng.eqtb.int_params[IntParam::PdfCompressLevel.idx() as usize].clamp(0, 9) as u32;
    let png_options = png_embed_options(optimize_pdf_size, compression_level);
    let mut next_obj = eng.pdf_next_obj;
    let mut jobs = Vec::with_capacity(used_images.len());
    for (object, image) in &used_images {
        // PDF page resources were imported while scanning \pdfximage.
        if image.embedded {
            continue;
        }
        let bytes = match tex_kpse::fs::read(&image.path) {
            Ok(bytes) => bytes,
            Err(error) => return Err(format!("Cannot read image `{}`: {error}", image.path)),
        };
        eng.record_loaded_bytes(std::path::Path::new(&image.path), &bytes);
        let mask = next_obj;
        if crate::pdf_images::png_needs_soft_mask(&bytes) {
            next_obj = match next_obj.checked_add(1) {
                Some(value) => value,
                None => return Err("PDF image object number overflow".into()),
            };
        }
        jobs.push(ImageJob {
            object: *object,
            mask,
            bytes,
            path: &image.path,
        });
    }
    #[cfg(target_arch = "wasm32")]
    let workers = 1;
    #[cfg(not(target_arch = "wasm32"))]
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(8)
        .min(jobs.len());
    let embedded_result = if workers <= 1 {
        embed_chunk(&jobs, png_options)
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = jobs
                .chunks(jobs.len().div_ceil(workers))
                .map(|chunk| scope.spawn(move || embed_chunk(chunk, png_options)))
                .collect();
            let mut objects = Vec::with_capacity(jobs.len().saturating_mul(2));
            let mut error = None;
            for handle in handles {
                match handle.join() {
                    Ok(Ok(chunk)) => objects.extend(chunk),
                    Ok(Err(message)) => error = Some(message),
                    Err(_) => error = Some("Image embedding worker panicked".to_string()),
                }
            }
            match error {
                Some(message) => Err(message),
                None => Ok(objects),
            }
        })
    };
    let embedded = match embedded_result {
        Ok(embedded) => embedded,
        Err(error) => return Err(error),
    };
    for image in embedded {
        eng.pdf_doc.objects.push((image.obj_num, image.bytes));
    }
    let pdf = pdffile::write_pdf(&eng.pdf_doc);

    Ok(pdf)
}
