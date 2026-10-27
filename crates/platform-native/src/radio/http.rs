//! The smallest HTTP client a radio stream needs: one `GET` over TCP or TLS (rustls with
//! the system's trust store), redirects followed, the response head parsed, the body read
//! as it comes. It asks in HTTP/1.0, so servers never send a chunked body, and accepts
//! SHOUTcast's `ICY 200 OK` status line. An `https://` request goes through the proxy in
//! `HTTPS_PROXY` (an `http://host:port` proxy, by `CONNECT`) unless `NO_PROXY` names the
//! host.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{mpsc, Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

/// How long one connection attempt may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// A blocked read returns this often, so the stream thread can notice it is no longer
/// wanted.
pub const READ_TIMEOUT: Duration = Duration::from_millis(500);
/// Redirects followed before giving up.
const MAX_REDIRECTS: usize = 5;
/// Longest response head, bytes.
const MAX_HEAD: usize = 16 * 1024;

/// An `http://` or `https://` URL, split.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub tls: bool,
    pub host: String,
    pub port: u16,
    /// From the first `/`, with the query.
    pub path: String,
}

impl Url {
    pub fn parse(s: &str) -> Result<Url, String> {
        let (tls, rest) = if let Some(r) = s.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = s.strip_prefix("http://") {
            (false, r)
        } else {
            return Err(format!("not an http(s) URL: {s}"));
        };
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, path) = rest.split_at(end);
        let path = path.split('#').next().unwrap_or("");
        let path = match path {
            "" => "/".to_string(),
            p if p.starts_with('?') => format!("/{p}"),
            p => p.to_string(),
        };
        if authority.contains('@') {
            return Err(format!(
                "credentials in a stream URL are not supported: {s}"
            ));
        }
        let (host, port) = split_host_port(authority, if tls { 443 } else { 80 })?;
        Ok(Url {
            tls,
            host,
            port,
            path,
        })
    }

    /// `location` (absolute, or a path on this server) seen from this URL.
    pub fn join(&self, location: &str) -> Result<Url, String> {
        if location.starts_with("http://") || location.starts_with("https://") {
            return Url::parse(location);
        }
        if !location.starts_with('/') {
            return Err(format!("unsupported redirect: {location}"));
        }
        Ok(Url {
            path: location.to_string(),
            ..self.clone()
        })
    }
}

impl Url {
    /// Where a redirect to `location` leads; never from https down to http.
    pub fn redirect(&self, location: &str) -> Result<Url, String> {
        let next = self.join(location)?;
        if self.tls && !next.tls {
            return Err(format!("refusing a redirect from https to {location}"));
        }
        Ok(next)
    }
}

/// `host[:port]` (an IPv6 host in brackets).
fn split_host_port(s: &str, default: u16) -> Result<(String, u16), String> {
    let (host, port) = match s.rfind(':') {
        Some(i) if !s[i..].contains(']') => (&s[..i], Some(&s[i + 1..])),
        _ => (s, None),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return Err(format!("no host in `{s}`"));
    }
    let port = match port {
        Some(p) => p.parse().map_err(|_| format!("bad port in `{s}`"))?,
        None => default,
    };
    Ok((host.to_ascii_lowercase(), port))
}

/// The `GET` for `url`.
pub fn request(url: &Url) -> String {
    let default = if url.tls { 443 } else { 80 };
    let host = if url.port == default {
        url.host.clone()
    } else {
        format!("{}:{}", url.host, url.port)
    };
    format!(
        "GET {} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: Skywave/{}\r\nAccept: */*\r\n\
         Icy-MetaData: 1\r\nConnection: close\r\n\r\n",
        url.path,
        env!("CARGO_PKG_VERSION"),
    )
}

/// A response's status line and headers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    pub status: u16,
    /// Names lowercased, values trimmed, in order.
    pub headers: Vec<(String, String)>,
}

impl Head {
    /// The first header called `name` (lowercase).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Where the head in `buf` ends (past its blank line), once it has.
pub fn head_end(buf: &[u8]) -> Option<usize> {
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|i| i + 2);
    match (crlf, lf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Parses a response head (`HTTP/1.x 200 OK` or `ICY 200 OK`, then headers).
pub fn parse_head(bytes: &[u8]) -> Result<Head, String> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines();
    let status_line = lines.next().unwrap_or("");
    let mut parts = status_line.split_whitespace();
    let proto = parts.next().unwrap_or("");
    if !(proto.starts_with("HTTP/") || proto == "ICY") {
        return Err(format!("not an HTTP response: `{status_line}`"));
    }
    let status = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("no status in `{status_line}`"))?;
    let headers = lines
        .take_while(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let (n, v) = l.split_once(':')?;
            Some((n.trim().to_ascii_lowercase(), v.trim().to_string()))
        })
        .collect();
    Ok(Head { status, headers })
}

/// A connection to a stream server.
pub enum Conn {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Conn::Plain(s) => s.read(buf),
            Conn::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Conn::Plain(s) => s.write(buf),
            Conn::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Conn::Plain(s) => s.flush(),
            Conn::Tls(s) => s.flush(),
        }
    }
}

/// A read that timed out (the socket's read timeout), not a failure.
pub fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}

/// `GET`s `url`, following redirects, until a `200` arrives; returns the connection, the
/// head and the body bytes read with it. Gives up at `deadline`, or when `wanted` says the
/// stream is no longer wanted.
pub fn open(
    url: &str,
    deadline: Instant,
    wanted: &impl Fn() -> bool,
) -> Result<(Conn, Head, Vec<u8>), String> {
    let mut url = Url::parse(url)?;
    for _ in 0..=MAX_REDIRECTS {
        let mut conn = connect(&url, deadline, wanted)?;
        conn.write_all(request(&url).as_bytes())
            .map_err(|e| format!("sending the request: {e}"))?;
        let (head, body) = read_head(&mut conn, deadline, wanted)?;
        match head.status {
            200 => return Ok((conn, head, body)),
            301 | 302 | 303 | 307 | 308 => {
                let to = head
                    .header("location")
                    .ok_or("a redirect without Location")?;
                url = url.redirect(to)?;
            }
            s => return Err(format!("HTTP {s}")),
        }
    }
    Err("too many redirects".into())
}

/// Reads the response head; returns it and the body bytes that came with it.
fn read_head(
    conn: &mut impl Read,
    deadline: Instant,
    wanted: &impl Fn() -> bool,
) -> Result<(Head, Vec<u8>), String> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = head_end(&buf) {
            let head = parse_head(&buf[..end])?;
            return Ok((head, buf.split_off(end)));
        }
        if buf.len() > MAX_HEAD {
            return Err("response head too long".into());
        }
        if !wanted() {
            return Err("no longer wanted".into());
        }
        if Instant::now() >= deadline {
            return Err("timed out waiting for the server".into());
        }
        match conn.read(&mut chunk) {
            Ok(0) => return Err("the server closed the connection".into()),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if is_timeout(&e) => {}
            Err(e) => return Err(format!("reading the response: {e}")),
        }
    }
}

fn connect(url: &Url, deadline: Instant, wanted: &impl Fn() -> bool) -> Result<Conn, String> {
    let proxy = if url.tls { proxy_for(&url.host) } else { None };
    let (host, port) = proxy
        .as_ref()
        .map_or((url.host.as_str(), url.port), |(h, p)| (h.as_str(), *p));
    let tcp = tcp(host, port)?;
    if !wanted() {
        return Err("no longer wanted".into());
    }
    if proxy.is_some() {
        tunnel(&tcp, url)?;
    }
    if !url.tls {
        return Ok(Conn::Plain(tcp));
    }
    let name = ServerName::try_from(url.host.clone()).map_err(|e| format!("{e}"))?;
    let tls = ClientConnection::new(tls_config()?, name).map_err(|e| format!("TLS: {e}"))?;
    let mut stream = StreamOwned::new(tls, tcp);
    handshake(&mut stream, deadline, wanted)?;
    Ok(Conn::Tls(Box::new(stream)))
}

/// Completes the TLS handshake. The socket's short read timeout only lets the thread check
/// whether it is still wanted; a slow link keeps trying until `deadline`.
fn handshake(
    s: &mut StreamOwned<ClientConnection, TcpStream>,
    deadline: Instant,
    wanted: &impl Fn() -> bool,
) -> Result<(), String> {
    while s.conn.is_handshaking() {
        if !wanted() {
            return Err("no longer wanted".into());
        }
        if Instant::now() >= deadline {
            return Err("timed out in the TLS handshake".into());
        }
        match s.conn.complete_io(&mut s.sock) {
            Ok(_) => {}
            Err(e) if is_timeout(&e) => {}
            Err(e) => return Err(format!("TLS handshake: {e}")),
        }
    }
    Ok(())
}

fn tcp(host: &str, port: u16) -> Result<TcpStream, String> {
    let addrs = lookup(host, port)?;
    let mut last = format!("{host} has no address");
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => {
                s.set_read_timeout(Some(READ_TIMEOUT))
                    .and_then(|()| s.set_write_timeout(Some(CONNECT_TIMEOUT)))
                    .and_then(|()| s.set_nodelay(true))
                    .map_err(|e| e.to_string())?;
                return Ok(s);
            }
            Err(e) => last = format!("connecting to {host}: {e}"),
        }
    }
    Err(last)
}

/// Resolves `host` on a helper thread, giving up after [`CONNECT_TIMEOUT`] (the system
/// resolver has no timeout of its own; a hung lookup's thread ends when the resolver does).
fn lookup(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    let (tx, rx) = mpsc::channel();
    let name = host.to_string();
    thread::Builder::new()
        .name("radio-dns".into())
        .spawn(move || {
            let found = (name.as_str(), port).to_socket_addrs();
            let _ = tx.send(found.map(Iterator::collect::<Vec<_>>));
        })
        .map_err(|e| e.to_string())?;
    match rx.recv_timeout(CONNECT_TIMEOUT) {
        Ok(Ok(addrs)) => Ok(addrs),
        Ok(Err(e)) => Err(format!("looking up {host}: {e}")),
        Err(_) => Err(format!("looking up {host} timed out")),
    }
}

/// Asks the proxy on `tcp` for a tunnel to `url`'s server.
fn tunnel(mut tcp: &TcpStream, url: &Url) -> Result<(), String> {
    let target = format!("{}:{}", url.host, url.port);
    write!(tcp, "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n")
        .map_err(|e| format!("proxy: {e}"))?;
    // Byte by byte, so nothing past the proxy's head (the TLS handshake) is read here.
    let mut head = Vec::new();
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let mut byte = [0u8; 1];
    while head_end(&head).is_none() {
        if head.len() > MAX_HEAD || Instant::now() >= deadline {
            return Err("proxy: no answer".into());
        }
        match tcp.read(&mut byte) {
            Ok(0) => return Err("proxy: closed".into()),
            Ok(_) => head.push(byte[0]),
            Err(e) if is_timeout(&e) => {}
            Err(e) => return Err(format!("proxy: {e}")),
        }
    }
    match parse_head(&head)?.status {
        200 => Ok(()),
        s => Err(format!("proxy: HTTP {s}")),
    }
}

/// The proxy for an `https://` request to `host`, from the environment.
fn proxy_for(host: &str) -> Option<(String, u16)> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let proxy = var("HTTPS_PROXY").or_else(|| var("https_proxy"))?;
    let no_proxy = var("NO_PROXY")
        .or_else(|| var("no_proxy"))
        .unwrap_or_default();
    if bypasses(host, &no_proxy) {
        return None;
    }
    let p = proxy.strip_prefix("http://").unwrap_or(&proxy);
    let p = p.split('/').next().unwrap_or(p);
    if p.contains('@') {
        log::warn!("radio: proxies with credentials are not supported; connecting directly");
        return None;
    }
    split_host_port(p, 80).ok()
}

/// Whether `NO_PROXY` (comma-separated hosts or domain suffixes, `*` for all) names `host`.
fn bypasses(host: &str, no_proxy: &str) -> bool {
    no_proxy.split(',').map(str::trim).any(|n| {
        let n = n.trim_start_matches("*.").trim_start_matches('.');
        n == "*" || (!n.is_empty() && (host == n || host.ends_with(&format!(".{n}"))))
    })
}

/// The TLS client settings, made once: the ring provider and the system's root
/// certificates (which also honor `SSL_CERT_FILE` and `SSL_CERT_DIR`).
fn tls_config() -> Result<Arc<ClientConfig>, String> {
    static CONFIG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = RootCertStore::empty();
            let found = rustls_native_certs::load_native_certs();
            for e in &found.errors {
                log::warn!("radio: reading the system certificates: {e}");
            }
            let (added, _) = roots.add_parsable_certificates(found.certs);
            if added == 0 {
                return Err("no trusted root certificates on this system".into());
            }
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let config = ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())?
                .with_root_certificates(roots)
                .with_no_client_auth();
            Ok(Arc::new(config))
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_split() {
        let u = Url::parse("https://listen.example.com/station_128k.mp3").unwrap();
        assert_eq!(
            u,
            Url {
                tls: true,
                host: "listen.example.com".into(),
                port: 443,
                path: "/station_128k.mp3".into()
            }
        );
        let u = Url::parse("http://Example.com:8000?x=1#frag").unwrap();
        assert_eq!(
            (u.tls, u.host.as_str(), u.port),
            (false, "example.com", 8000)
        );
        assert_eq!(u.path, "/?x=1");
        assert_eq!(Url::parse("https://[::1]:8443/a").unwrap().host, "::1");
        assert_eq!(Url::parse("https://e.com").unwrap().path, "/");
        assert!(Url::parse("ftp://e.com/a").is_err());
        assert!(Url::parse("https:///a").is_err());
        assert!(Url::parse("https://e.com:x/a").is_err());
        assert!(Url::parse("https://u:p@e.com/a").is_err());
        let j = u.join("/elsewhere").unwrap();
        assert_eq!(
            (j.host.as_str(), j.path.as_str()),
            ("example.com", "/elsewhere")
        );
        assert!(u.join("https://other.example/s").unwrap().tls);
        assert!(u.join("relative").is_err());
    }

    #[test]
    fn requests_ask_for_icy_metadata_in_http_1_0() {
        let r = request(&Url::parse("https://e.com/s?x").unwrap());
        assert!(r.starts_with("GET /s?x HTTP/1.0\r\nHost: e.com\r\n"), "{r}");
        assert!(r.contains("\r\nIcy-MetaData: 1\r\n") && r.ends_with("\r\n\r\n"));
        let r = request(&Url::parse("http://e.com:8000/s").unwrap());
        assert!(r.contains("Host: e.com:8000\r\n"), "{r}");
    }

    #[test]
    fn heads_parse() {
        let bytes = b"HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nicy-metaint:16000\r\n\
                      icy-name: A Station\r\n\r\n\xff\xfb";
        let end = head_end(bytes).unwrap();
        assert_eq!(&bytes[end..], b"\xff\xfb");
        let h = parse_head(&bytes[..end]).unwrap();
        assert_eq!(h.status, 200);
        assert_eq!(h.header("content-type"), Some("audio/mpeg"));
        assert_eq!(h.header("icy-metaint"), Some("16000"));
        assert_eq!(h.header("icy-name"), Some("A Station"));
        // SHOUTcast v1, bare newlines.
        let bytes = b"ICY 200 OK\nicy-metaint: 8192\n\nbody";
        let end = head_end(bytes).unwrap();
        let h = parse_head(&bytes[..end]).unwrap();
        assert_eq!((h.status, h.header("icy-metaint")), (200, Some("8192")));
        assert_eq!(
            parse_head(b"HTTP/1.0 302 Found\r\nLocation: /x\r\n\r\n")
                .unwrap()
                .header("location"),
            Some("/x")
        );
        assert!(parse_head(b"SSH-2.0-x\r\n\r\n").is_err());
        assert!(parse_head(b"HTTP/1.1 OK\r\n\r\n").is_err());
        assert_eq!(head_end(b"HTTP/1.1 200 OK\r\nA: b\r\n"), None);
    }

    #[test]
    fn no_proxy_matches_hosts_and_suffixes() {
        let list = "localhost, .internal,example.com";
        assert!(bypasses("localhost", list));
        assert!(bypasses("a.internal", list));
        assert!(bypasses("cdn.example.com", list));
        assert!(!bypasses("notexample.com", list));
        assert!(!bypasses("stream.example.org", list));
        assert!(bypasses("anything", "*"));
        assert!(!bypasses("anything", ""));
    }

    /// Redirects are followed and the body after the head is kept, against a server on
    /// this machine.
    #[test]
    fn opens_through_a_redirect() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            for answer in [
                "HTTP/1.1 302 Found\r\nLocation: /real\r\n\r\n".to_string(),
                "ICY 200 OK\r\ncontent-type: audio/mpeg\r\n\r\nBODY".to_string(),
            ] {
                let (mut s, _) = listener.accept().unwrap();
                let mut req = Vec::new();
                let mut b = [0u8; 256];
                while head_end(&req).is_none() {
                    let n = s.read(&mut b).unwrap();
                    req.extend_from_slice(&b[..n]);
                }
                s.write_all(answer.as_bytes()).unwrap();
            }
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let (mut conn, head, mut body) =
            open(&format!("http://127.0.0.1:{port}/s"), deadline, &|| true).unwrap();
        conn.read_to_end(&mut body).unwrap();
        assert_eq!(head.header("content-type"), Some("audio/mpeg"));
        assert_eq!(body, b"BODY");
        server.join().unwrap();
    }

    /// A server that answers too slowly for one read timeout is waited for: the handshake
    /// fails only on what the server does (here it hangs up after 1.2 s), not on the
    /// 500 ms socket timeout.
    #[test]
    fn a_slow_tls_handshake_is_waited_for() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(1200));
            drop(s);
        });
        let t0 = Instant::now();
        let deadline = t0 + Duration::from_secs(10);
        let url = Url::parse(&format!("https://127.0.0.1:{port}/s")).unwrap();
        let Err(e) = connect(&url, deadline, &|| true) else {
            panic!("a server that hangs up cannot complete a handshake");
        };
        assert!(
            t0.elapsed() >= Duration::from_secs(1),
            "gave up after {:?}: {e}",
            t0.elapsed()
        );
        assert!(e.starts_with("TLS handshake:"), "{e}");
        // And the deadline still bounds it.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = Url::parse(&format!("https://127.0.0.1:{port}/s")).unwrap();
        let t0 = Instant::now();
        let e = connect(&url, t0 + Duration::from_millis(800), &|| true).err();
        assert_eq!(e.as_deref(), Some("timed out in the TLS handshake"));
        drop(listener);
    }

    #[test]
    fn https_never_redirects_to_http() {
        let tls = Url::parse("https://e.com/a").unwrap();
        assert!(tls.redirect("http://e.com/b").is_err());
        assert!(tls.redirect("https://f.com/b").unwrap().tls);
        assert!(tls.redirect("/b").unwrap().tls);
        let plain = Url::parse("http://e.com/a").unwrap();
        assert!(
            plain.redirect("https://e.com/b").unwrap().tls,
            "upgrading is fine"
        );
    }
}
