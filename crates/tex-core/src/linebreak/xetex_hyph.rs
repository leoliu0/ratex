//! XeTeX's version of tex.web §891-§903 "Try to hyphenate the following
//! word" (xetex.web §21499-§21833): the word is found through the
//! hyphenation codes of the language (`\savinghyphcodes`, `\lccode`s of any
//! Unicode scalar), may be at most `\XeTeXhyphenatablelength` characters
//! long, and is either a string of TFM characters and ligatures or a
//! native-font word.

use super::*;

/// What xetex.web's hyphenation routines read from the current language.
struct XeCtx<'a> {
    /// the language's patterns and exceptions; `None` when it has neither
    trie: Option<&'a crate::hyphen::Trie>,
    /// the language's saved hyphenation codes (`hyph_index`)
    codes: Option<&'a [u16; 256]>,
    lh: usize,
    rh: usize,
    uc_hyph: bool,
    /// `max_hyphenatable_length`
    max_len: usize,
    /// `max_hyph_char`
    max_hyph_char: u32,
}

impl Engine {
    fn xe_ctx(&self, st: LangState) -> XeCtx<'_> {
        XeCtx {
            trie: self.trie_for_language(st.lang).filter(|t| !t.is_empty()),
            codes: self.xe_hyph_codes(st.lang),
            lh: usize::from(st.lhm),
            rh: usize::from(st.rhm),
            uc_hyph: self.eqtb.int_params[IntParam::UcHyph.idx() as usize] > 0,
            max_len: self.xe_max_hyphenatable_length(),
            max_hyph_char: self.xe_hyph.max_pattern_char + 1,
        }
    }

    /// tex.web §863/§866 as xetex.web applies it: every glue node outside
    /// math starts a hyphenation attempt on the word after it.
    pub(super) fn xetex_hyphenate_list(&mut self, list: &mut NodeList) {
        // "Initialize for hyphenating a paragraph": init_trie
        self.xe_hyph.trie_packed = true;
        let mut lang = self.paragraph_language();
        // (first replaced index, end index, replacement)
        let mut edits: Vec<(usize, usize, NodeList)> = Vec::new();
        let mut auto_breaking = true;
        let mut i = 0;
        while i < list.len() {
            match &list[i] {
                Node::MathKern(_, kind @ 1..=4, _) => auto_breaking = crate::boxes::math_end_lr(*kind),
                Node::Whatsit(WhatIt::Language { lang: l, lhm, rhm }, _) => {
                    lang = LangState {
                        lang: *l,
                        lhm: *lhm,
                        rhm: *rhm,
                    };
                }
                Node::Glue(_, _) | Node::Leaders { .. } if auto_breaking => {
                    if let Some(edit) = self.xetex_hyphenate_word_after(list, i, &mut lang) {
                        i = edit.1;
                        edits.push(edit);
                        continue;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        for (start, end, nodes) in edits.into_iter().rev() {
            list.splice(start..end, nodes);
        }
    }

    /// xetex.web §21499 for the glue at `g`: the replacement for
    /// `list[start..end]` when the word after it was changed.
    fn xetex_hyphenate_word_after(
        &self,
        list: &[Node],
        g: usize,
        lang: &mut LangState,
    ) -> Option<(usize, usize, NodeList)> {
        let mut ctx = self.xe_ctx(*lang);
        // "Skip to node ha, or goto done1 if no hyphenation should be
        // attempted"
        let mut ha = g;
        let mut s = g + 1;
        let hf = loop {
            let (c, f) = match list.get(s)? {
                Node::Char { c, font, .. } => (u32::from(*c), *font),
                Node::Ligature {
                    letters,
                    n_letters,
                    font,
                    ..
                } if *n_letters > 0 => (u32::from(letters[0]), *font),
                // §1363 adv_past in the pre-hyphenation loop
                Node::Whatsit(WhatIt::Language { lang: l, lhm, rhm }, _) => {
                    *lang = LangState {
                        lang: *l,
                        lhm: *lhm,
                        rhm: *rhm,
                    };
                    ctx = self.xe_ctx(*lang);
                    ha = s;
                    s += 1;
                    continue;
                }
                Node::Ligature { .. } | Node::Kern(_, _) | Node::Whatsit(_, _) => {
                    ha = s;
                    s += 1;
                    continue;
                }
                Node::MathKern(_, kind, _) if *kind >= crate::boxes::LR_KIND_MIN => {
                    ha = s;
                    s += 1;
                    continue;
                }
                _ => return None,
            };
            let lc = self.xe_lc_code(ctx.codes, c);
            if lc != 0 {
                if lc == c || ctx.uc_hyph {
                    break f;
                }
                return None;
            }
            ha = s;
            s += 1;
        };
        let hyf_char = self.eqtb.hyphen_char.get(hf as usize).copied().unwrap_or(-1);
        // a TFM font cannot have a hyphen beyond 255, and without patterns
        // nothing is found in a TFM word
        let hyf_char = u8::try_from(hyf_char).ok()?;
        let trie = ctx.trie?;
        if ctx.lh + ctx.rh > ctx.max_len {
            return None;
        }
        let font = self.eqtb.fonts.get(hf as usize)?.clone();
        // "Skip to node hb, putting letters into hu and hc"
        let mut hu: Vec<u16> = vec![NON_CHAR];
        let mut hc: Vec<u32> = vec![0];
        let mut hn = 0usize;
        let mut hb = s;
        let first_attr = list[s].attr();
        let mut hyf_bchar: Option<u8> = None;
        'word: loop {
            match list.get(s) {
                Some(Node::Char { c, font: f, .. }) => {
                    if *f != hf {
                        break;
                    }
                    hyf_bchar = Some(*c);
                    let lc = self.xe_lc_code(ctx.codes, u32::from(*c));
                    if lc == 0 || lc > ctx.max_hyph_char || hn == ctx.max_len {
                        break;
                    }
                    hb = s;
                    hn += 1;
                    hu.push(u16::from(*c));
                    hc.push(lc);
                    hyf_bchar = None;
                }
                Some(Node::Ligature {
                    font: f,
                    letters,
                    n_letters,
                    subtype,
                    ..
                }) => {
                    if *f != hf {
                        break;
                    }
                    let mut j = hn;
                    if *n_letters > 0 {
                        hyf_bchar = Some(letters[0]);
                    }
                    for &c in &letters[..*n_letters as usize] {
                        let lc = self.xe_lc_code(ctx.codes, u32::from(c));
                        if lc == 0 || lc > ctx.max_hyph_char || j == ctx.max_len {
                            hu.truncate(hn + 1);
                            hc.truncate(hn + 1);
                            break 'word;
                        }
                        j += 1;
                        hu.push(u16::from(c));
                        hc.push(lc);
                    }
                    hb = s;
                    hn = j;
                    hyf_bchar = if subtype & 1 != 0 { font.bchar } else { None };
                }
                Some(Node::Kern(_, _)) => {
                    hb = s;
                    hyf_bchar = font.bchar;
                }
                _ => break,
            }
            s += 1;
        }
        // "Check that the nodes following hb permit hyphenation and that at
        // least l_hyf+r_hyf letters have been found"
        if hn < ctx.lh + ctx.rh {
            return None;
        }
        loop {
            match list.get(s) {
                Some(Node::Char { .. } | Node::Ligature { .. } | Node::Kern(_, _)) => s += 1,
                None
                | Some(
                    Node::ExplicitKern(_, _)
                    | Node::AccentKern(_, _)
                    | Node::ItalicKern(_, _)
                    | Node::Whatsit(_, _)
                    | Node::NativeGlyphRun { .. }
                    | Node::Glue(_, _)
                    | Node::Leaders { .. }
                    | Node::Penalty(_, _)
                    | Node::Ins { .. }
                    | Node::VAdjust(_, _)
                    | Node::PreAdjust(_, _)
                    | Node::Mark { .. },
                ) => break,
                Some(Node::MathKern(_, kind, _)) if *kind >= crate::boxes::LR_KIND_MIN => break,
                _ => return None,
            }
        }
        hu.truncate(hn + 1);
        hc.truncate(hn + 1);
        let hyf = Self::xe_find_hyphens(trie, &ctx, &hc, hn)?;
        let mut hu_buf = hu;
        hu_buf.resize(hn + 3, NON_CHAR);
        let mut hyf_buf = hyf;
        hyf_buf.resize(hn + 2, 0);
        Some(Self::replace_hyphenated_word(
            list,
            ha,
            hb,
            hf,
            &font,
            hu_buf,
            hyf_buf,
            hn,
            hyf_bchar,
            hyf_char,
            first_attr,
        ))
    }

    /// xetex.web §923 "Find hyphen locations for the word in hc" and "If no
    /// hyphens were found, return": `hyf[0..=hn]`, or `None` without a
    /// hyphen in `l_hyf..=hn-r_hyf`.
    fn xe_find_hyphens(
        trie: &crate::hyphen::Trie,
        ctx: &XeCtx<'_>,
        hc: &[u32],
        hn: usize,
    ) -> Option<Vec<u8>> {
        let mut hyf = vec![0u8; hn + 1];
        let word = &hc[1..=hn];
        if let Some(points) = trie.xe_exception(word) {
            for &k in points {
                if k <= hn {
                    hyf[k] = 1;
                }
            }
        } else {
            trie.xe_gap_values(word, &mut hyf);
        }
        for h in hyf.iter_mut().take(ctx.lh) {
            *h = 0;
        }
        for j in 0..ctx.rh.min(hn + 1) {
            hyf[hn - j] = 0;
        }
        (ctx.lh..=hn - ctx.rh)
            .any(|j| hyf[j] % 2 == 1)
            .then_some(hyf)
    }
}
