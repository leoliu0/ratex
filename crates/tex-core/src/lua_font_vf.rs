//! `font.read_vf` (luatex `make_vf_table`) and the scaling of virtual-font
//! quantities (`store_scaled_f`).

use tex_lua::{CallbackLua, LuaBytes, LuaTable};

/// luatex `store_scaled_f`: scale the fix_word-like quantity `sq` by the
/// font size `z_in` (the TFM `store_scaled` algorithm on an `i32`).
pub(crate) fn store_scaled_f(sq: i32, z_in: i32) -> i32 {
    let mut z = i64::from(z_in);
    let mut alpha: i64 = 16;
    while z >= 0x80_0000 {
        z /= 2;
        alpha += alpha;
    }
    let beta = 256 / alpha;
    alpha *= z;
    let (a, b, c, d) = {
        let mut q = i64::from(sq);
        if q >= 0 {
            let d = q % 256;
            q /= 256;
            let c = q % 256;
            q /= 256;
            let b = q % 256;
            q /= 256;
            (q % 256, b, c, d)
        } else {
            q = (q + 1_073_741_824) + 1_073_741_824;
            let d = q % 256;
            q /= 256;
            let c = q % 256;
            q /= 256;
            let b = q % 256;
            q /= 256;
            ((q + 128) % 256, b, c, d)
        }
    };
    let sw = (((((d * z) >> 8) + (c * z)) >> 8) + (b * z)) / beta;
    (if a == 0 { sw } else { sw - alpha }) as i32
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u32, String> {
        let b = *self.data.get(self.pos).ok_or("unexpected end of file")?;
        self.pos += 1;
        Ok(u32::from(b))
    }

    fn unsigned(&mut self, n: usize) -> Result<u32, String> {
        let mut v = 0u32;
        for _ in 0..n {
            v = v.wrapping_mul(256).wrapping_add(self.byte()?);
        }
        Ok(v)
    }

    fn signed(&mut self, n: usize) -> Result<i32, String> {
        let first = self.byte()?;
        let mut v = if first > 127 { first as i32 - 256 } else { first as i32 };
        for _ in 1..n {
            v = v.wrapping_mul(256).wrapping_add(self.byte()? as i32);
        }
        Ok(v)
    }
}

fn command(cx: &mut CallbackLua<'_>, list: &LuaTable, k: &mut i64, name: &str, args: &[i64]) -> Result<(), String> {
    let e = cx.create_table().map_err(|e| format!("{e:?}"))?;
    e.raw_seti(1, LuaBytes(name.as_bytes().to_vec())).map_err(|e| format!("{e:?}"))?;
    for (i, a) in args.iter().enumerate() {
        e.raw_seti(i as i64 + 2, *a).map_err(|e| format!("{e:?}"))?;
    }
    list.raw_seti(*k, e).map_err(|e| format!("{e:?}"))?;
    *k += 1;
    Ok(())
}

/// luatex `make_vf_table`. Malformed files are reported as errors.
pub(crate) fn vf_table(cx: &mut CallbackLua<'_>, name: &[u8], data: &[u8], atsize: i32) -> Result<LuaTable, String> {
    let mut r = Reader { data, pos: 0 };
    let t = cx.create_table().map_err(|e| format!("{e:?}"))?;
    let set = |t: &LuaTable, k: &str, v: i64| t.raw_set(k, v).map_err(|e| format!("{e:?}"));
    if r.byte()? != 247 {
        return Err("PRE command expected".to_string());
    }
    if r.byte()? != 202 {
        return Err("wrong id byte".to_string());
    }
    let hl = r.byte()? as usize;
    let mut header = Vec::with_capacity(hl);
    for _ in 0..hl {
        header.push(r.byte()? as u8);
    }
    t.raw_set("header", LuaBytes(header)).map_err(|e| format!("{e:?}"))?;
    set(&t, "checksum", i64::from(r.unsigned(4)?))?;
    let ds = r.signed(4)? / 16;
    set(&t, "designsize", i64::from(ds))?;
    t.raw_set("name", LuaBytes(name.to_vec())).map_err(|e| format!("{e:?}"))?;
    set(&t, "size", i64::from(atsize))?;

    let mut cmd = r.byte()?;
    let fonts = cx.create_table().map_err(|e| format!("{e:?}"))?;
    let mut any_font = false;
    while (243..=246).contains(&cmd) {
        let entry = cx.create_table().map_err(|e| format!("{e:?}"))?;
        let nf = r.unsigned((cmd - 243 + 1) as usize)? as i64 + 1;
        r.unsigned(4)?; // checksum
        let fs = store_scaled_f(r.signed(4)?, atsize);
        entry.raw_set("size", i64::from(fs)).map_err(|e| format!("{e:?}"))?;
        r.signed(4)?; // design size
        let area = r.byte()?;
        let len = r.byte()?;
        for _ in 0..area {
            r.byte()?;
        }
        let mut fname = Vec::with_capacity(len as usize);
        for _ in 0..len {
            fname.push(r.byte()? as u8);
        }
        entry.raw_set("name", LuaBytes(fname)).map_err(|e| format!("{e:?}"))?;
        fonts.raw_seti(nf, entry).map_err(|e| format!("{e:?}"))?;
        any_font = true;
        cmd = r.byte()?;
    }
    if any_font {
        t.raw_set("fonts", fonts).map_err(|e| format!("{e:?}"))?;
    }

    let characters = cx.create_table().map_err(|e| format!("{e:?}"))?;
    while cmd <= 242 {
        let (mut packet_length, cc, tfm_width);
        if cmd == 242 {
            packet_length = r.unsigned(4)?;
            cc = r.unsigned(4)? as i64;
            tfm_width = i64::from(r.signed(4)?);
        } else {
            packet_length = cmd;
            cc = i64::from(r.byte()?);
            tfm_width = i64::from(r.unsigned(3)?);
        }
        let entry = cx.create_table().map_err(|e| format!("{e:?}"))?;
        set(&entry, "width", tfm_width)?;
        let commands = cx.create_table().map_err(|e| format!("{e:?}"))?;
        let mut k = 1i64;
        let mut vf_nf = 0i64;
        let (mut w, mut x, mut y, mut z) = (0i32, 0i32, 0i32, 0i32);
        let mut stack: Vec<(i32, i32, i32, i32)> = Vec::new();
        let sc = |v: i32| i64::from(store_scaled_f(v, atsize));
        while packet_length > 0 {
            let c = r.byte()?;
            packet_length -= 1;
            if c < 128 {
                if vf_nf == 0 {
                    vf_nf = 1;
                    command(cx, &commands, &mut k, "font", &[vf_nf])?;
                }
                command(cx, &commands, &mut k, "char", &[i64::from(c)])?;
            } else if (171..=234).contains(&c) || (235..=238).contains(&c) {
                if c >= 235 {
                    let n = (c - 235 + 1) as usize;
                    vf_nf = r.unsigned(n)? as i64 + 1;
                    packet_length = packet_length.saturating_sub(n as u32);
                } else {
                    vf_nf = i64::from(c - 171 + 1);
                }
                command(cx, &commands, &mut k, "font", &[vf_nf])?;
            } else {
                match c {
                    132 | 137 => {
                        let h = r.signed(4)?;
                        let v = r.signed(4)?;
                        if c == 137 {
                            command(cx, &commands, &mut k, "push", &[])?;
                        }
                        command(cx, &commands, &mut k, "rule", &[sc(h), sc(v)])?;
                        if c == 137 {
                            command(cx, &commands, &mut k, "pop", &[])?;
                        }
                        packet_length = packet_length.saturating_sub(8);
                    }
                    128..=131 | 133..=136 => {
                        let base = if c <= 131 { 128 } else { 133 };
                        let n = (c - base + 1) as usize;
                        if vf_nf == 0 {
                            vf_nf = 1;
                            command(cx, &commands, &mut k, "font", &[vf_nf])?;
                        }
                        let ch = i64::from(r.unsigned(n)?);
                        if c >= 133 {
                            command(cx, &commands, &mut k, "push", &[])?;
                        }
                        command(cx, &commands, &mut k, "char", &[ch])?;
                        if c >= 133 {
                            command(cx, &commands, &mut k, "pop", &[])?;
                        }
                        packet_length = packet_length.saturating_sub(n as u32);
                    }
                    143..=146 | 148..=151 | 153..=156 => {
                        let (base, which) = match c {
                            143..=146 => (143, 0),
                            148..=151 => (148, 1),
                            _ => (153, 2),
                        };
                        let n = (c - base + 1) as usize;
                        let v = r.signed(n)?;
                        match which {
                            1 => w = v,
                            2 => x = v,
                            _ => {}
                        }
                        command(cx, &commands, &mut k, "right", &[sc(v)])?;
                        packet_length = packet_length.saturating_sub(n as u32);
                    }
                    157..=160 | 162..=165 | 167..=170 => {
                        let (base, which) = match c {
                            157..=160 => (157, 0),
                            162..=165 => (162, 1),
                            _ => (167, 2),
                        };
                        let n = (c - base + 1) as usize;
                        let v = r.signed(n)?;
                        match which {
                            1 => y = v,
                            2 => z = v,
                            _ => {}
                        }
                        command(cx, &commands, &mut k, "down", &[sc(v)])?;
                        packet_length = packet_length.saturating_sub(n as u32);
                    }
                    239..=242 => {
                        let n = (c - 239 + 1) as usize;
                        let len = r.unsigned(n)?;
                        packet_length = packet_length.saturating_sub(n as u32);
                        if len == 0 {
                            return Err("special of negative length".to_string());
                        }
                        packet_length = packet_length.saturating_sub(len);
                        let mut s = Vec::with_capacity(len as usize);
                        for _ in 0..len {
                            s.push(r.byte()? as u8);
                        }
                        let e = cx.create_table().map_err(|e| format!("{e:?}"))?;
                        e.raw_seti(1, LuaBytes(b"special".to_vec())).map_err(|e| format!("{e:?}"))?;
                        e.raw_seti(2, LuaBytes(s)).map_err(|e| format!("{e:?}"))?;
                        commands.raw_seti(k, e).map_err(|e| format!("{e:?}"))?;
                        k += 1;
                    }
                    147 => command(cx, &commands, &mut k, "right", &[sc(w)])?,
                    152 => command(cx, &commands, &mut k, "right", &[sc(x)])?,
                    161 => command(cx, &commands, &mut k, "down", &[sc(y)])?,
                    166 => command(cx, &commands, &mut k, "down", &[sc(z)])?,
                    138 => {}
                    141 => {
                        if stack.len() == 256 {
                            return Err("virtual font stack size".to_string());
                        }
                        stack.push((w, x, y, z));
                        command(cx, &commands, &mut k, "push", &[])?;
                    }
                    142 => {
                        let Some((sw, sx, sy, sz)) = stack.pop() else {
                            return Err("more POPs than PUSHs in character".to_string());
                        };
                        w = sw;
                        x = sx;
                        y = sy;
                        z = sz;
                        command(cx, &commands, &mut k, "pop", &[])?;
                    }
                    _ => return Err("improver DVI command".to_string()),
                }
            }
        }
        entry.raw_set("commands", commands).map_err(|e| format!("{e:?}"))?;
        if !stack.is_empty() {
            return Err("more PUSHs than POPs in character packet".to_string());
        }
        characters.raw_seti(cc, entry).map_err(|e| format!("{e:?}"))?;
        cmd = r.byte()?;
    }
    t.raw_set("characters", characters).map_err(|e| format!("{e:?}"))?;
    if cmd != 248 {
        return Err("POST command expected".to_string());
    }
    Ok(t)
}
