//! Minimal HTTP/1.1 client for LAN calls (rpcd, AdGuard Home).
//! Avoids pulling in hyper-client/reqwest + TLS just to talk to 192.168.1.x.

use std::{process::Stdio, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

pub type Result<T> = std::result::Result<T, String>;

pub async fn request(
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<(u16, Vec<u8>)> {
    let response = request_plain_http(method, host, path, headers, body).await?;
    if matches!(response.0, 301 | 302 | 307 | 308) {
        // OpenWrt commonly enables uhttpd's `redirect_https`, including for `/ubus`.
        // Keep the binary free of a TLS stack and use the router's curl for the redirected
        // request. Request bodies are written to curl's stdin, so rpcd login passwords never
        // appear in argv/`ps`. Router certificates are self-signed, hence `--insecure`; this
        // still improves on the original LAN HTTP transport by encrypting the connection.
        return request_https_with_curl(method, host, path, headers, body).await;
    }
    Ok(response)
}

async fn request_plain_http(
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<(u16, Vec<u8>)> {
    let addr = if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:80")
    };
    let fut = async {
        let mut s = TcpStream::connect(&addr)
            .await
            .map_err(|e| format!("connect {addr}: {e}"))?;
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        if let Some(b) = body {
            req.push_str(&format!("Content-Length: {}\r\n", b.len()));
        }
        req.push_str("\r\n");
        s.write_all(req.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        if let Some(b) = body {
            s.write_all(b).await.map_err(|e| e.to_string())?;
        }
        let mut buf = Vec::with_capacity(8192);
        s.read_to_end(&mut buf).await.map_err(|e| e.to_string())?;
        parse_response(&buf)
    };
    timeout(Duration::from_secs(5), fut)
        .await
        .map_err(|_| format!("timeout {addr}"))?
}

async fn request_https_with_curl(
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<(u16, Vec<u8>)> {
    let url = format!("https://{host}{path}");
    let mut cmd = tokio::process::Command::new("curl");
    cmd.args([
        "--silent",
        "--show-error",
        "--insecure",
        "--http1.1",
        "--max-time",
        "5",
        "--request",
        method,
    ]);
    for (key, value) in headers {
        cmd.arg("--header").arg(format!("{key}: {value}"));
    }
    if body.is_some() {
        cmd.args(["--data-binary", "@-"]);
    }
    cmd.args(["--output", "-", "--write-out", "\n%{http_code}"])
        .arg(&url)
        .stdin(if body.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| format!("curl {url}: {e}"))?;
    if let Some(bytes) = body {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("curl {url}: no stdin"))?;
        stdin
            .write_all(bytes)
            .await
            .map_err(|e| format!("curl {url}: {e}"))?;
    }
    let out = timeout(Duration::from_secs(6), child.wait_with_output())
        .await
        .map_err(|_| format!("timeout {url}"))?
        .map_err(|e| format!("curl {url}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "curl {url}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    let split = out
        .stdout
        .iter()
        .rposition(|b| *b == b'\n')
        .ok_or_else(|| format!("curl {url}: missing status"))?;
    let status = std::str::from_utf8(&out.stdout[split + 1..])
        .map_err(|e| format!("curl {url}: {e}"))?
        .trim()
        .parse::<u16>()
        .map_err(|e| format!("curl {url}: bad status: {e}"))?;
    Ok((status, out.stdout[..split].to_vec()))
}

fn parse_response(buf: &[u8]) -> Result<(u16, Vec<u8>)> {
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("no header terminator")?;
    let head = std::str::from_utf8(&buf[..split]).map_err(|e| e.to_string())?;
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or("bad status line")?;
    let body = &buf[split + 4..];
    let chunked = head.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    Ok((
        status,
        if chunked {
            dechunk(body)?
        } else {
            body.to_vec()
        },
    ))
}

fn dechunk(mut b: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let nl = b.windows(2).position(|w| w == b"\r\n").ok_or("bad chunk")?;
        let size_str = std::str::from_utf8(&b[..nl]).map_err(|e| e.to_string())?;
        let size = usize::from_str_radix(size_str.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|e| e.to_string())?;
        if size == 0 {
            return Ok(out);
        }
        let start = nl + 2;
        out.extend_from_slice(b.get(start..start + size).ok_or("short chunk")?);
        b = b.get(start + size + 2..).unwrap_or(&[]);
    }
}
