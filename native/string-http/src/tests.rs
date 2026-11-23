use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    thread,
};
use ureq::tls::RootCerts;

#[test]
fn verifies_certificates_with_windows() {
    let client = Client::new(ProxySettings::default());
    assert!(matches!(
        client.agent.config().tls_config().root_certs(),
        RootCerts::PlatformVerifier
    ));
}

#[test]
fn windows_proxy_forms_and_exceptions() {
    let server = " http=proxy.test:8080 ; HTTPS=secure.test:3128 ; ftp=unused.test ";
    let bypass = "<local>;*.internal.test;special.test:443;http://only.test;10.*";
    let settings = ProxySettings::parse(true, server, bypass);
    let route = |address: &str| {
        settings
            .route(&Url::parse(address).unwrap())
            .unwrap()
            .map(|p| p.uri().authority().unwrap().as_str())
    };
    assert_eq!(route("http://example.test"), Some("proxy.test:8080"));
    assert_eq!(route("https://example.test"), Some("secure.test:3128"));
    for address in [
        "http://printer",
        "https://a.internal.test",
        "http://10.2.0.3",
        "https://special.test",
        "http://only.test",
        "https://127.0.0.2",
        "http://[::1]",
    ] {
        assert_eq!(route(address), None, "{address}");
    }
    assert!(route("https://special.test:4431").is_some());
    assert!(route("https://only.test").is_some());
    assert!(route("https://a.internal.test.example").is_some());
    assert!(route("http://[2001:db8::1]").is_some());
    for scheme in ["http", "https"] {
        let url = Url::parse(&format!("{scheme}://example.test")).unwrap();
        assert!(
            ProxySettings::parse(true, "proxy.test:8080", "")
                .route(&url)
                .unwrap()
                .is_some()
        );
        assert!(
            ProxySettings::parse(false, server, "")
                .route(&url)
                .unwrap()
                .is_none()
        );
    }
    assert!(
        ProxySettings::parse(true, "http=plain.test:80", "")
            .route(&Url::parse("https://example.test").unwrap())
            .unwrap()
            .is_none()
    );
}

#[test]
fn unsupported_socks_routes_fail_before_a_request_is_built() {
    for setting in ["socks=unused.test:1080", "https=socks5://unused.test:1080"] {
        let client = Client::new(ProxySettings::parse(true, setting, ""));
        let result = client.get("https://service.invalid", Duration::from_secs(1), 0, |_| {
            panic!("unsupported route reached request construction")
        });
        assert!(result.unwrap_err().to_string().contains("SOCKS"));
    }
}

#[test]
fn explicit_http_proxy_and_windows_exceptions_precede_socks_fallback() {
    let settings = ProxySettings::parse(
        true,
        "socks=socks.test:1080;http=http.test:80",
        "*.internal.test",
    );
    let route = |url| settings.route(&Url::parse(url).unwrap());
    assert!(route("http://example.test").unwrap().is_some());
    assert!(route("https://example.test").is_err());
    assert!(route("https://service.internal.test").unwrap().is_none());
    assert!(route("https://localhost").unwrap().is_none());
}

fn headers(stream: &mut BufReader<std::net::TcpStream>) -> String {
    let mut line = String::new();
    loop {
        let n = stream.read_line(&mut line).unwrap();
        assert!(n > 0);
        if line.ends_with("\r\n\r\n") {
            return line;
        }
    }
}

#[test]
fn injected_proxy_carries_request_and_redirect_bypasses_it() {
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = TcpListener::bind("127.0.0.1:0").unwrap();
    let redirect = origin.local_addr().unwrap();
    let client = Client::new(ProxySettings::parse(
        true,
        &proxy.local_addr().unwrap().to_string(),
        "",
    ));
    assert!(
        client
            .proxy
            .route(&Url::parse("http://music.invalid").unwrap())
            .unwrap()
            .is_some()
    );
    let worker = thread::spawn(move || {
        let mut conn = BufReader::new(proxy.accept().unwrap().0);
        let connect = headers(&mut conn);
        assert!(connect.starts_with("CONNECT music.invalid:80 HTTP/1.1"));
        assert!(connect.contains("String/1.0"));
        conn.get_mut()
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .unwrap();
        let req = headers(&mut conn);
        assert!(req.starts_with("GET /song HTTP/1.1"));
        assert!(req.contains("String/1.0"));
        write!(conn.get_mut(), "HTTP/1.1 302 Found\r\nLocation: http://{redirect}/cover\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        let mut direct = BufReader::new(origin.accept().unwrap().0);
        assert!(headers(&mut direct).starts_with("GET /cover HTTP/1.1"));
        direct
            .get_mut()
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nokay")
            .unwrap();
    });
    let mut res = client
        .get(
            "http://music.invalid/song",
            Duration::from_secs(3),
            10,
            |r| r.config().user_agent("String/1.0").build(),
        )
        .unwrap();
    assert_eq!(res.body_mut().read_to_string().unwrap(), "okay");
    worker.join().unwrap();
}
