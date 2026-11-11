//! HTTPS clients, and connection errors that say why they failed.
//!
//! Every client trusts the WebPKI roots bundled with the program and the
//! operating system's certificate authorities: the Windows certificate store,
//! the macOS keychains, or the system bundle on Linux and other Unix systems.
//! When `SSL_CERT_FILE` or `SSL_CERT_DIR` is set, the certificates there are
//! read instead of the system's. A certificate authority of a company network
//! that curl and browsers trust is therefore trusted here too.

/// A client from `configure`, which sets everything except the roots. When
/// the system's roots cannot be used (the store holds no certificate that
/// parses), the client is built again with the bundled roots alone, so it
/// still reaches servers with public certificates.
///
/// Redirects are not followed unless `configure` asks: provider keys travel
/// in custom headers (`x-api-key`, `api-key`) that reqwest keeps when a
/// redirect leaves the origin, and a redirect may also downgrade to http.
pub fn client(
    configure: impl Fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder,
) -> reqwest::Result<reqwest::Client> {
    let builder = || reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
    configure(builder())
        .build()
        .or_else(|_| configure(builder().tls_built_in_native_certs(false)).build())
}

/// [`client`] for the blocking API.
pub fn blocking_client(
    configure: impl Fn(reqwest::blocking::ClientBuilder) -> reqwest::blocking::ClientBuilder,
) -> reqwest::Result<reqwest::blocking::Client> {
    let builder =
        || reqwest::blocking::Client::builder().redirect(reqwest::redirect::Policy::none());
    configure(builder())
        .build()
        .or_else(|_| configure(builder().tls_built_in_native_certs(false)).build())
}

/// What to do when a server's certificate is refused.
pub const CERTIFICATE_HINT: &str = "If a private certificate authority issued the server's certificate, install that CA in the system trust store, or set SSL_CERT_FILE to a PEM file that holds it.";

/// The causes under a failed request, joined by ": ", for example
/// `client error (Connect): invalid peer certificate: UnknownIssuer`; empty
/// when there is none. reqwest's own message, which names the URL, is left
/// out.
pub fn causes(error: &reqwest::Error) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut next = std::error::Error::source(error);
    while let Some(cause) = next {
        let text = cause.to_string();
        if !text.is_empty() && parts.last() != Some(&text) {
            parts.push(text);
        }
        next = cause.source();
    }
    parts.join(": ")
}

/// Whether an error's text says that the server's certificate was refused.
pub fn certificate_refused(text: &str) -> bool {
    text.contains("invalid peer certificate") || text.contains("UnknownIssuer")
}

/// `text`, followed by [`CERTIFICATE_HINT`] when the server's certificate
/// was refused.
pub fn with_hint(text: String) -> String {
    if certificate_refused(&text) {
        format!("{}. {CERTIFICATE_HINT}", text.trim_end_matches('.'))
    } else {
        text
    }
}

/// A failed request for people: reqwest's message, whose URL keeps no
/// credentials, query or fragment, then the [`causes`], with
/// [`CERTIFICATE_HINT`] when the server's certificate was refused.
pub fn describe(mut error: reqwest::Error) -> String {
    if let Some(url) = error.url_mut() {
        let _ = url.set_username("");
        let _ = url.set_password(None);
        url.set_query(None);
        url.set_fragment(None);
    }
    let mut text = error.to_string();
    let causes = causes(&error);
    if !causes.is_empty() {
        text = format!("{text}: {causes}");
    }
    with_hint(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hint_follows_a_refused_certificate_only() {
        let refused = with_hint(
            "error sending request: client error (Connect): invalid peer certificate: UnknownIssuer"
                .into(),
        );
        assert!(refused.ends_with(&format!("UnknownIssuer. {CERTIFICATE_HINT}")));
        let refused_connection = "error sending request: client error (Connect): tcp connect error: Connection refused (os error 111)";
        assert_eq!(with_hint(refused_connection.into()), refused_connection);
    }

    #[test]
    fn a_client_builds_with_the_system_roots_and_the_bundled_roots() {
        assert!(client(|builder| builder).is_ok());
        assert!(blocking_client(|builder| builder).is_ok());
    }
}

#[cfg(test)]
mod redirect_tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Provider keys travel in custom headers (`x-api-key`, `api-key`) that
    /// reqwest keeps on a cross-origin redirect, so no client follows one.
    #[test]
    fn clients_never_follow_redirects() {
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let origin = TcpListener::bind("127.0.0.1:0").unwrap();
        let location = format!("http://{}/", target.local_addr().unwrap());
        let url = format!("http://{}/", origin.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = origin.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let reply = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(reply.as_bytes()).unwrap();
        });
        let client =
            super::blocking_client(|builder| builder.timeout(std::time::Duration::from_secs(2)))
                .unwrap();
        // A followed redirect reaches `target`, which never answers.
        let response = client
            .post(&url)
            .header("x-api-key", "secret")
            .body("{}")
            .send();
        server.join().unwrap();
        assert!(
            target.accept().is_err(),
            "the redirect target was contacted"
        );
        assert_eq!(response.unwrap().status().as_u16(), 307);
    }
}
