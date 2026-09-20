//! SyncTeX file generator for Ratex (PDF to source synchronization).
//!
//! Generates `.synctex.gz` files compatible with TeX Live SyncTeX parser version 1.
//! Maps PDF coordinates (in big points, scaled to SyncTeX units: 1 bp = 65536 / 72.27 pt units)
//! back to source file and line numbers for forward and inverse search in editors
//! (VS Code LaTeX Workshop, TeXstudio, VimTeX, Emacs AUCTeX, etc.).

use flate2::write::GzEncoder;
use flate2::Compression;
use std::collections::HashMap;
use std::io::Write;

/// A record representing a point or box in SyncTeX format.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncRecord {
    pub file_id: u32,
    pub line: u32,
    /// x coordinate in scaled points (sp)
    pub x_sp: i64,
    /// y coordinate in scaled points (sp) from top of page
    pub y_sp: i64,
    /// width in scaled points (sp)
    pub w_sp: i64,
    /// height in scaled points (sp)
    pub h_sp: i64,
}

/// SyncTeX document state collected during engine execution.
#[derive(Clone, Debug, Default)]
pub struct SyncTexData {
    /// File paths interned as 1-based SyncTeX Input IDs.
    pub files: Vec<String>,
    file_map: HashMap<String, u32>,
    /// Records grouped by 1-based page number.
    pub pages: HashMap<u32, Vec<SyncRecord>>,
}

impl SyncTexData {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a source file and get its 1-based SyncTeX File ID.
    pub fn get_or_register_file(&mut self, path: &str) -> u32 {
        if let Some(&id) = self.file_map.get(path) {
            return id;
        }
        let id = (self.files.len() + 1) as u32;
        self.files.push(path.to_string());
        self.file_map.insert(path.to_string(), id);
        id
    }

    /// Record a synchronization point on a given 1-based page.
    pub fn record_point(&mut self, page: u32, file_id: u32, line: u32, x_sp: i64, y_sp: i64) {
        self.pages.entry(page).or_default().push(SyncRecord {
            file_id,
            line,
            x_sp,
            y_sp,
            w_sp: 0,
            h_sp: 0,
        });
    }

    /// Record a box/run with dimensions on a given 1-based page.
    pub fn record_box(
        &mut self,
        page: u32,
        file_id: u32,
        line: u32,
        x_sp: i64,
        y_sp: i64,
        w_sp: i64,
        h_sp: i64,
    ) {
        self.pages.entry(page).or_default().push(SyncRecord {
            file_id,
            line,
            x_sp,
            y_sp,
            w_sp,
            h_sp,
        });
    }

    /// Serialize to standard SyncTeX Version 1 format string.
    pub fn serialize_text(&self) -> String {
        let mut out = String::new();
        out.push_str("SyncTeX Version:1\n");
        for (i, path) in self.files.iter().enumerate() {
            out.push_str(&format!("Input:{}:{}\n", i + 1, path));
        }
        out.push_str("Output:pdf\n");
        out.push_str("Magnification:1000\n");
        out.push_str("Unit:1\n");
        out.push_str("X Offset:0\n");
        out.push_str("Y Offset:0\n");
        out.push_str("Content:\n");

        let mut sorted_pages: Vec<_> = self.pages.keys().copied().collect();
        sorted_pages.sort_unstable();

        let mut total_records = 0;
        for page in sorted_pages {
            let records = &self.pages[&page];
            total_records += records.len();
            out.push_str(&format!("{{{page}\n"));
            for r in records {
                if r.w_sp == 0 && r.h_sp == 0 {
                    // Point record: x<link>,<line>:<x>,<y>
                    out.push_str(&format!(
                        "x{},{}:{},{}\n",
                        r.file_id, r.line, r.x_sp, r.y_sp
                    ));
                } else {
                    // Box record: [link,line:x,y:w,h,depth
                    out.push_str(&format!(
                        "k{},{}:{},{}:{},{},0\n",
                        r.file_id, r.line, r.x_sp, r.y_sp, r.w_sp, r.h_sp
                    ));
                }
            }
            out.push_str(&format!("}}{page}\n"));
        }

        out.push_str("Postamble:\n");
        out.push_str(&format!("Count:{}\n", total_records));
        out.push_str("Post scriptum:\n");
        out
    }

    /// Produce gzip-compressed `.synctex.gz` bytes.
    pub fn to_synctex_gz(&self) -> Result<Vec<u8>, std::io::Error> {
        let text = self.serialize_text();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(text.as_bytes())?;
        encoder.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synctex_roundtrip_format() {
        let mut data = SyncTexData::new();
        let f1 = data.get_or_register_file("main.tex");
        assert_eq!(f1, 1);
        let f2 = data.get_or_register_file("chapter1.tex");
        assert_eq!(f2, 2);
        assert_eq!(data.get_or_register_file("main.tex"), 1);

        data.record_point(1, f1, 10, 65536 * 72, 65536 * 100);
        data.record_box(1, f2, 25, 65536 * 50, 65536 * 200, 65536 * 300, 65536 * 12);

        let text = data.serialize_text();
        assert!(text.contains("SyncTeX Version:1"));
        assert!(text.contains("Input:1:main.tex"));
        assert!(text.contains("Input:2:chapter1.tex"));
        assert!(text.contains("{1"));
        assert!(text.contains(&format!("x1,10:{},{}", 65536 * 72, 65536 * 100)));
        assert!(text.contains(&format!("k2,25:{},{}", 65536 * 50, 65536 * 200)));
        assert!(text.contains("}1"));
        assert!(text.contains("Count:2"));

        let gz = data.to_synctex_gz().expect("compression must succeed");
        assert!(!gz.is_empty());
        assert_eq!(&gz[..2], &[0x1f, 0x8b]); // Gzip magic number
    }
}
