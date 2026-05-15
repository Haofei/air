use air_core::{Diagnostic, Severity};

pub(crate) fn emit_diagnostics(diagnostics: &[Diagnostic]) {
    for diagnostic in diagnostics {
        emit_diagnostic(diagnostic);
    }
}

fn emit_diagnostic(diagnostic: &Diagnostic) {
    eprintln!(
        "{}[{}]: {}",
        severity_label(diagnostic),
        diagnostic.code,
        diagnostic.message
    );
    eprintln!(
        "  help: Run `air validate` or `air validate-plan` after editing the AIR file to re-check the IR contract."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_severity_labels_are_stable() {
        let error = Diagnostic::error("AIR001", "bad module");
        let warning = Diagnostic::warning("AIR002", "risky module");

        assert_eq!(severity_label(&error), "error");
        assert_eq!(severity_label(&warning), "warning");
    }
}

fn severity_label(diagnostic: &Diagnostic) -> &'static str {
    match diagnostic.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}
