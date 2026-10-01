//! Sampled images (`image`, `colorimage`, `imagemask`) as PDF image XObjects.

use std::io::Write;

use crate::graphics::{put_matrix, ColorSpace, DevColor};
use crate::interp::{err, Interp, OpFn, Res};
use crate::types::{FxMap, Key, Matrix, PsDict, Value};

/// An image XObject for the output PDF.
pub(crate) struct ImageRes {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) bpc: u8,
    /// PDF colour space object; `None` for stencil masks.
    pub(crate) space: Option<String>,
    pub(crate) decode: Option<Vec<f64>>,
    pub(crate) interpolate: bool,
    pub(crate) filter: &'static str,
    pub(crate) data: Vec<u8>,
}

enum Space {
    Gray,
    Rgb,
    Cmyk,
    Indexed { hival: u32, rgb: Vec<u8> },
}

impl Space {
    fn pdf(&self) -> String {
        match self {
            Self::Gray => "/DeviceGray".into(),
            Self::Rgb => "/DeviceRGB".into(),
            Self::Cmyk => "/DeviceCMYK".into(),
            Self::Indexed { hival, rgb } => {
                let mut s = format!("[/Indexed /DeviceRGB {hival} <");
                for b in rgb {
                    s.push_str(&format!("{b:02X}"));
                }
                s.push_str(">]");
                s
            }
        }
    }
}

struct Spec {
    width: u32,
    height: u32,
    bpc: u8,
    /// Colour space of the samples as supplied by the program.
    space: ColorSpace,
    decode: Option<Vec<f64>>,
    matrix: Matrix,
    sources: Vec<Value>,
    multi: bool,
    mask: bool,
    interpolate: bool,
}

fn dev_rgb(c: Option<DevColor>) -> [u8; 3] {
    let v = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    match c {
        Some(DevColor::Gray(g)) => [v(g); 3],
        Some(DevColor::Rgb(r, g, b)) => [v(r), v(g), v(b)],
        Some(DevColor::Cmyk(c, m, y, k)) => [v(1.0 - (c + k)), v(1.0 - (m + k)), v(1.0 - (y + k))],
        None => [0; 3],
    }
}

fn unpack(row: &[u8], bpc: u8, count: usize) -> Vec<u32> {
    let mut out = Vec::with_capacity(count);
    match bpc {
        8 => out.extend(row.iter().take(count).map(|&b| u32::from(b))),
        16 => out.extend(row.chunks_exact(2).take(count).map(|c| u32::from(c[0]) << 8 | u32::from(c[1]))),
        _ => {
            let bits = u32::from(bpc);
            let mask = (1u32 << bits) - 1;
            for k in 0..count {
                let bit = k * bits as usize;
                let v = if bits == 12 {
                    let byte = bit / 8;
                    let pair = u32::from(row.get(byte).copied().unwrap_or(0)) << 8
                        | u32::from(row.get(byte + 1).copied().unwrap_or(0));
                    if bit % 8 == 0 { pair >> 4 } else { pair & 0xFFF }
                } else {
                    let byte = row.get(bit / 8).copied().unwrap_or(0);
                    (u32::from(byte) >> (8 - bits - (bit % 8) as u32)) & mask
                };
                out.push(v);
            }
        }
    }
    out
}

fn pack(samples: &[u32], bpc: u8, out: &mut Vec<u8>) {
    match bpc {
        8 => out.extend(samples.iter().map(|&s| s as u8)),
        16 => out.extend(samples.iter().flat_map(|&s| [(s >> 8) as u8, s as u8])),
        _ => {
            let bits = u32::from(bpc);
            let mut acc = 0u32;
            let mut n = 0;
            for &s in samples {
                acc = acc << bits | s;
                n += bits;
                if n == 8 {
                    out.push(acc as u8);
                    acc = 0;
                    n = 0;
                }
            }
            if n > 0 {
                out.push((acc << (8 - n)) as u8);
            }
        }
    }
}

impl Interp {
    fn image_spec_from_dict(&mut self, dict: &PsDict, mask: bool) -> Res<Spec> {
        let get = |k| dict.get(&Key::Name(k));
        let num = |v: Option<Value>, what: &str| v.and_then(|v| v.as_f64()).map_or_else(|| err("rangecheck", format!("image dictionary needs /{what}")), Ok);
        let image_type = get(self.n.image_type).and_then(|v| v.as_f64()).unwrap_or(1.0);
        if image_type == 3.0 {
            // Masked image: render the image data, ignore the mask.
            let data = match dict.get(&self.key_bytes(b"DataDict")) {
                Some(Value::Dict(d)) => d,
                _ => return err("rangecheck", "ImageType 3 without DataDict"),
            };
            return self.image_spec_from_dict(&data, mask);
        }
        let width = num(get(self.n.width), "Width")?;
        let height = num(get(self.n.height), "Height")?;
        let bpc = if mask { 1.0 } else { num(get(self.n.bits_per_component), "BitsPerComponent")? };
        let matrix = match get(self.n.image_matrix) {
            Some(m) => self.matrix_from(&m)?,
            None => return err("rangecheck", "image dictionary needs /ImageMatrix"),
        };
        let decode = match get(self.n.decode) {
            Some(Value::Array(a) | Value::Proc(a)) => Some(self.numbers(&a)?),
            _ => None,
        };
        let multi = matches!(get(self.n.multiple_data_sources), Some(Value::Bool(true)));
        let sources = match get(self.n.data_source) {
            Some(Value::Array(a)) if multi => a.to_vec(),
            Some(v) => vec![v],
            None => return err("rangecheck", "image dictionary needs /DataSource"),
        };
        let interpolate = matches!(get(self.n.interpolate), Some(Value::Bool(true)));
        Ok(Spec {
            width: width as u32,
            height: height as u32,
            bpc: bpc as u8,
            space: if mask { ColorSpace::Gray } else { self.gs.space.clone() },
            decode,
            matrix,
            sources,
            multi,
            mask,
            interpolate,
        })
    }

    fn read_image_sources(&mut self, spec: &Spec, per_source: usize) -> Res<Vec<Vec<u8>>> {
        let n = spec.sources.len();
        let mut data: Vec<Vec<u8>> = vec![Vec::new(); n];
        let mut done = vec![false; n];
        while done.iter().any(|d| !d) {
            for k in 0..n {
                if done[k] {
                    continue;
                }
                let need = per_source - data[k].len();
                let before = data[k].len();
                match spec.sources[k].clone() {
                    Value::String(s) | Value::ExecString(s) => {
                        let bytes = s.to_vec();
                        if bytes.is_empty() {
                            done[k] = true;
                        }
                        data[k].extend(bytes.iter().cycle().take(need));
                    }
                    Value::File(f) | Value::ExecFile(f) => {
                        let mut buf = Vec::new();
                        self.read_bytes(f, need.min(1 << 16), &mut buf)?;
                        if buf.is_empty() {
                            done[k] = true;
                        }
                        data[k].extend_from_slice(&buf);
                    }
                    proc @ (Value::Proc(_) | Value::Operator(_) | Value::ExecName(_)) => {
                        self.call(proc)?;
                        match self.pop()? {
                            Value::String(s) | Value::ExecString(s) => {
                                if s.len == 0 {
                                    done[k] = true;
                                }
                                s.with(|b| data[k].extend_from_slice(&b[..b.len().min(need)]));
                            }
                            _ => return err("typecheck", "image data procedure must return a string"),
                        }
                    }
                    _ => return err("typecheck", "invalid image data source"),
                }
                self.alloc(data[k].len() - before)?;
                if data[k].len() >= per_source {
                    done[k] = true;
                }
            }
        }
        for d in &mut data {
            d.resize(per_source, 0);
        }
        Ok(data)
    }

    fn render_image(&mut self, spec: Spec) -> Res {
        if spec.width == 0 || spec.height == 0 {
            return Ok(());
        }
        if !matches!(spec.bpc, 1 | 2 | 4 | 8 | 12 | 16) {
            return err("rangecheck", "image BitsPerComponent");
        }
        let ncomp = if spec.mask { 1 } else { spec.space.ncomp() };
        if ncomp == 0 {
            return err("rangecheck", "image in a Pattern colour space");
        }
        if spec.multi && spec.sources.len() != ncomp {
            return err("rangecheck", "image needs one data source per component");
        }
        let (w, h) = (spec.width as usize, spec.height as usize);
        let bpc = usize::from(spec.bpc);
        let planes = if spec.multi { ncomp } else { 1 };
        let row_bytes = (w * bpc * (ncomp / planes)).div_ceil(8);
        let per_source = row_bytes.checked_mul(h).filter(|&n| n <= crate::interp::MAX_OBJECT_LEN * 8);
        let Some(per_source) = per_source else {
            return err("limitcheck", "image too large");
        };
        let Some(inverse) = spec.matrix.invert() else {
            return Ok(());
        };
        let placement = Matrix::new([spec.width as f64, 0.0, 0.0, -(spec.height as f64), 0.0, spec.height as f64])
            .then(&inverse)
            .then(&self.gs.ctm);

        // JPEG data passes through untouched.
        let dct = match spec.sources.first() {
            Some(Value::File(f) | Value::ExecFile(f)) if !spec.multi && self.files[*f as usize].is_dct() => Some(*f),
            _ => None,
        };
        let (data, bpc_out, space, decode, filter) = if let Some(f) = dct {
            let jpeg = self.read_dct(f)?;
            self.alloc(jpeg.len())?;
            let space = self.device_space(&spec.space)?;
            (jpeg, 8, Some(space.pdf()), spec.decode.clone(), "DCTDecode")
        } else {
            let sources = self.read_image_sources(&spec, per_source)?;
            let (samples, bpc_out, space, decode) = self.convert_samples(&spec, sources, ncomp, row_bytes)?;
            (compress(&samples), bpc_out, space, decode, "FlateDecode")
        };
        if !self.emitting() {
            return Ok(());
        }
        if spec.mask {
            let Some(c) = self.gs.device else { return Ok(()) };
            self.sync_fill(c);
        }
        let index = self.out.images.len();
        self.out.images.push(ImageRes {
            width: spec.width,
            height: spec.height,
            bpc: bpc_out,
            space,
            decode,
            interpolate: spec.interpolate,
            filter,
            data,
        });
        let out = &mut self.out.content;
        out.extend_from_slice(b"q\n");
        put_matrix(out, &placement);
        out.extend_from_slice(format!("/Im{} Do\nQ\n", index + 1).as_bytes());
        self.check_output()
    }

    /// The PDF colour space for device-like image spaces.
    fn device_space(&mut self, space: &ColorSpace) -> Res<Space> {
        Ok(match space {
            ColorSpace::Gray => Space::Gray,
            ColorSpace::Rgb => Space::Rgb,
            ColorSpace::Cmyk => Space::Cmyk,
            ColorSpace::Indexed { hival, .. } => {
                let mut rgb = Vec::with_capacity((*hival as usize + 1) * 3);
                for idx in 0..=*hival {
                    let c = self.resolve_color(space, &[f64::from(idx)], 0)?;
                    rgb.extend_from_slice(&dev_rgb(c));
                }
                Space::Indexed { hival: *hival, rgb }
            }
            ColorSpace::Separation { .. } | ColorSpace::DeviceN { .. } => Space::Rgb,
            ColorSpace::Pattern => return err("rangecheck", "image in a Pattern colour space"),
        })
    }

    /// Interleaves planes, narrows 12-bit samples, and converts spot colour
    /// spaces through their tint transforms into RGB.
    #[allow(clippy::type_complexity)]
    fn convert_samples(
        &mut self,
        spec: &Spec,
        sources: Vec<Vec<u8>>,
        ncomp: usize,
        row_bytes: usize,
    ) -> Res<(Vec<u8>, u8, Option<String>, Option<Vec<f64>>)> {
        let (w, h) = (spec.width as usize, spec.height as usize);
        let max = (1u32 << spec.bpc.min(16)) - 1;
        let decode = |k: usize, s: u32, d: &Option<Vec<f64>>, hi: f64| -> f64 {
            let (lo, hi) = d.as_ref().map_or((0.0, hi), |d| (d.get(2 * k).copied().unwrap_or(0.0), d.get(2 * k + 1).copied().unwrap_or(hi)));
            lo + f64::from(s) * (hi - lo) / f64::from(max.max(1))
        };
        if spec.mask {
            let decode = spec.decode.clone().or(Some(vec![0.0, 1.0]));
            return Ok((sources.into_iter().next().unwrap(), 1, None, decode));
        }
        let spot = matches!(spec.space, ColorSpace::Separation { .. } | ColorSpace::DeviceN { .. });
        let simple = !spot && !spec.multi && spec.bpc != 12;
        if simple {
            let space = self.device_space(&spec.space)?;
            return Ok((sources.into_iter().next().unwrap(), spec.bpc, Some(space.pdf()), spec.decode.clone()));
        }
        // Gather samples pixel by pixel.
        let mut rows: Vec<Vec<u32>> = Vec::with_capacity(h);
        for y in 0..h {
            if spec.multi {
                let plane_row = (w * usize::from(spec.bpc)).div_ceil(8);
                let planes: Vec<Vec<u32>> = sources
                    .iter()
                    .map(|p| unpack(&p[y * plane_row..(y + 1) * plane_row], spec.bpc, w))
                    .collect();
                rows.push((0..w).flat_map(|x| planes.iter().map(move |p| p[x])).collect());
            } else {
                let row = &sources[0][y * row_bytes..(y + 1) * row_bytes];
                rows.push(unpack(row, spec.bpc, w * ncomp));
            }
        }
        if spot {
            let hi = 1.0;
            let mut cache: FxMap<Vec<u32>, [u8; 3]> = FxMap::default();
            let mut out = Vec::with_capacity(w * h * 3);
            for row in &rows {
                for px in row.chunks_exact(ncomp) {
                    if let Some(rgb) = cache.get(px) {
                        out.extend_from_slice(rgb);
                        continue;
                    }
                    if cache.len() >= 65_536 {
                        return err("limitcheck", "too many distinct spot colours in image");
                    }
                    let tints: Vec<f64> = px.iter().enumerate().map(|(k, &s)| decode(k, s, &spec.decode, hi)).collect();
                    let space = spec.space.clone();
                    let rgb = dev_rgb(self.resolve_color(&space, &tints, 0)?);
                    cache.insert(px.to_vec(), rgb);
                    out.extend_from_slice(&rgb);
                }
            }
            return Ok((out, 8, Some(Space::Rgb.pdf()), None));
        }
        let space = self.device_space(&spec.space)?;
        let bpc_out = if spec.bpc == 12 { 8 } else { spec.bpc };
        let mut out = Vec::with_capacity(rows.len() * row_bytes);
        for row in &rows {
            if spec.bpc == 12 {
                let narrowed: Vec<u32> = row.iter().map(|s| s >> 4).collect();
                pack(&narrowed, 8, &mut out);
            } else {
                pack(row, bpc_out, &mut out);
            }
        }
        Ok((out, bpc_out, Some(space.pdf()), spec.decode.clone()))
    }
}

fn compress(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::with_capacity(data.len() / 2), flate2::Compression::default());
    enc.write_all(data).expect("writing to a Vec cannot fail");
    enc.finish().expect("writing to a Vec cannot fail")
}

macro_rules! ops {
    ($($name:literal => $f:expr,)*) => {
        &[$(($name, $f as OpFn),)*]
    };
}

pub(crate) static OPERATORS: &[(&str, OpFn)] = ops! {
    "image" => |i: &mut Interp| {
        i.need(1)?;
        if let Value::Dict(d) = i.arg(0).clone() {
            let spec = i.image_spec_from_dict(&d, false)?;
            i.pop_n(1);
            return i.render_image(spec);
        }
        let source = { i.need(5)?; i.arg(0).clone() };
        let matrix = i.matrix_at(1)?;
        let bpc = i.int_at(2)?;
        let height = i.int_at(3)?;
        let width = i.int_at(4)?;
        if width < 0 || height < 0 { return err("rangecheck", "image size"); }
        i.pop_n(5);
        i.render_image(Spec {
            width: width as u32, height: height as u32, bpc: bpc.clamp(0, 16) as u8,
            space: ColorSpace::Gray, decode: None, matrix, sources: vec![source], multi: false, mask: false, interpolate: false,
        })
    },
    "imagemask" => |i: &mut Interp| {
        i.need(1)?;
        if let Value::Dict(d) = i.arg(0).clone() {
            let spec = i.image_spec_from_dict(&d, true)?;
            i.pop_n(1);
            return i.render_image(spec);
        }
        let source = { i.need(5)?; i.arg(0).clone() };
        let matrix = i.matrix_at(1)?;
        let polarity = i.bool_at(2)?;
        let height = i.int_at(3)?;
        let width = i.int_at(4)?;
        if width < 0 || height < 0 { return err("rangecheck", "image size"); }
        i.pop_n(5);
        let decode = if polarity { vec![1.0, 0.0] } else { vec![0.0, 1.0] };
        i.render_image(Spec {
            width: width as u32, height: height as u32, bpc: 1, space: ColorSpace::Gray,
            decode: Some(decode), matrix, sources: vec![source], multi: false, mask: true, interpolate: false,
        })
    },
    "colorimage" => |i: &mut Interp| {
        let ncomp = i.int_at(0)?;
        let multi = i.bool_at(1)?;
        let space = match ncomp {
            1 => ColorSpace::Gray,
            3 => ColorSpace::Rgb,
            4 => ColorSpace::Cmyk,
            _ => return err("rangecheck", "colorimage component count"),
        };
        let nsrc = if multi { ncomp as usize } else { 1 };
        i.need(nsrc + 6)?;
        let sources: Vec<Value> = (0..nsrc).rev().map(|k| i.arg(2 + k).clone()).collect();
        let matrix = i.matrix_at(2 + nsrc)?;
        let bpc = i.int_at(3 + nsrc)?;
        let height = i.int_at(4 + nsrc)?;
        let width = i.int_at(5 + nsrc)?;
        if width < 0 || height < 0 { return err("rangecheck", "image size"); }
        i.pop_n(nsrc + 6);
        i.render_image(Spec {
            width: width as u32, height: height as u32, bpc: bpc.clamp(0, 16) as u8,
            space, decode: None, matrix, sources, multi, mask: false, interpolate: false,
        })
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_round_trip_through_unpack_and_pack() {
        for bpc in [1u8, 2, 4, 8, 16] {
            let max = (1u32 << bpc) - 1;
            let samples: Vec<u32> = (0..13).map(|k| (k * 7) % (max + 1)).collect();
            let mut packed = Vec::new();
            pack(&samples, bpc, &mut packed);
            assert_eq!(unpack(&packed, bpc, samples.len()), samples, "bpc {bpc}");
        }
        // 12-bit samples: 0xABC, 0x123
        assert_eq!(unpack(&[0xAB, 0xC1, 0x23], 12, 2), [0xABC, 0x123]);
    }
}
