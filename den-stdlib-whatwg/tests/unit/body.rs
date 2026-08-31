use super::{BLOCKED_PORTS, is_blocked_port, is_valid_method, null_body_status, utf8_text};

#[test]
fn null_body_statuses_are_the_fetch_empty_status_set() {
    assert!(null_body_status(204));
    assert!(null_body_status(205));
    assert!(null_body_status(304));
    assert!(!null_body_status(200));
    assert!(!null_body_status(404));
}

#[test]
fn http_methods_follow_the_fetch_token_production() {
    assert!(is_valid_method("GET"));
    assert!(is_valid_method("PATCH"));
    assert!(is_valid_method("X-Custom+1"));
    assert!(!is_valid_method(""));
    assert!(!is_valid_method("GET /"));
    assert!(!is_valid_method("GET\n"));
    assert!(!is_valid_method("GÉT"));
}

#[test]
fn blocked_ports_include_the_fetch_bad_port_list() {
    assert!(is_blocked_port(22));
    assert!(is_blocked_port(25));
    assert!(is_blocked_port(6667));
    assert!(!is_blocked_port(80));
    assert!(!is_blocked_port(443));
    assert!(BLOCKED_PORTS.contains(&10080));
}

#[test]
fn utf8_text_strips_a_leading_bom() {
    assert_eq!(utf8_text(b"hello"), "hello");
    assert_eq!(utf8_text("\u{FEFF}hello".as_bytes()), "hello");
    assert_eq!(utf8_text(&[0xff, 0xfe, b'x']), "\u{FFFD}\u{FFFD}x");
}
