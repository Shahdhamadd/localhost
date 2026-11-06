use crate::cgi::{self, Job};
use crate::config::{Route, Server};
use crate::files;
use crate::http::{Request, Response};
use crate::sessions::{self, Sessions};
use std::path::Path;

pub enum Answer {
    Now(Response),
    Cgi(Job),
}

pub fn handle(
    servers: &[Server],
    sessions: &mut Sessions,
    host: &str,
    port: u16,
    request: &Request,
) -> Answer {
    let server = server_for(servers, host, port, request.header("host"));

    if request.path.contains("..") {
        return Answer::Now(files::error_page(server, 403));
    }
    if request.body.len() > server.max_body {
        return Answer::Now(files::error_page(server, 413));
    }
    if request.path == "/session" {
        return Answer::Now(sessions::handle(sessions, request));
    }

    let route = match server.route_for(&request.path) {
        Some(route) => route,
        None => return Answer::Now(files::error_page(server, 404)),
    };

    if !route.methods.contains(&request.method) {
        let mut response = files::error_page(server, 405);
        response.add_header("Allow", &route.methods.join(", "));
        return Answer::Now(response);
    }

    if let Some(target) = &route.redirect {
        let mut response = Response::text(301, "Moved Permanently");
        response.add_header("Location", target);
        return Answer::Now(response);
    }

    let file = file_path(route, &request.path);
    if let Some(program) = cgi_program(route, &file) {
        if request.method != "DELETE" {
            return run_cgi(server, &program, &file, request);
        }
    }

    let response = match request.method.as_str() {
        "POST" => files::upload(server, &file, request),
        "DELETE" => files::delete(server, &file),
        _ => files::serve(server, route, &file, &request.path),
    };
    Answer::Now(response)
}

pub fn error_for_door(servers: &[Server], host: &str, port: u16, status: u16) -> Response {
    files::error_page(server_for(servers, host, port, ""), status)
}

fn run_cgi(server: &Server, program: &str, file: &str, request: &Request) -> Answer {
    let script = Path::new(file);
    if !script.is_file() {
        return Answer::Now(files::error_page(server, 404));
    }
    match cgi::start(program, script, request) {
        Some(job) => Answer::Cgi(job),
        None => Answer::Now(files::error_page(server, 500)),
    }
}

fn cgi_program(route: &Route, file: &str) -> Option<String> {
    let extension = Path::new(file).extension()?.to_str()?;
    route.cgi.get(&format!(".{}", extension)).cloned()
}

pub fn server_for<'a>(servers: &'a [Server], host: &str, port: u16, host_header: &str) -> &'a Server {
    let name = host_header.split(':').next().unwrap_or("");
    let mut first = None;

    for server in servers {
        if server.host != host || !server.ports.contains(&port) {
            continue;
        }
        if server.names.iter().any(|n| n == name) {
            return server;
        }
        if first.is_none() {
            first = Some(server);
        }
    }
    first.unwrap_or(&servers[0])
}

impl Server {
    pub fn route_for(&self, path: &str) -> Option<&Route> {
        let mut best: Option<&Route> = None;

        for route in &self.routes {
            let matches = route.path == "/"
                || path == route.path
                || path.starts_with(&format!("{}/", route.path));
            if !matches {
                continue;
            }
            let longer = match best {
                Some(old) => route.path.len() > old.path.len(),
                None => true,
            };
            if longer {
                best = Some(route);
            }
        }
        best
    }
}

pub fn file_path(route: &Route, path: &str) -> String {
    let rest = &path[route.path.len()..];
    format!("{}/{}", route.root, rest.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use crate::http::{self, Parse};

    const CONFIG: &str = "
server {
    host 127.0.0.1
    ports 8080 8081
    server_name localhost
    client_max_body_size 100
    error_page 404 www/errors/404.html
    route / {
        root www/main
        methods GET
        index index.html
        listing off
    }
    route /files {
        root www/main/files
        methods GET
        listing on
    }
    route /uploads {
        root www/main/uploads
        methods GET POST DELETE
    }
    route /old {
        redirect /
    }
    route /cgi {
        root www/main/cgi
        methods GET POST
        cgi .py python3
    }
    route /broken-cgi {
        root www/main/cgi
        cgi .py no-such-program-anywhere
    }
}

server {
    host 127.0.0.1
    ports 8080
    server_name test.com
    route / {
        root www/test
    }
}

server {
    host 127.0.0.1
    ports 9000
    server_name admin
    route / {
        root www/admin
    }
}
";

    fn servers() -> Vec<Server> {
        let (servers, problems) = config::load_text(CONFIG).expect("config parses");
        assert!(problems.is_empty(), "{:?}", problems);
        servers
    }

    fn request(raw: &[u8]) -> Request {
        match http::parse(raw) {
            Parse::Done(request, _) => request,
            _ => panic!("test request does not parse"),
        }
    }

    fn answer(servers: &[Server], raw: &[u8]) -> Response {
        let mut sessions = Sessions::new();
        match handle(servers, &mut sessions, "127.0.0.1", 8080, &request(raw)) {
            Answer::Now(response) => response,
            Answer::Cgi(_) => panic!("unexpected CGI job"),
        }
    }

    fn body(response: &Response) -> String {
        String::from_utf8_lossy(&response.body).to_string()
    }

    #[test]
    fn longest_route_prefix_wins() {
        let servers = servers();
        let server = &servers[0];
        assert_eq!(server.route_for("/files/hello.txt").unwrap().path, "/files");
        assert_eq!(server.route_for("/files").unwrap().path, "/files");
        assert_eq!(server.route_for("/filesystem").unwrap().path, "/");
        assert_eq!(server.route_for("/about.html").unwrap().path, "/");
        assert_eq!(server.route_for("/cgi/hello.py").unwrap().path, "/cgi");
    }

    #[test]
    fn url_paths_map_onto_route_roots() {
        let servers = servers();
        let server = &servers[0];
        let route = server.route_for("/files/hello.txt").unwrap();
        assert_eq!(file_path(route, "/files/hello.txt"), "www/main/files/hello.txt");
        let route = server.route_for("/about.html").unwrap();
        assert_eq!(file_path(route, "/about.html"), "www/main/about.html");
        let route = server.route_for("/uploads/").unwrap();
        assert_eq!(file_path(route, "/uploads/"), "www/main/uploads/");
    }

    #[test]
    fn cgi_is_chosen_by_extension() {
        let servers = servers();
        let route = servers[0].route_for("/cgi/hello.py").unwrap();
        assert_eq!(cgi_program(route, "www/main/cgi/hello.py").as_deref(), Some("python3"));
        assert_eq!(cgi_program(route, "www/main/cgi/notes.txt"), None);
        let plain = servers[0].route_for("/about.html").unwrap();
        assert_eq!(cgi_program(plain, "www/main/hello.py"), None);
    }

    #[test]
    fn redirect_routes_answer_301() {
        let response = answer(&servers(), b"GET /old HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(response.status, 301);
        assert_eq!(response.header("location"), "/");
    }

    #[test]
    fn server_is_picked_by_host_header_with_a_default() {
        let servers = servers();
        assert_eq!(server_for(&servers, "127.0.0.1", 8080, "test.com:8080").names[0], "test.com");
        assert_eq!(server_for(&servers, "127.0.0.1", 8080, "localhost").names[0], "localhost");
        assert_eq!(server_for(&servers, "127.0.0.1", 8080, "unknown.org").names[0], "localhost");
        assert_eq!(server_for(&servers, "127.0.0.1", 8080, "").names[0], "localhost");
        assert_eq!(server_for(&servers, "127.0.0.1", 8081, "test.com").names[0], "localhost");
        assert_eq!(server_for(&servers, "127.0.0.1", 9000, "").names[0], "admin");
    }

    #[test]
    fn static_pages_give_200() {
        let servers = servers();
        let response = answer(&servers, b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(response.status, 200);
        assert!(response.header("content-type").starts_with("text/html"));
        assert!(body(&response).contains("Welcome"));
        let response = answer(&servers, b"GET /files/hello.txt HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(response.status, 200);
    }

    #[test]
    fn directories_redirect_list_or_refuse() {
        let servers = servers();
        let response = answer(&servers, b"GET /files HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(response.status, 301);
        assert_eq!(response.header("location"), "/files/");
        let response = answer(&servers, b"GET /files/ HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(response.status, 200);
        assert!(body(&response).contains("hello.txt"));
        let response = answer(&servers, b"GET /cgi/ HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(response.status, 403);
    }

    #[test]
    fn missing_files_give_404_with_the_custom_page() {
        let servers = servers();
        let response = answer(&servers, b"GET /nothing-here HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(response.status, 404);
        assert!(body(&response).contains("does not exist"));
        let response = answer(&servers, b"GET /nothing-here HTTP/1.1\r\nHost: test.com\r\n\r\n");
        assert_eq!(response.status, 404);
        assert!(body(&response).contains("404 Not Found"));
        let response = answer(&servers, b"GET /cgi/nope.py HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(response.status, 404);
    }

    #[test]
    fn forbidden_methods_give_405_with_allow() {
        let servers = servers();
        let response = answer(&servers, b"DELETE /files/hello.txt HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(response.status, 405);
        assert_eq!(response.header("allow"), "GET");
        let response = answer(&servers, b"PUT / HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(response.status, 405);
    }

    #[test]
    fn path_escapes_give_403() {
        let response = answer(&servers(), b"GET /../Cargo.toml HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(response.status, 403);
        let response = answer(&servers(), b"GET /files/%2e%2e/index.html HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(response.status, 403);
    }

    #[test]
    fn bodies_over_the_limit_give_413() {
        let mut raw = b"POST /uploads/big.bin HTTP/1.1\r\nHost: localhost\r\nContent-Length: 101\r\n\r\n".to_vec();
        raw.extend_from_slice(&[b'x'; 101]);
        let response = answer(&servers(), &raw);
        assert_eq!(response.status, 413);
    }

    #[test]
    fn malformed_requests_give_400_for_that_door() {
        let servers = servers();
        assert!(matches!(http::parse(b"HELLO\r\n\r\n"), Parse::Bad));
        let response = error_for_door(&servers, "127.0.0.1", 8080, 400);
        assert_eq!(response.status, 400);
        assert_eq!(error_for_door(&servers, "127.0.0.1", 9000, 413).status, 413);
    }

    #[test]
    fn unstartable_cgi_gives_500() {
        let response = answer(&servers(), b"GET /broken-cgi/hello.py HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(response.status, 500);
    }

    #[test]
    fn runnable_cgi_returns_a_job() {
        let servers = servers();
        let mut sessions = Sessions::new();
        let request = request(b"GET /cgi/hello.py HTTP/1.1\r\nHost: localhost\r\n\r\n");
        match handle(&servers, &mut sessions, "127.0.0.1", 8080, &request) {
            Answer::Cgi(mut job) => {
                let _ = job.child.kill();
                let _ = job.child.wait();
            }
            Answer::Now(response) => panic!("expected a CGI job, got {}", response.status),
        }
    }
}
