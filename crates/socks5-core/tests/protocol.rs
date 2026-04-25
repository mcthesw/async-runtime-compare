use socks5_core::*;

#[test]
fn select_no_auth_accepts_no_auth_method() {
    assert_eq!(select_no_auth(&[0x05, 0x01, 0x00]), Ok(()));
}

#[test]
fn select_no_auth_accepts_no_auth_among_multiple_methods() {
    assert_eq!(select_no_auth(&[0x05, 0x02, 0x02, 0x00]), Ok(()));
}

#[test]
fn select_no_auth_rejects_missing_no_auth_method() {
    assert_eq!(
        select_no_auth(&[0x05, 0x01, 0x02]),
        Err(SocksError::NoAcceptableAuthMethod)
    );
}

#[test]
fn select_no_auth_rejects_invalid_version() {
    assert_eq!(
        select_no_auth(&[0x04, 0x01, 0x00]),
        Err(SocksError::InvalidVersion(0x04))
    );
}

#[test]
fn select_no_auth_rejects_short_greeting() {
    assert_eq!(
        select_no_auth(&[0x05, 0x02, 0x00]),
        Err(SocksError::ShortBuffer {
            needed: 4,
            actual: 3
        })
    );
}

#[test]
fn select_no_auth_rejects_trailing_bytes() {
    assert_eq!(
        select_no_auth(&[0x05, 0x01, 0x00, 0xff]),
        Err(SocksError::TrailingBytes)
    );
}

#[test]
fn parse_connect_request_accepts_ipv4_target() {
    let req = parse_connect_request(&[0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1, 0x1f, 0x90])
        .expect("valid IPv4 request");

    assert_eq!(
        req,
        ConnectRequest {
            target: TargetAddr::Ipv4([127, 0, 0, 1]),
            port: 8080
        }
    );
}

#[test]
fn parse_connect_request_accepts_domain_target() {
    let req = parse_connect_request(&[
        0x05, 0x01, 0x00, 0x03, 11, b'e', b'x', b'a', b'm', b'p', b'l', b'e', b'.', b'c', b'o',
        b'm', 0x01, 0xbb,
    ])
    .expect("valid domain request");

    assert_eq!(
        req,
        ConnectRequest {
            target: TargetAddr::Domain(String::from("example.com")),
            port: 443
        }
    );
}

#[test]
fn parse_connect_request_accepts_ipv6_target() {
    let req = parse_connect_request(&[
        0x05, 0x01, 0x00, 0x04, 0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
        0x13, 0x88,
    ])
    .expect("valid IPv6 request");

    assert_eq!(
        req,
        ConnectRequest {
            target: TargetAddr::Ipv6([0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
            port: 5000
        }
    );
}

#[test]
fn parse_connect_request_rejects_non_connect_command() {
    assert_eq!(
        parse_connect_request(&[0x05, 0x02, 0x00, 0x01, 127, 0, 0, 1, 0, 80]),
        Err(SocksError::UnsupportedCommand(0x02))
    );
}

#[test]
fn parse_connect_request_rejects_invalid_reserved_byte() {
    assert_eq!(
        parse_connect_request(&[0x05, 0x01, 0xff, 0x01, 127, 0, 0, 1, 0, 80]),
        Err(SocksError::InvalidReserved(0xff))
    );
}

#[test]
fn parse_connect_request_rejects_unsupported_address_type() {
    assert_eq!(
        parse_connect_request(&[0x05, 0x01, 0x00, 0xff, 127, 0, 0, 1, 0, 80]),
        Err(SocksError::UnsupportedAddressType(0xff))
    );
}

#[test]
fn parse_connect_request_rejects_short_ipv4_request() {
    assert_eq!(
        parse_connect_request(&[0x05, 0x01, 0x00, 0x01, 127]),
        Err(SocksError::ShortBuffer {
            needed: 10,
            actual: 5
        })
    );
}

#[test]
fn parse_connect_request_rejects_trailing_bytes() {
    assert_eq!(
        parse_connect_request(&[0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1, 0, 80, 0xff]),
        Err(SocksError::TrailingBytes)
    );
}

#[test]
fn parse_connect_request_rejects_empty_domain() {
    assert_eq!(
        parse_connect_request(&[0x05, 0x01, 0x00, 0x03, 0, 0, 80]),
        Err(SocksError::EmptyDomain)
    );
}

#[test]
fn parse_connect_request_rejects_invalid_domain_utf8() {
    assert_eq!(
        parse_connect_request(&[0x05, 0x01, 0x00, 0x03, 1, 0xff, 0, 80]),
        Err(SocksError::InvalidDomain)
    );
}

#[test]
fn encode_method_selection_no_auth_returns_no_auth_reply() {
    assert_eq!(encode_method_selection_no_auth(), [0x05, 0x00]);
}

#[test]
fn encode_success_reply_returns_zero_bound_address() {
    assert_eq!(
        encode_success_reply(),
        [0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]
    );
}

#[test]
fn encode_general_failure_reply_returns_zero_bound_address() {
    assert_eq!(
        encode_general_failure_reply(),
        [0x05, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0]
    );
}
