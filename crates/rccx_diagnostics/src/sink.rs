//! Diagnostic sink: collects diagnostics produced by compiler stages.

use crate::{Diagnostic, Severity};

/// In-memory collector for diagnostics. Stages push into a sink; the driver
/// drains and renders at well-defined points.
#[derive(Debug, Default)]
pub struct DiagnosticSink {
    diagnostics: Vec<Diagnostic>,
    error_count: u32,
    warning_count: u32,
}

impl DiagnosticSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn emit(&mut self, diagnostic: Diagnostic) {
        match diagnostic.severity {
            Severity::Error => self.error_count += 1,
            Severity::Warning => self.warning_count += 1,
            _ => {}
        }
        self.diagnostics.push(diagnostic);
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn has_errors(&self) -> bool {
        self.error_count > 0
    }

    pub fn error_count(&self) -> u32 {
        self.error_count
    }

    pub fn warning_count(&self) -> u32 {
        self.warning_count
    }

    pub fn drain(&mut self) -> Vec<Diagnostic> {
        self.error_count = 0;
        self.warning_count = 0;
        std::mem::take(&mut self.diagnostics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::E_UNIMPLEMENTED;

    #[test]
    fn counts_track_severity() {
        let mut sink = DiagnosticSink::new();
        sink.emit(Diagnostic::error(E_UNIMPLEMENTED, "boom"));
        sink.emit(Diagnostic::warning(E_UNIMPLEMENTED, "careful"));
        assert!(sink.has_errors());
        assert_eq!(sink.error_count(), 1);
        assert_eq!(sink.warning_count(), 1);
    }

    #[test]
    fn drain_resets_counts() {
        let mut sink = DiagnosticSink::new();
        sink.emit(Diagnostic::error(E_UNIMPLEMENTED, "x"));
        let drained = sink.drain();
        assert_eq!(drained.len(), 1);
        assert!(!sink.has_errors());
    }
}
