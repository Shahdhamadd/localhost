use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Request {
    pub method: String,
    pub target: String,
    pub path: String,
    pub version: String,
    pub query: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

pub enum Head {
    Incomplete,
    Bad,
    Done(Request, usize),
}

pub enum Parse {
    Incomplete,
    Bad,
    Done(Request, usize),
}

impl Request {
    pub fn header(&self, name: &str) -> &str {
        match self.headers.get(name) {
            Some(value) => value,
            None => "",
        }
    }

    pub fn keep_alive(&self) -> bool {
        let connection = self.header("connection").to_ascii_lowercase();
        if self.version == "HTTP/1.0" {
            return connection == "keep-alive";
        }
        connection != "close"
    }

    pub fn cookie(&self, name: &str) -> &str {
        for part in self.header("cookie").split(';') {
            if let Some((key, value)) = part.trim().split_once('=') {
                if key == name {
                    return value;
                }
            }
        }
        ""
    }

    pub fn chunked(&self) -> bool {
        self.header("transfer-encoding")
            .split(',')
            .next_back()
            .map(|coding| coding.trim().eq_ignore_ascii_case("chunked"))
            .unwrap_or(false)
    }

    pub fn content_length(&self) -> Result<Option<usize>, ()> {
        let text = self.header("content-length");
        if text.is_empty() {
            return Ok(None);
        }
        text.parse().map(Some).map_err(|_| ())
    }
}

pub fn parse_head(input: &[u8]) -> Head {
    let head_end = match find(input, b"\r\n\r\n") {
        Some(position) => position,
        None => return Head::Incomplete,
    };
    let head = match std::str::from_utf8(&input[..head_end]) {
        Ok(text) => text,
        Err(_) => return Head::Bad,
    };

    let mut lines = head.split("\r\n");
    let first: Vec<&str> = lines.next().unwrap_or("").split(' ').collect();
    if first.len() != 3 || first[0].is_empty() || !first[2].starts_with("HTTP/1.") {
        return Head::Bad;
    }
    let (target, query) = first[1].split_once('?').unwrap_or((first[1], ""));
    if !target.starts_with('/') {
        return Head::Bad;
    }
    let path = match decode_path(target) {
        Some(path) => path,
        None => return Head::Bad,
    };

    let mut headers = HashMap::new();
    for line in lines {
        match line.split_once(':') {
            Some((name, value)) if !name.trim().is_empty() => {
                headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
            }
            _ => return Head::Bad,
        }
    }

    let request = Request {
        method: first[0].to_string(),
        target: target.to_string(),
        path,
        version: first[2].to_string(),
        query: query.to_string(),
        headers,
        body: Vec::new(),
    };
    if request.header("host").is_empty() {
        return Head::Bad;
    }
    Head::Done(request, head_end + 4)
}

pub fn parse(input: &[u8]) -> Parse {
    let (mut request, body_start) = match parse_head(input) {
        Head::Done(request, body_start) => (request, body_start),
        Head::Incomplete => return Parse::Incomplete,
        Head::Bad => return Parse::Bad,
    };

    let rest = &input[body_start..];
    if request.chunked() {
        return read_chunked(request, rest, body_start);
    }

    let length = match request.content_length() {
        Ok(Some(number)) => number,
        Ok(None) => 0,
        Err(_) => return Parse::Bad,
    };
    if rest.len() < length {
        return Parse::Incomplete;
    }
    request.body = rest[..length].to_vec();
    Parse::Done(request, body_start + length)
}

fn read_chunked(mut request: Request, data: &[u8], body_start: usize) -> Parse {
    let mut position = 0;
    loop {
        let line_end = match find(&data[position..], b"\r\n") {
            Some(offset) => position + offset,
            None => return Parse::Incomplete,
        };
        let line = String::from_utf8_lossy(&data[position..line_end]);
        let size_text = line.split(';').next().unwrap_or("").trim();
        let size = match usize::from_str_radix(size_text, 16) {
            Ok(number) => number,
            Err(_) => return Parse::Bad,
        };
        position = line_end + 2;

        if size == 0 {
            break;
        }
        if data.len() < position + size + 2 {
            return Parse::Incomplete;
        }
        request.body.extend_from_slice(&data[position..position + size]);
        if &data[position + size..position + size + 2] != b"\r\n" {
            return Parse::Bad;
        }
        position += size + 2;
    }

    loop {
        let line_end = match find(&data[position..], b"\r\n") {
            Some(offset) => position + offset,
            None => return Parse::Incomplete,
        };
        let empty = line_end == position;
        position = line_end + 2;
        if empty {
            break;
        }
    }
    Parse::Done(request, body_start + position)
}

pub fn decode_path(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            let value = u8::from_str_radix(hex, 16).ok()?;
            if value == 0 {
                return None;
            }
            out.push(value);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        let keep = byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte);
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{:02X}", byte));
        }
    }
    out
}

pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

pub fn find(data: &[u8], pattern: &[u8]) -> Option<usize> {
    data.windows(pattern.len()).position(|window| window == pattern)
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, content_type: &str, body: Vec<u8>) -> Response {
        let mut content_type = content_type.to_string();
        if content_type.starts_with("text/") && !content_type.contains("charset") {
            content_type.push_str("; charset=utf-8");
        }
        let headers = vec![(String::from("Content-Type"), content_type)];
        Response { status, headers, body }
    }

    pub fn text(status: u16, message: &str) -> Response {
        Response::new(status, "text/plain", message.as_bytes().to_vec())
    }

    pub fn default_error(status: u16) -> Response {
        let html = format!(
            "<html><body><h1>{} {}</h1><p>localhost server</p></body></html>",
            status,
            reason(status)
        );
        Response::new(status, "text/html", html.into_bytes())
    }

    pub fn add_header(&mut self, name: &str, value: &str) {
        self.headers.push((name.to_string(), value.to_string()));
    }

    #[cfg(test)]
    pub fn header(&self, name: &str) -> &str {
        for (key, value) in &self.headers {
            if key.eq_ignore_ascii_case(name) {
                return value;
            }
        }
        ""
    }

    pub fn to_bytes(&self, keep_alive: bool) -> Vec<u8> {
        let mut head = format!("HTTP/1.1 {} {}\r\n", self.status, reason(self.status));
        head.push_str("Server: localhost\r\n");
        head.push_str(&format!("Date: {}\r\n", http_date(now())));
        for (name, value) in &self.headers {
            head.push_str(&format!("{}: {}\r\n", name, value));
        }
        head.push_str(&format!("Content-Length: {}\r\n", self.body.len()));
        if keep_alive {
            head.push_str("Connection: keep-alive\r\n\r\n");
        } else {
            head.push_str("Connection: close\r\n\r\n");
        }

        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        504 => "Gateway Timeout",
        _ => "Unknown",
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn http_date(seconds: u64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = (seconds / 86400) as i64;
    let rest = seconds % 86400;
    let (hour, minute, second) = (rest / 3600, rest % 3600 / 60, rest % 60);

    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };

    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        DAYS[(days % 7) as usize],
        day,
        MONTHS[(month - 1) as usize],
        year,
        hour,
        minute,
        second
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(input: &[u8]) -> (Request, usize) {
        match parse(input) {
            Parse::Done(request, used) => (request, used),
            Parse::Incomplete => panic!("parse said incomplete"),
            Parse::Bad => panic!("parse said bad"),
        }
    }

    fn is_bad(input: &[u8]) -> bool {
        matches!(parse(input), Parse::Bad)
    }

    fn is_incomplete(input: &[u8]) -> bool {
        matches!(parse(input), Parse::Incomplete)
    }

    #[test]
    fn get_request_line_and_headers() {
        let raw = b"GET /about.html?x=1&y=2 HTTP/1.1\r\nHost: localhost:8080\r\nAccept: */*\r\n\r\n";
        let (request, used) = done(raw);
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/about.html");
        assert_eq!(request.query, "x=1&y=2");
        assert_eq!(request.version, "HTTP/1.1");
        assert_eq!(request.header("host"), "localhost:8080");
        assert_eq!(request.header("accept"), "*/*");
        assert!(request.body.is_empty());
        assert_eq!(used, raw.len());
    }

    #[test]
    fn header_names_are_case_insensitive() {
        let (request, _) = done(b"GET / HTTP/1.1\r\nHOST: a\r\nContent-TYPE: text/plain\r\n\r\n");
        assert_eq!(request.header("content-type"), "text/plain");
    }

    #[test]
    fn post_with_content_length_body() {
        let raw = b"POST /up HTTP/1.1\r\nHost: a\r\nContent-Length: 5\r\n\r\nhelloEXTRA";
        let (request, used) = done(raw);
        assert_eq!(request.method, "POST");
        assert_eq!(request.body, b"hello");
        assert_eq!(used, raw.len() - b"EXTRA".len(), "EXTRA belongs to the next request");
    }

    #[test]
    fn delete_request() {
        let (request, _) = done(b"DELETE /uploads/a.txt HTTP/1.1\r\nHost: a\r\n\r\n");
        assert_eq!(request.method, "DELETE");
        assert_eq!(request.path, "/uploads/a.txt");
    }

    #[test]
    fn chunked_body_is_joined() {
        let raw = b"POST /cgi/echo.py HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let (request, used) = done(raw);
        assert_eq!(request.body, b"hello world");
        assert_eq!(used, raw.len());
    }

    #[test]
    fn chunked_accepts_extensions_trailers_and_any_case() {
        let raw = b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: Chunked\r\n\r\n3;name=value\r\nabc\r\n0\r\nX-Trailer: 1\r\n\r\n";
        let (request, used) = done(raw);
        assert_eq!(request.body, b"abc");
        assert_eq!(used, raw.len());
    }

    #[test]
    fn chunked_waits_for_the_last_chunk() {
        assert!(is_incomplete(b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n"));
        assert!(is_incomplete(b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel"));
    }

    #[test]
    fn chunked_rejects_bad_sizes() {
        assert!(is_bad(b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\nhello\r\n0\r\n\r\n"));
        assert!(is_bad(b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nhello\r\n0\r\n\r\n"));
    }

    #[test]
    fn incomplete_until_the_body_arrives() {
        assert!(is_incomplete(b"GET / HTTP/1.1\r\nHost: a\r\n"));
        assert!(is_incomplete(b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 10\r\n\r\nhello"));
    }

    #[test]
    fn pipelined_requests_report_bytes_used() {
        let two = b"GET /a HTTP/1.1\r\nHost: a\r\n\r\nGET /b HTTP/1.1\r\nHost: a\r\n\r\n";
        let (first, used) = done(two);
        assert_eq!(first.path, "/a");
        let (second, used2) = done(&two[used..]);
        assert_eq!(second.path, "/b");
        assert_eq!(used + used2, two.len());
    }

    #[test]
    fn malformed_requests_are_bad() {
        assert!(is_bad(b"HELLO\r\n\r\n"));
        assert!(is_bad(b"GET / HTTP/1.1\r\n\r\n"));
        assert!(is_bad(b"GET / HTTP/2.0\r\nHost: a\r\n\r\n"));
        assert!(is_bad(b"GET index.html HTTP/1.1\r\nHost: a\r\n\r\n"));
        assert!(is_bad(b"GET / HTTP/1.1\r\nHost: a\r\nno colon here\r\n\r\n"));
        assert!(is_bad(b"GET / HTTP/1.1\r\nHost: a\r\n: empty name\r\n\r\n"));
        assert!(is_bad(b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: abc\r\n\r\n"));
        assert!(is_bad(b"GET /%zz HTTP/1.1\r\nHost: a\r\n\r\n"));
        assert!(is_bad(b"GET /a%00b HTTP/1.1\r\nHost: a\r\n\r\n"));
    }

    #[test]
    fn path_is_percent_decoded_but_target_is_kept() {
        let (request, _) = done(b"GET /files/hello%20world.txt?q=a%20b HTTP/1.1\r\nHost: a\r\n\r\n");
        assert_eq!(request.path, "/files/hello world.txt");
        assert_eq!(request.target, "/files/hello%20world.txt");
        assert_eq!(request.query, "q=a%20b");
        assert_eq!(encode_path("/files/hello world.txt"), "/files/hello%20world.txt");
    }

    #[test]
    fn keep_alive_rules() {
        let (r, _) = done(b"GET / HTTP/1.1\r\nHost: a\r\n\r\n");
        assert!(r.keep_alive());
        let (r, _) = done(b"GET / HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n");
        assert!(!r.keep_alive());
        let (r, _) = done(b"GET / HTTP/1.0\r\nHost: a\r\n\r\n");
        assert!(!r.keep_alive());
        let (r, _) = done(b"GET / HTTP/1.0\r\nHost: a\r\nConnection: Keep-Alive\r\n\r\n");
        assert!(r.keep_alive());
    }

    #[test]
    fn cookies_are_read_by_name() {
        let (r, _) = done(b"GET / HTTP/1.1\r\nHost: a\r\nCookie: theme=dark; session_id=abc123\r\n\r\n");
        assert_eq!(r.cookie("session_id"), "abc123");
        assert_eq!(r.cookie("missing"), "");
    }

    #[test]
    fn response_bytes_have_status_line_and_length() {
        let mut response = Response::new(404, "text/html", b"<h1>nope</h1>".to_vec());
        response.add_header("X-Test", "1");
        let text = String::from_utf8(response.to_bytes(true)).unwrap();
        assert!(text.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(text.contains("Content-Type: text/html; charset=utf-8\r\n"));
        assert!(text.contains("Content-Length: 13\r\n"));
        assert!(text.contains("Connection: keep-alive\r\n"));
        assert!(text.contains("Date: "));
        assert!(text.contains("X-Test: 1\r\n"));
        assert!(text.ends_with("\r\n\r\n<h1>nope</h1>"));
        let closed = String::from_utf8(Response::text(200, "ok").to_bytes(false)).unwrap();
        assert!(closed.contains("Connection: close\r\n"));
    }

    #[test]
    fn http_dates_are_formatted_like_rfc_9110() {
        assert_eq!(http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(http_date(1_000_000_000), "Sun, 09 Sep 2001 01:46:40 GMT");
        assert_eq!(http_date(1_709_164_800), "Thu, 29 Feb 2024 00:00:00 GMT");
    }

    #[test]
    fn html_is_escaped() {
        assert_eq!(escape_html("<a href=\"x\">&'</a>"), "&lt;a href=&quot;x&quot;&gt;&amp;&#39;&lt;/a&gt;");
    }
}
