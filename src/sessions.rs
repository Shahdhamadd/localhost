use crate::http::{Request, Response};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::time::{Duration, Instant};

pub struct Session {
    pub visits: u32,
    pub last_seen: Instant,
}

pub type Sessions = HashMap<String, Session>;

pub const LIFETIME: Duration = Duration::from_secs(3600);
pub const MAX_SESSIONS: usize = 10_000;

pub fn handle(sessions: &mut Sessions, request: &Request) -> Response {
    let mut id = request.cookie("session_id").to_string();

    if request.query == "logout" {
        sessions.remove(&id);
        let html = "<p>Logged out, your session was deleted.</p><a href=\"/session\">Start again</a>";
        let mut response = Response::new(200, "text/html", html.as_bytes().to_vec());
        response.add_header("Set-Cookie", "session_id=; Path=/; Max-Age=0");
        return response;
    }

    let is_new = !sessions.contains_key(&id);
    if is_new {
        id = new_id();
        make_room(sessions);
    }
    let visits = sessions.get(&id).map(|s| s.visits).unwrap_or(0) + 1;
    sessions.insert(id.clone(), Session { visits, last_seen: Instant::now() });

    let html = format!(
        "<h1>Session</h1><p>Your session id: {}</p><p>You visited this page {} times.</p>\
         <a href=\"/session\">Visit again</a> · <a href=\"/session?logout\">Log out</a>",
        id, visits
    );
    let mut response = Response::new(200, "text/html", html.into_bytes());
    if is_new {
        response.add_header(
            "Set-Cookie",
            &format!("session_id={}; Path=/; HttpOnly; SameSite=Lax", id),
        );
    }
    response
}

pub fn sweep(sessions: &mut Sessions) {
    sessions.retain(|_, session| session.last_seen.elapsed() < LIFETIME);
}

fn make_room(sessions: &mut Sessions) {
    while sessions.len() >= MAX_SESSIONS {
        let oldest = sessions
            .iter()
            .min_by_key(|(_, session)| session.last_seen)
            .map(|(id, _)| id.clone());
        match oldest {
            Some(id) => sessions.remove(&id),
            None => return,
        };
    }
}

fn new_id() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut file) = File::open("/dev/urandom") {
        let _ = file.read_exact(&mut bytes);
    }
    let mut id = String::new();
    for byte in bytes {
        id.push_str(&format!("{:02x}", byte));
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{self, Parse};

    fn request(raw: &[u8]) -> Request {
        match http::parse(raw) {
            Parse::Done(request, _) => request,
            _ => panic!("test request does not parse"),
        }
    }

    #[test]
    fn first_visit_sets_cookie_and_later_visits_count() {
        let mut sessions = Sessions::new();
        let first = handle(&mut sessions, &request(b"GET /session HTTP/1.1\r\nHost: a\r\n\r\n"));
        let cookie = first.header("set-cookie").to_string();
        assert!(cookie.starts_with("session_id="));
        assert!(cookie.contains("HttpOnly"));
        let pair = cookie.split(';').next().unwrap().to_string();

        let raw = format!("GET /session HTTP/1.1\r\nHost: a\r\nCookie: {}\r\n\r\n", pair);
        let second = handle(&mut sessions, &request(raw.as_bytes()));
        assert!(String::from_utf8_lossy(&second.body).contains("2 times"));
        assert_eq!(second.header("set-cookie"), "");

        let raw = format!("GET /session?logout HTTP/1.1\r\nHost: a\r\nCookie: {}\r\n\r\n", pair);
        let out = handle(&mut sessions, &request(raw.as_bytes()));
        assert!(out.header("set-cookie").contains("Max-Age=0"));
        assert!(sessions.is_empty());
    }

    #[test]
    fn unknown_cookie_starts_a_fresh_session() {
        let mut sessions = Sessions::new();
        let response = handle(&mut sessions, &request(b"GET /session HTTP/1.1\r\nHost: a\r\nCookie: session_id=forged\r\n\r\n"));
        assert!(response.header("set-cookie").starts_with("session_id="));
        assert!(!sessions.contains_key("forged"));
    }

    #[test]
    fn old_sessions_are_swept_and_count_is_capped() {
        let mut sessions = Sessions::new();
        if let Some(old) = Instant::now().checked_sub(LIFETIME + Duration::from_secs(1)) {
            sessions.insert("old".into(), Session { visits: 1, last_seen: old });
        }
        sessions.insert("fresh".into(), Session { visits: 1, last_seen: Instant::now() });
        sweep(&mut sessions);
        assert!(!sessions.contains_key("old"));
        assert!(sessions.contains_key("fresh"));

        for i in 0..MAX_SESSIONS {
            sessions.insert(format!("s{}", i), Session { visits: 1, last_seen: Instant::now() });
        }
        handle(&mut sessions, &request(b"GET /session HTTP/1.1\r\nHost: a\r\n\r\n"));
        assert!(sessions.len() <= MAX_SESSIONS);
    }
}
