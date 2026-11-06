use crate::http::{Request, Response};
use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Job {
    pub child: Child,
    pub stdout: ChildStdout,
    output: Vec<u8>,
}

pub fn start(program: &str, script: &Path, request: &Request) -> Option<Job> {
    let full_path = fs::canonicalize(script).ok()?;
    let folder = full_path.parent()?;
    let name = full_path.file_name()?;
    let body = body_file(&request.body)?;

    let mut child = Command::new(program)
        .arg(name)
        .current_dir(folder)
        .env("GATEWAY_INTERFACE", "CGI/1.1")
        .env("SERVER_SOFTWARE", "localhost/0.1")
        .env("SERVER_PROTOCOL", "HTTP/1.1")
        .env("REQUEST_METHOD", &request.method)
        .env("QUERY_STRING", &request.query)
        .env("SCRIPT_NAME", &request.target)
        .env("PATH_INFO", &full_path)
        .env("CONTENT_LENGTH", request.body.len().to_string())
        .env("CONTENT_TYPE", request.header("content-type"))
        .env("HTTP_HOST", request.header("host"))
        .env("HTTP_COOKIE", request.header("cookie"))
        .stdin(Stdio::from(body))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let stdout = child.stdout.take()?;
    unsafe {
        libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
    }
    Some(Job { child, stdout, output: Vec::new() })
}

fn body_file(body: &[u8]) -> Option<File> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_nanos();
    let path = format!("/tmp/cgi-body-{}-{}", std::process::id(), nanos);
    fs::write(&path, body).ok()?;
    let file = File::open(&path).ok();
    let _ = fs::remove_file(&path);
    file
}

pub fn read_step(job: &mut Job) -> Option<Result<Response, u16>> {
    let mut buffer = [0u8; 4096];
    match job.stdout.read(&mut buffer) {
        Ok(0) => {}
        Ok(n) => {
            job.output.extend_from_slice(&buffer[..n]);
            return None;
        }
        Err(e) if e.kind() == ErrorKind::WouldBlock => return None,
        Err(_) => {}
    }

    let exit_code = finish(&mut job.child);
    if job.output.is_empty() || exit_code.is_some_and(|code| code != 0) {
        return Some(Err(500));
    }
    Some(Ok(to_response(&job.output)))
}

fn finish(child: &mut Child) -> Option<i32> {
    match child.try_wait() {
        Ok(Some(status)) => status.code(),
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

fn to_response(output: &[u8]) -> Response {
    let mut status = 200;
    let mut content_type = String::from("text/html");
    let mut headers = Vec::new();
    let mut position = 0;
    let mut has_status = false;

    loop {
        let line_end = match output[position..].iter().position(|&byte| byte == b'\n') {
            Some(offset) => position + offset,
            None => return Response::new(200, "text/html", output.to_vec()),
        };
        let line = String::from_utf8_lossy(&output[position..line_end]).trim().to_string();
        position = line_end + 1;
        if line.is_empty() {
            break;
        }
        let (name, value) = match line.split_once(':') {
            Some((name, value)) => (name.trim().to_string(), value.trim().to_string()),
            None => return Response::new(200, "text/html", output.to_vec()),
        };
        match name.to_lowercase().as_str() {
            "status" => {
                status = value.split(' ').next().unwrap_or("").parse().unwrap_or(500);
                has_status = true;
            }
            "content-type" => content_type = value,
            "content-length" => {}
            "location" => {
                if !has_status {
                    status = 302;
                }
                headers.push((name, value));
            }
            _ => headers.push((name, value)),
        }
    }

    let mut response = Response::new(status, &content_type, output[position..].to_vec());
    for (name, value) in headers {
        response.add_header(&name, &value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_headers_become_response_headers() {
        let response = to_response(b"Content-Type: text/plain\nX-Extra: yes\n\nhello");
        assert_eq!(response.status, 200);
        assert_eq!(response.header("content-type"), "text/plain; charset=utf-8");
        assert_eq!(response.header("x-extra"), "yes");
        assert_eq!(response.body, b"hello");
    }

    #[test]
    fn status_and_location_lines_are_honoured() {
        let response = to_response(b"Status: 404 Not Found\nContent-Type: text/html\n\nnope");
        assert_eq!(response.status, 404);
        let response = to_response(b"Location: /elsewhere\n\n");
        assert_eq!(response.status, 302);
        assert_eq!(response.header("location"), "/elsewhere");
    }

    #[test]
    fn output_without_headers_is_sent_as_html() {
        let response = to_response(b"just text, no blank line");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"just text, no blank line");
    }
}
