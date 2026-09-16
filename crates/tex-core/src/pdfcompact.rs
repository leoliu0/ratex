//! Emit PDF 1.5 object streams and a cross-reference stream directly.
use std::io::Write;

pub(crate) fn serialize(
    objects: &[Option<Vec<u8>>],
    packable: &[bool],
    catalog: usize,
    info: usize,
) -> Vec<u8> {
    let packed: Vec<_> = objects
        .iter()
        .enumerate()
        .filter(|(i, object)| object.is_some() && packable.get(*i).copied().unwrap_or(false))
        .map(|(i, _)| i + 1)
        .collect();
    let streams = packed.len().div_ceil(100);
    let xref_id = objects.len() + streams + 1;
    // type, byte offset or object-stream number, generation or stream index
    let mut xref = vec![(0u8, 0u64, 65535u16); xref_id + 1];
    let mut out = b"%PDF-1.5\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let write_object =
        |out: &mut Vec<u8>, xref: &mut Vec<(u8, u64, u16)>, id: usize, body: &[u8]| {
            xref[id] = (1, out.len() as u64, 0);
            writeln!(out, "{id} 0 obj").unwrap();
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        };
    for (i, body) in objects.iter().enumerate() {
        if !packable.get(i).copied().unwrap_or(false) {
            if let Some(body) = body {
                write_object(&mut out, &mut xref, i + 1, body);
            }
        }
    }
    for (stream, chunk) in packed.chunks(100).enumerate() {
        let id = objects.len() + stream + 1;
        let mut header = Vec::new();
        let mut payload = Vec::new();
        for (index, &object) in chunk.iter().enumerate() {
            write!(header, "{object} {} ", payload.len()).unwrap();
            payload.extend_from_slice(objects[object - 1].as_ref().unwrap());
            // End a possible trailing PDF comment before the next object.
            payload.push(b'\n');
            xref[object] = (2, id as u64, index as u16);
        }
        let first = header.len();
        header.extend_from_slice(&payload);
        let compressed = crate::pdffile::flate(&header);
        let mut body = format!(
            "<< /Type /ObjStm /N {} /First {first} /Length {} /Filter /FlateDecode >>\nstream\n",
            chunk.len(),
            compressed.len()
        )
        .into_bytes();
        body.extend_from_slice(&compressed);
        body.extend_from_slice(b"\nendstream");
        write_object(&mut out, &mut xref, id, &body);
    }
    let startxref = out.len();
    xref[xref_id] = (1, startxref as u64, 0);
    let mut entries = Vec::with_capacity(xref.len() * 11);
    for (kind, offset, generation) in xref {
        entries.push(kind);
        entries.extend_from_slice(&offset.to_be_bytes());
        entries.extend_from_slice(&generation.to_be_bytes());
    }
    let compressed = crate::pdffile::flate(&entries);
    writeln!(out, "{xref_id} 0 obj\n<< /Type /XRef /Size {} /W [1 8 2] /Root {catalog} 0 R /Info {info} 0 R /Length {} /Filter /FlateDecode >>\nstream", xref_id + 1, compressed.len()).unwrap();
    out.extend_from_slice(&compressed);
    write!(out, "\nendstream\nendobj\nstartxref\n{startxref}\n%%EOF\n").unwrap();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_objects_keep_sparse_ids_streams_and_comments() {
        let mut objects = vec![None; 207];
        objects[0] = Some(b"<< /Type /Catalog /Pages 3 0 R >>".to_vec());
        objects[2] = Some(b"<< /Type /Pages /Count 0 /Kids [] >>".to_vec());
        objects[4] = Some(b"<< /Producer (test) >> % trailing comment".to_vec());
        for (i, slot) in objects.iter_mut().enumerate().skip(6).take(200) {
            *slot = Some(format!("({i})").into_bytes());
        }
        objects[206] = Some(b"<< /Length 3 >>\nstream\nabc\nendstream".to_vec());
        let mut packable = vec![true; objects.len()];
        packable[206] = false;
        let bytes = serialize(&objects, &packable, 1, 5);
        let doc = lopdf::Document::load_mem(&bytes).unwrap();
        assert_eq!(
            doc.catalog()
                .unwrap()
                .get(b"Pages")
                .unwrap()
                .as_reference()
                .unwrap(),
            (3, 0)
        );
        assert_eq!(
            doc.get_object((207, 0))
                .unwrap()
                .as_stream()
                .unwrap()
                .content,
            b"abc"
        );
        for i in 6..206 {
            assert_eq!(
                doc.get_object((i + 1, 0)).unwrap().as_str().unwrap(),
                i.to_string().as_bytes()
            );
        }
        assert!(doc.get_object((2, 0)).is_err());
    }
}

/// Preserve normalization of caller-supplied PDF objects and attributes.
pub(crate) fn serialize_compatible(
    objects: &[Option<Vec<u8>>],
    catalog: usize,
    info: usize,
) -> Vec<u8> {
    // ---- assemble file
    let mut buf = b"%PDF-1.5\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = vec![0usize; objects.len() + 1];
    for (i, obj) in objects.iter().enumerate() {
        let Some(bytes) = obj else { continue };
        offsets[i + 1] = buf.len();
        buf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        buf.extend_from_slice(bytes);
        buf.extend_from_slice(b"\nendobj\n");
    }
    let xref_pos = buf.len();
    let n = objects.len();
    buf.extend_from_slice(format!("xref\n0 {}\n", n + 1).as_bytes());
    buf.extend_from_slice(b"0000000000 65535 f \n");
    for &off in &offsets[1..] {
        if off == 0 {
            buf.extend_from_slice(b"0000000000 65535 f \n");
        } else {
            buf.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
        }
    }
    buf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root {} 0 R /Info {} 0 R >>\nstartxref\n{}\n%%EOF\n",
            n + 1,
            catalog,
            info,
            xref_pos
        )
        .as_bytes(),
    );
    buf
}
