use crate::config::{Route, Server};
use crate::http::{self, Request, Response};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

pub fn serve(server: &Server, route: &Route, file: &str, url_path: &str) -> Response {
    let path = Path::new(file);

    if path.is_dir() {
        if !url_path.ends_with('/') {
            let mut response = Response::text(301, "Moved Permanently");
            response.add_header("Location", &format!("{}/", http::encode_path(url_path)));
            return response;
        }
        if let Some(index) = &route.index {
            let index_file = path.join(index);
            if index_file.is_file() {
                return send_file(server, &index_file);
            }
        }
        if route.listing {
            return listing(path, url_path);
        }
        return error_page(server, 403);
    }

    if path.is_file() {
        return send_file(server, path);
    }
    error_page(server, 404)
}

pub fn error_page(server: &Server, status: u16) -> Response {
    if let Some(file) = server.error_pages.get(&status) {
        if let Ok(bytes) = fs::read(file) {
            return Response::new(status, "text/html", bytes);
        }
    }
    Response::default_error(status)
}

fn send_file(server: &Server, path: &Path) -> Response {
    match fs::read(path) {
        Ok(bytes) => Response::new(200, content_type(path), bytes),
        Err(e) if e.kind() == ErrorKind::PermissionDenied => error_page(server, 403),
        Err(_) => error_page(server, 500),
    }
}

fn listing(dir: &Path, url_path: &str) -> Response {
    let mut entries: Vec<(String, bool)> = Vec::new();
    if let Ok(read) = fs::read_dir(dir) {
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.path().is_dir();
            entries.push((name, is_dir));
        }
    }
    entries.sort();

    let mut html = format!(
        "<!DOCTYPE html><html><head><title>Index of {0}</title></head><body><h1>Index of {0}</h1><ul>",
        http::escape_html(url_path)
    );
    for (name, is_dir) in entries {
        let suffix = if is_dir { "/" } else { "" };
        html.push_str(&format!(
            "<li><a href=\"{}{}\">{}{}</a></li>",
            http::encode_path(&name),
            suffix,
            http::escape_html(&name),
            suffix
        ));
    }
    html.push_str("</ul></body></html>");
    Response::new(200, "text/html", html.into_bytes())
}

fn content_type(path: &Path) -> &'static str {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match extension {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        "json" => "application/json",
        "txt" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

pub fn upload(server: &Server, file: &str, request: &Request) -> Response {
    let path = Path::new(file);
    let (target, content) = if path.is_dir() {
        match multipart_file(request) {
            Some((name, content)) => (path.join(name), content),
            None => return error_page(server, 400),
        }
    } else {
        (path.to_path_buf(), &request.body[..])
    };

    match fs::write(&target, content) {
        Ok(_) => {
            let message = format!("<p>Saved {} bytes.</p><a href=\"/\">Back home</a>", content.len());
            Response::new(201, "text/html", message.into_bytes())
        }
        Err(e) if e.kind() == ErrorKind::PermissionDenied => error_page(server, 403),
        Err(e) if e.kind() == ErrorKind::NotFound => error_page(server, 404),
        Err(_) => error_page(server, 500),
    }
}

pub fn delete(server: &Server, file: &str) -> Response {
    let path = Path::new(file);
    if path.is_dir() {
        return error_page(server, 403);
    }
    if !path.is_file() {
        return error_page(server, 404);
    }
    match fs::remove_file(path) {
        Ok(_) => Response::text(200, "Deleted"),
        Err(e) if e.kind() == ErrorKind::PermissionDenied => error_page(server, 403),
        Err(_) => error_page(server, 500),
    }
}

pub fn multipart_file(request: &Request) -> Option<(String, &[u8])> {
    let boundary = request.header("content-type").split("boundary=").nth(1)?;
    let marker = format!("\r\n--{}", boundary.trim_matches('"'));
    let body = &request.body;

    let name_start = http::find(body, b"filename=\"")? + 10;
    let name_end = name_start + http::find(&body[name_start..], b"\"")?;
    let data_start = name_end + http::find(&body[name_end..], b"\r\n\r\n")? + 4;
    let data_end = data_start + http::find(&body[data_start..], marker.as_bytes())?;

    let full_name = String::from_utf8_lossy(&body[name_start..name_end]).to_string();
    let name = full_name.rsplit(['/', '\\']).next()?.to_string();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    Some((name, &body[data_start..data_end]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Parse;

    fn request(raw: &[u8]) -> Request {
        match http::parse(raw) {
            Parse::Done(request, _) => request,
            _ => panic!("test request does not parse"),
        }
    }

    #[test]
    fn multipart_body_gives_name_and_content() {
        let body = b"------b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"C:\\dir\\form.txt\"\r\nContent-Type: text/plain\r\n\r\nfrom a form\r\n------b--\r\n";
        let mut raw = format!(
            "POST /uploads/ HTTP/1.1\r\nHost: a\r\nContent-Type: multipart/form-data; boundary=----b\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        raw.extend_from_slice(body);
        let request = request(&raw);
        let (name, content) = multipart_file(&request).expect("a file");
        assert_eq!(name, "form.txt");
        assert_eq!(content, b"from a form");
    }

    #[test]
    fn multipart_without_file_is_rejected() {
        let request = request(b"POST /uploads/ HTTP/1.1\r\nHost: a\r\nContent-Type: text/plain\r\nContent-Length: 3\r\n\r\nabc");
        assert!(multipart_file(&request).is_none());
    }

    #[test]
    fn content_types_by_extension() {
        assert_eq!(content_type(Path::new("a/b.html")), "text/html");
        assert_eq!(content_type(Path::new("style.css")), "text/css");
        assert_eq!(content_type(Path::new("logo.svg")), "image/svg+xml");
        assert_eq!(content_type(Path::new("blob.bin")), "application/octet-stream");
    }
}
