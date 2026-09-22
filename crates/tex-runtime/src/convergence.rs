use std::collections::BTreeMap;

pub const DEFAULT_MAX_PASSES: u32 = 5;

/// Analysis of auxiliary files and rerun signals to determine whether another pass is needed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConvergenceState {
    pub pass: u32,
    pub auxiliary_observations: BTreeMap<String, u64>,
    pub bibliography_required: bool,
    pub rerun_requested: bool,
    pub has_unresolved_references: bool,
}

impl ConvergenceState {
    /// Detects rerun signals from engine log and diagnostics.
    pub fn inspect_log_for_rerun(log: &str) -> bool {
        let lower = log.to_ascii_lowercase();
        lower.contains("rerun to get cross-references right")
            || lower.contains("rerun LaTeX")
            || lower.contains("rerun to get outlines right")
            || lower.contains("rerun to get bibliographies right")
            || lower.contains("please rerun")
    }

    /// Check if aux content contains only certified inert records.
    /// Certified records: \relax, \newlabel, \citation, \bibdata, \bibstyle, \@writefile, \gdef
    pub fn is_aux_content_certified_inert(aux_content: &str) -> bool {
        for line in aux_content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('%') {
                continue;
            }
            if !trimmed.starts_with('\\') {
                return false;
            }
            let cmd = trimmed
                .split(['{', ' ', '['])
                .next()
                .unwrap_or("")
                .trim_start_matches('\\');
            match cmd {
                "relax" | "newlabel" | "citation" | "bibdata" | "bibstyle" | "@writefile"
                | "gdef" | "providecolor" | "abspage@last" => {}
                _ => return false,
            }
        }
        true
    }

    /// Check whether a single pass is sound for a fresh document (e.g., standard resume/article).
    pub fn is_sound_single_pass(
        aux_content: &str,
        log: &str,
        has_citations: bool,
        has_bibdata: bool,
    ) -> bool {
        if has_citations || has_bibdata {
            return false;
        }
        if Self::inspect_log_for_rerun(log) {
            return false;
        }
        if log.contains("LaTeX Warning: Reference `")
            || log.contains("LaTeX Warning: Citation `")
            || log.contains("undefined references")
        {
            return false;
        }
        Self::is_aux_content_certified_inert(aux_content)
    }
}
