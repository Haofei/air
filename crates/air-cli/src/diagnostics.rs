use air_core::{Diagnostic, Severity};
use miette::{Diagnostic as MietteDiagnostic, Report};
use std::error::Error;
use std::fmt;

pub(crate) fn emit_diagnostics(diagnostics: &[Diagnostic]) {
    for diagnostic in diagnostics {
        eprintln!("{:?}", Report::new(CliDiagnostic::from(diagnostic)));
    }
}

#[derive(Debug)]
struct CliDiagnostic {
    severity: miette::Severity,
    code: &'static str,
    message: String,
    help: Option<String>,
}

impl From<&Diagnostic> for CliDiagnostic {
    fn from(diagnostic: &Diagnostic) -> Self {
        let severity = match diagnostic.severity {
            Severity::Error => miette::Severity::Error,
            Severity::Warning => miette::Severity::Warning,
        };
        Self {
            severity,
            code: diagnostic.code,
            message: diagnostic.message.clone(),
            help: Some("Run `air validate` or `air validate-plan` after editing the AIR file to re-check the IR contract.".to_string()),
        }
    }
}

impl fmt::Display for CliDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for CliDiagnostic {}

impl MietteDiagnostic for CliDiagnostic {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(self.code))
    }

    fn severity(&self) -> Option<miette::Severity> {
        Some(self.severity)
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        self.help
            .as_ref()
            .map(|help| Box::new(help) as Box<dyn fmt::Display>)
    }
}
