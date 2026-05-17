pub fn report_status(score: u8) -> &'static str {
    if score >= 90 {
        "excellent"
    } else if score >= 70 {
        "good"
    } else if score >= 50 {
        "watch"
    } else {
        "poor"
    }
}
