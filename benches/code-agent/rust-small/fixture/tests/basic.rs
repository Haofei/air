use air_bench_rust_small::collections::{active_names, Item};
use air_bench_rust_small::config::env_key;
use air_bench_rust_small::errors::error_message;
use air_bench_rust_small::features::feature_enabled;
use air_bench_rust_small::http::status_class;
use air_bench_rust_small::math::add_then_double;
use air_bench_rust_small::pricing::apply_discount;
use air_bench_rust_small::report::report_status;
use air_bench_rust_small::schedule::normalized_window;
use air_bench_rust_small::strings::normalize_words;

#[test]
fn math_and_pricing_behave() {
    assert_eq!(add_then_double(2, 3), 10);
    assert_eq!(apply_discount(10_000, 25), 7_500);
    assert_eq!(apply_discount(10_000, 250), 0);
}

#[test]
fn strings_and_config_behave() {
    assert_eq!(
        normalize_words(" Alpha, beta ,,Gamma "),
        vec!["alpha", "beta", "gamma"]
    );
    assert_eq!(env_key("local-api key"), "LOCAL_API_KEY");
}

#[test]
fn report_errors_features_and_http_behave() {
    assert_eq!(report_status(95), "excellent");
    assert_eq!(report_status(72), "good");
    assert_eq!(report_status(55), "watch");
    assert_eq!(report_status(20), "poor");
    assert_eq!(error_message(503, "down"), "server: down");
    assert!(feature_enabled("team", "audit"));
    assert!(!feature_enabled("free", "audit"));
    assert_eq!(status_class(204), "success");
    assert_eq!(status_class(404), "client_error");
}

#[test]
fn collections_and_schedule_behave() {
    let items = vec![
        Item {
            name: "alpha".to_string(),
            archived: false,
            score: 2,
        },
        Item {
            name: "beta".to_string(),
            archived: true,
            score: 4,
        },
        Item {
            name: "gamma".to_string(),
            archived: false,
            score: 0,
        },
    ];
    assert_eq!(active_names(&items), vec!["alpha"]);
    assert_eq!(normalized_window(9, 3), (3, 9));
}
