pub fn feature_enabled(tier: &str, feature: &str) -> bool {
    match tier {
        "enterprise" => true,
        "team" => matches!(feature, "export" | "audit"),
        "free" => feature == "export",
        _ => false,
    }
}
