use std::collections::HashMap;
use std::fs;
use std::net::Ipv4Addr;
use std::path::Path;

pub const MAX_BODY_LIMIT: usize = 1 << 30;
pub const DEFAULT_BODY_LIMIT: usize = 1_000_000;

pub struct Route {
    pub path: String,
    pub root: String,
    pub methods: Vec<String>,
    pub index: Option<String>,
    pub listing: bool,
    pub redirect: Option<String>,
    pub cgi: HashMap<String, String>,
}

pub struct Server {
    pub host: String,
    pub ports: Vec<u16>,
    pub names: Vec<String>,
    pub max_body: usize,
    pub error_pages: HashMap<u16, String>,
    pub routes: Vec<Route>,
}

pub fn load(path: &str) -> Result<Vec<Server>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read {}: {}", path, e))?;
    let (servers, problems) = load_text(&text)?;
    for problem in &problems {
        eprintln!("config: {}", problem);
    }
    if servers.is_empty() {
        return Err(String::from("no valid server in the config file"));
    }
    Ok(servers)
}

pub fn load_text(text: &str) -> Result<(Vec<Server>, Vec<String>), String> {
    let blocks = split_blocks(text)?;

    let mut servers: Vec<Server> = Vec::new();
    let mut problems = Vec::new();
    for (number, block) in blocks.iter().enumerate() {
        match parse_server(block) {
            Ok(server) => match find_conflict(&servers, &server) {
                Some(problem) => problems.push(format!("server #{} skipped: {}", number + 1, problem)),
                None => servers.push(server),
            },
            Err(problem) => problems.push(format!("server #{} skipped: {}", number + 1, problem)),
        }
    }
    Ok((servers, problems))
}

fn split_blocks(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut blocks = Vec::new();
    let mut current = Vec::new();
    let mut depth = 0;

    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        if depth == 0 {
            if words != ["server", "{"] {
                return Err(format!("expected 'server {{' but found '{}'", line.trim()));
            }
            depth = 1;
            continue;
        }
        if words.last() == Some(&"{") {
            depth += 1;
        }
        if words == ["}"] {
            depth -= 1;
            if depth == 0 {
                blocks.push(current);
                current = Vec::new();
                continue;
            }
        }
        current.push(line.trim().to_string());
    }

    if depth != 0 {
        return Err(String::from("a '}' is missing at the end of the file"));
    }
    Ok(blocks)
}

fn parse_server(lines: &[String]) -> Result<Server, String> {
    let mut server = Server {
        host: String::new(),
        ports: Vec::new(),
        names: Vec::new(),
        max_body: DEFAULT_BODY_LIMIT,
        error_pages: HashMap::new(),
        routes: Vec::new(),
    };
    let mut route: Option<Route> = None;

    for line in lines {
        let words: Vec<&str> = line.split_whitespace().collect();
        let key = words[0];
        let values = &words[1..];

        if key == "}" {
            match route.take() {
                Some(mut finished) => {
                    if finished.methods.is_empty() {
                        finished.methods.push(String::from("GET"));
                    }
                    server.routes.push(finished);
                }
                None => return Err(String::from("a '}' has no matching '{'")),
            }
            continue;
        }

        if key == "route" {
            if route.is_some() || values.len() != 2 || values[1] != "{" {
                return Err(format!("bad route line '{}'", line));
            }
            route = Some(Route {
                path: route_path(values[0])?,
                root: String::new(),
                methods: Vec::new(),
                index: None,
                listing: false,
                redirect: None,
                cgi: HashMap::new(),
            });
            continue;
        }

        match route.as_mut() {
            Some(current) => set_route_value(current, key, values)?,
            None => set_server_value(&mut server, key, values)?,
        }
    }

    check_server(&server)?;
    Ok(server)
}

fn route_path(raw: &str) -> Result<String, String> {
    if !raw.starts_with('/') {
        return Err(format!("route '{}' must start with '/'", raw));
    }
    let trimmed = raw.trim_end_matches('/');
    Ok(if trimmed.is_empty() { String::from("/") } else { trimmed.to_string() })
}

fn set_server_value(server: &mut Server, key: &str, values: &[&str]) -> Result<(), String> {
    match key {
        "host" => server.host = one_value(key, values)?,
        "ports" => {
            for value in values {
                server.ports.push(parse_number(key, value)?);
            }
        }
        "server_name" => {
            for value in values {
                server.names.push(value.to_string());
            }
        }
        "client_max_body_size" => server.max_body = parse_number(key, &one_value(key, values)?)?,
        "error_page" => {
            if values.len() != 2 {
                return Err(String::from("'error_page' needs a code and a file"));
            }
            let code = parse_number(key, values[0])?;
            server.error_pages.insert(code, values[1].to_string());
        }
        _ => return Err(format!("unknown server setting '{}'", key)),
    }
    Ok(())
}

fn set_route_value(route: &mut Route, key: &str, values: &[&str]) -> Result<(), String> {
    match key {
        "root" => route.root = one_value(key, values)?,
        "methods" => {
            for value in values {
                if !["GET", "POST", "DELETE"].contains(value) {
                    return Err(format!("unknown method '{}'", value));
                }
                route.methods.push(value.to_string());
            }
        }
        "index" => route.index = Some(one_value(key, values)?),
        "listing" => {
            route.listing = match one_value(key, values)?.as_str() {
                "on" => true,
                "off" => false,
                other => return Err(format!("'listing' must be on or off, not '{}'", other)),
            }
        }
        "redirect" => route.redirect = Some(one_value(key, values)?),
        "cgi" => {
            if values.len() != 2 || !values[0].starts_with('.') {
                return Err(String::from("'cgi' needs an extension like .py and a program"));
            }
            route.cgi.insert(values[0].to_string(), values[1].to_string());
        }
        _ => return Err(format!("unknown route setting '{}'", key)),
    }
    Ok(())
}

fn one_value(key: &str, values: &[&str]) -> Result<String, String> {
    if values.len() != 1 {
        return Err(format!("'{}' needs exactly one value", key));
    }
    Ok(values[0].to_string())
}

fn parse_number<T: std::str::FromStr>(key: &str, value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("'{}' got '{}', which is not a valid number", key, value))
}

fn check_server(server: &Server) -> Result<(), String> {
    if server.host.parse::<Ipv4Addr>().is_err() {
        return Err(format!("host '{}' is not an address like 127.0.0.1", server.host));
    }
    if server.names.is_empty() {
        return Err(String::from("'server_name' is missing"));
    }
    if server.ports.is_empty() {
        return Err(String::from("'ports' is missing"));
    }
    for (i, port) in server.ports.iter().enumerate() {
        if *port == 0 {
            return Err(String::from("port 0 cannot be used"));
        }
        if server.ports[..i].contains(port) {
            return Err(format!("port {} is written twice", port));
        }
    }
    if server.max_body == 0 || server.max_body > MAX_BODY_LIMIT {
        return Err(format!(
            "client_max_body_size must be between 1 and {} bytes, not {}",
            MAX_BODY_LIMIT, server.max_body
        ));
    }
    for (code, file) in &server.error_pages {
        if !(400..600).contains(code) {
            return Err(format!("error_page {} is not an error code", code));
        }
        if !Path::new(file).is_file() {
            return Err(format!("error_page {}: file '{}' does not exist", code, file));
        }
    }
    if server.routes.is_empty() {
        return Err(String::from("the server has no route"));
    }
    for (i, route) in server.routes.iter().enumerate() {
        if server.routes[..i].iter().any(|other| other.path == route.path) {
            return Err(format!("route '{}' is written twice", route.path));
        }
        if route.root.is_empty() && route.redirect.is_none() {
            return Err(format!("route '{}' needs a 'root' or a 'redirect'", route.path));
        }
        if !route.root.is_empty() && !Path::new(&route.root).is_dir() {
            return Err(format!("route '{}': folder '{}' does not exist", route.path, route.root));
        }
    }
    Ok(())
}

fn find_conflict(servers: &[Server], new: &Server) -> Option<String> {
    for old in servers {
        if old.host != new.host {
            continue;
        }
        for port in &new.ports {
            if !old.ports.contains(port) {
                continue;
            }
            for name in &new.names {
                if old.names.contains(name) {
                    return Some(format!(
                        "{}:{} with name '{}' is already used by another server",
                        new.host, port, name
                    ));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "
server {
    host 0.0.0.0
    ports 8080 8081
    server_name localhost www.localhost
    client_max_body_size 500
    error_page 404 www/errors/404.html
    route / {
        root www/main
        methods GET POST
        index index.html
        listing on
    }
    route /old {
        redirect /
    }
    route /cgi/ {
        root www/main/cgi
        cgi .py python3
    }
}

server {
    host 0.0.0.0
    ports 8080
    server_name test.com
    route / {
        root www/test
    }
}
";

    fn problems_of(text: &str) -> Vec<String> {
        load_text(text).expect("file structure is fine").1
    }

    fn first_problem(text: &str) -> String {
        problems_of(text).into_iter().next().unwrap_or_default()
    }

    fn one_server(extra_server_lines: &str, route_lines: &str) -> String {
        format!(
            "server {{\n host 127.0.0.1\n ports 8080\n server_name a\n{}\n route / {{\n root www/main\n{}\n }}\n}}\n",
            extra_server_lines, route_lines
        )
    }

    #[test]
    fn a_good_file_gives_servers_with_defaults() {
        let (servers, problems) = load_text(GOOD).unwrap();
        assert!(problems.is_empty(), "{:?}", problems);
        assert_eq!(servers.len(), 2);

        let first = &servers[0];
        assert_eq!(first.host, "0.0.0.0");
        assert_eq!(first.ports, vec![8080, 8081]);
        assert_eq!(first.names, vec!["localhost", "www.localhost"]);
        assert_eq!(first.max_body, 500);
        assert_eq!(first.error_pages[&404], "www/errors/404.html");
        assert_eq!(first.routes.len(), 3);
        assert_eq!(first.routes[0].methods, vec!["GET", "POST"]);
        assert_eq!(first.routes[0].index.as_deref(), Some("index.html"));
        assert!(first.routes[0].listing);
        assert_eq!(first.routes[1].redirect.as_deref(), Some("/"));
        assert_eq!(first.routes[2].path, "/cgi", "trailing slash is dropped");
        assert_eq!(first.routes[2].cgi[".py"], "python3");
        assert_eq!(first.routes[2].methods, vec!["GET"], "GET is the default method");

        let second = &servers[1];
        assert_eq!(second.max_body, DEFAULT_BODY_LIMIT);
        assert!(!second.routes[0].listing);
    }

    #[test]
    fn same_port_twice_in_one_server_is_refused() {
        let text = "server {\n host 127.0.0.1\n ports 8080 8080\n server_name a\n route / {\n root www/main\n }\n}\n";
        assert!(first_problem(text).contains("port 8080 is written twice"));
        assert!(load_text(text).unwrap().0.is_empty());
    }

    #[test]
    fn same_host_port_and_name_in_two_servers_skips_the_second() {
        let block = "server {\n host 127.0.0.1\n ports 8080\n server_name a\n route / {\n root www/main\n }\n}\n";
        let text = format!("{}{}", block, block);
        let (servers, problems) = load_text(&text).unwrap();
        assert_eq!(servers.len(), 1);
        assert!(problems[0].contains("server #2 skipped"));
        assert!(problems[0].contains("already used"));
    }

    #[test]
    fn shared_port_with_different_names_is_fine() {
        let (servers, problems) = load_text(GOOD).unwrap();
        assert_eq!(servers.len(), 2);
        assert!(problems.is_empty());
        assert!(servers[0].ports.contains(&8080) && servers[1].ports.contains(&8080));
    }

    #[test]
    fn one_broken_server_does_not_stop_the_others() {
        let text = format!("{}server {{\n host 127.0.0.1\n ports 7000\n server_name b\n colour blue\n route / {{\n root www/main\n }}\n}}\n", GOOD);
        let (servers, problems) = load_text(&text).unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("server #3 skipped: unknown server setting 'colour'"));
    }

    #[test]
    fn invalid_paths_are_refused() {
        assert!(first_problem(&one_server("", "root www/does-not-exist")).contains("does not exist"));
        assert!(first_problem(&one_server("error_page 404 www/errors/nope.html", "")).contains("does not exist"));
        assert!(first_problem(&one_server("", "")).is_empty());
    }

    #[test]
    fn body_size_must_be_in_bounds() {
        assert!(first_problem(&one_server("client_max_body_size 0", "")).contains("client_max_body_size"));
        assert!(first_problem(&one_server("client_max_body_size 99999999999", "")).contains("client_max_body_size"));
        assert!(first_problem(&one_server("client_max_body_size -5", "")).contains("not a valid number"));
        assert!(first_problem(&one_server("client_max_body_size big", "")).contains("not a valid number"));
        assert!(first_problem(&one_server("client_max_body_size 1048576", "")).is_empty());
    }

    #[test]
    fn ports_and_hosts_are_validated() {
        let bad_port = "server {\n host 127.0.0.1\n ports 70000\n server_name a\n route / {\n root www/main\n }\n}\n";
        assert!(first_problem(bad_port).contains("not a valid number"));
        let zero = "server {\n host 127.0.0.1\n ports 0\n server_name a\n route / {\n root www/main\n }\n}\n";
        assert!(first_problem(zero).contains("port 0"));
        let no_port = "server {\n host 127.0.0.1\n server_name a\n route / {\n root www/main\n }\n}\n";
        assert!(first_problem(no_port).contains("'ports' is missing"));
        let bad_host = "server {\n host example.com\n ports 8080\n server_name a\n route / {\n root www/main\n }\n}\n";
        assert!(first_problem(bad_host).contains("not an address"));
        let no_name = "server {\n host 127.0.0.1\n ports 8080\n route / {\n root www/main\n }\n}\n";
        assert!(first_problem(no_name).contains("'server_name' is missing"));
    }

    #[test]
    fn routes_are_validated() {
        let no_root = "server {\n host 127.0.0.1\n ports 8080\n server_name a\n route /x {\n listing on\n }\n}\n";
        assert!(first_problem(no_root).contains("needs a 'root' or a 'redirect'"));
        let no_slash = "server {\n host 127.0.0.1\n ports 8080\n server_name a\n route files {\n root www/main\n }\n}\n";
        assert!(first_problem(no_slash).contains("must start with '/'"));
        let twice = "server {\n host 127.0.0.1\n ports 8080\n server_name a\n route / {\n root www/main\n }\n route / {\n root www/test\n }\n}\n";
        assert!(first_problem(twice).contains("written twice"));
        assert!(first_problem(&one_server("", "methods PATCH")).contains("unknown method"));
        assert!(first_problem(&one_server("", "listing maybe")).contains("on or off"));
        assert!(first_problem(&one_server("", "cgi py python3")).contains("'cgi' needs"));
        assert!(first_problem(&one_server("", "colour blue")).contains("unknown route setting"));
        let none = "server {\n host 127.0.0.1\n ports 8080\n server_name a\n}\n";
        assert!(first_problem(none).contains("no route"));
    }

    fn error_of(text: &str) -> String {
        match load_text(text) {
            Err(message) => message,
            Ok(_) => panic!("expected a broken file"),
        }
    }

    #[test]
    fn broken_file_structure_is_an_error() {
        let missing_brace = "server {\n host 127.0.0.1\n ports 8080\n server_name a\n route / {\n root www/main\n }\n";
        assert!(error_of(missing_brace).contains("missing"));
        assert!(error_of("hello {\n}\n").contains("expected 'server {'"));
        assert!(load_text("").unwrap().0.is_empty());
    }
}
