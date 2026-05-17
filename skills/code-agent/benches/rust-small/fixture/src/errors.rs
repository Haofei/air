pub fn error_message(code: u16, detail: &str) -> String {
    let prefix = if code >= 500 {
        "server"
    } else if code >= 400 {
        "client"
    } else {
        "unknown"
    };
    format!("{prefix}: {detail}")
}
