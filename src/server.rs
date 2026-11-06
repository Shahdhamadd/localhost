use crate::cgi::{self, Job};
use crate::config::Server;
use crate::epoll;
use crate::http::{self, Head, Parse, Response};
use crate::listen::Listener;
use crate::router::{self, Answer};
use crate::sessions::{self, Sessions};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_LIMIT: Duration = Duration::from_secs(30);
const HEAD_LIMIT: usize = 65536;
const SWEEP_EVERY: Duration = Duration::from_secs(60);

struct Connection {
    stream: TcpStream,
    host: String,
    port: u16,
    input: Vec<u8>,
    output: Vec<u8>,
    written: usize,
    keep_alive: bool,
    draining: bool,
    last_active: Instant,
    started: Option<Instant>,
    cgi: Option<Job>,
}

type Clients = HashMap<i32, Connection>;
type Pipes = HashMap<i32, i32>;

pub fn run(servers: &[Server], listeners: Vec<Listener>) {
    let epoll = match epoll::create() {
        Ok(epoll) => epoll,
        Err(e) => {
            eprintln!("cannot create epoll: {}", e);
            return;
        }
    };

    for listener in &listeners {
        let _ = epoll::add(epoll, listener.socket.as_raw_fd(), epoll::READ);
    }

    let mut clients: Clients = HashMap::new();
    let mut pipes: Pipes = HashMap::new();
    let mut sessions: Sessions = HashMap::new();
    let mut events = vec![libc::epoll_event { events: 0, u64: 0 }; 64];
    let mut last_sweep = Instant::now();

    loop {
        let count = epoll::wait(epoll, &mut events);

        for event in &events[..count] {
            let fd = event.u64 as i32;

            if let Some(listener) = listeners.iter().find(|l| l.socket.as_raw_fd() == fd) {
                accept_client(epoll, listener, &mut clients, &mut pipes);
            } else if let Some(&client) = pipes.get(&fd) {
                cgi_step(epoll, fd, client, &mut clients, &mut pipes, servers);
            } else {
                handle_client(epoll, fd, &mut clients, &mut pipes, servers, &mut sessions);
            }
        }

        check_timeouts(epoll, &mut clients, &mut pipes);
        if last_sweep.elapsed() > SWEEP_EVERY {
            sessions::sweep(&mut sessions);
            last_sweep = Instant::now();
        }
    }
}

fn accept_client(epoll: i32, listener: &Listener, clients: &mut Clients, pipes: &mut Pipes) {
    let stream = match listener.socket.accept() {
        Ok((stream, _)) => stream,
        Err(e) if e.kind() == ErrorKind::WouldBlock => return,
        Err(e) => {
            eprintln!("accept failed: {}", e);
            evict_idle(epoll, clients, pipes);
            return;
        }
    };
    if stream.set_nonblocking(true).is_err() {
        return;
    }
    let fd = stream.as_raw_fd();
    if epoll::add(epoll, fd, epoll::READ).is_ok() {
        let connection = Connection {
            stream,
            host: listener.host.clone(),
            port: listener.port,
            input: Vec::new(),
            output: Vec::new(),
            written: 0,
            keep_alive: false,
            draining: false,
            last_active: Instant::now(),
            started: None,
            cgi: None,
        };
        clients.insert(fd, connection);
    }
}

fn evict_idle(epoll: i32, clients: &mut Clients, pipes: &mut Pipes) {
    let oldest = clients
        .iter()
        .filter(|(_, c)| c.input.is_empty() && c.output.is_empty() && c.cgi.is_none())
        .min_by_key(|(_, c)| c.last_active)
        .map(|(fd, _)| *fd);
    if let Some(fd) = oldest {
        println!("client {} evicted to make room", fd);
        close_client(epoll, fd, clients, pipes);
    }
}

fn handle_client(
    epoll: i32,
    fd: i32,
    clients: &mut Clients,
    pipes: &mut Pipes,
    servers: &[Server],
    sessions: &mut Sessions,
) {
    let connection = match clients.get_mut(&fd) {
        Some(connection) => connection,
        None => return,
    };

    let keep = if connection.draining {
        drain_step(connection)
    } else if connection.output.is_empty() {
        read_step(epoll, fd, connection, pipes, servers, sessions)
    } else {
        let keep = write_step(epoll, fd, connection);
        let next_request_waiting = keep
            && !connection.draining
            && connection.output.is_empty()
            && !connection.input.is_empty();
        if next_request_waiting {
            process_input(epoll, fd, connection, pipes, servers, sessions)
        } else {
            keep
        }
    };

    if !keep {
        close_client(epoll, fd, clients, pipes);
    }
}

fn read_step(
    epoll: i32,
    fd: i32,
    connection: &mut Connection,
    pipes: &mut Pipes,
    servers: &[Server],
    sessions: &mut Sessions,
) -> bool {
    let mut buffer = [0u8; 4096];
    match connection.stream.read(&mut buffer) {
        Ok(0) => return false,
        Ok(n) => connection.input.extend_from_slice(&buffer[..n]),
        Err(e) if e.kind() == ErrorKind::WouldBlock => return true,
        Err(_) => return false,
    }
    connection.last_active = Instant::now();
    process_input(epoll, fd, connection, pipes, servers, sessions)
}

fn process_input(
    epoll: i32,
    fd: i32,
    connection: &mut Connection,
    pipes: &mut Pipes,
    servers: &[Server],
    sessions: &mut Sessions,
) -> bool {
    if connection.started.is_none() {
        connection.started = Some(Instant::now());
    }

    let response = match http::parse(&connection.input) {
        Parse::Incomplete => {
            if !too_big(servers, connection) {
                return true;
            }
            connection.keep_alive = false;
            router::error_for_door(servers, &connection.host, connection.port, 413)
        }
        Parse::Bad => {
            connection.keep_alive = false;
            router::error_for_door(servers, &connection.host, connection.port, 400)
        }
        Parse::Done(request, used) => {
            connection.input.drain(..used);
            connection.keep_alive = request.keep_alive();
            match router::handle(servers, sessions, &connection.host, connection.port, &request) {
                Answer::Now(response) => {
                    println!("{} {}?{} -> {}", request.method, request.target, request.query, response.status);
                    response
                }
                Answer::Cgi(job) => {
                    println!("{} {}?{} -> CGI started", request.method, request.target, request.query);
                    return wait_for_cgi(epoll, fd, connection, pipes, job);
                }
            }
        }
    };
    send(connection, response);
    epoll::change(epoll, fd, epoll::WRITE).is_ok()
}

fn too_big(servers: &[Server], connection: &Connection) -> bool {
    let (request, body_start) = match http::parse_head(&connection.input) {
        Head::Done(request, body_start) => (request, body_start),
        Head::Incomplete => return connection.input.len() > HEAD_LIMIT,
        Head::Bad => return false,
    };
    let server = router::server_for(servers, &connection.host, connection.port, request.header("host"));
    if let Ok(Some(length)) = request.content_length() {
        if length > server.max_body {
            return true;
        }
    }
    connection.input.len() > body_start + server.max_body + HEAD_LIMIT
}

fn send(connection: &mut Connection, response: Response) {
    connection.output = response.to_bytes(connection.keep_alive);
    connection.written = 0;
    connection.started = None;
    connection.last_active = Instant::now();
}

fn wait_for_cgi(epoll: i32, fd: i32, connection: &mut Connection, pipes: &mut Pipes, job: Job) -> bool {
    let pipe = job.stdout.as_raw_fd();
    connection.cgi = Some(job);
    if epoll::add(epoll, pipe, epoll::READ).is_err() {
        return false;
    }
    pipes.insert(pipe, fd);
    epoll::change(epoll, fd, 0).is_ok()
}

fn cgi_step(epoll: i32, pipe: i32, fd: i32, clients: &mut Clients, pipes: &mut Pipes, servers: &[Server]) {
    let connection = match clients.get_mut(&fd) {
        Some(connection) => connection,
        None => return,
    };
    let job = match connection.cgi.as_mut() {
        Some(job) => job,
        None => return,
    };
    let result = match cgi::read_step(job) {
        Some(result) => result,
        None => return,
    };
    let response = match result {
        Ok(response) => response,
        Err(status) => router::error_for_door(servers, &connection.host, connection.port, status),
    };

    println!("CGI finished -> {}", response.status);
    epoll::remove(epoll, pipe);
    pipes.remove(&pipe);
    connection.cgi = None;
    send(connection, response);
    if epoll::change(epoll, fd, epoll::WRITE).is_err() {
        close_client(epoll, fd, clients, pipes);
    }
}

fn write_step(epoll: i32, fd: i32, connection: &mut Connection) -> bool {
    let rest = &connection.output[connection.written..];
    match connection.stream.write(rest) {
        Ok(0) => return false,
        Ok(n) => connection.written += n,
        Err(e) if e.kind() == ErrorKind::WouldBlock => return true,
        Err(_) => return false,
    }
    connection.last_active = Instant::now();

    if connection.written < connection.output.len() {
        return true;
    }
    if !connection.keep_alive {
        let _ = connection.stream.shutdown(Shutdown::Write);
        connection.draining = true;
        return epoll::change(epoll, fd, epoll::READ).is_ok();
    }
    connection.output.clear();
    connection.written = 0;
    epoll::change(epoll, fd, epoll::READ).is_ok()
}

fn drain_step(connection: &mut Connection) -> bool {
    let mut buffer = [0u8; 4096];
    match connection.stream.read(&mut buffer) {
        Ok(0) => false,
        Ok(_) => true,
        Err(e) if e.kind() == ErrorKind::WouldBlock => true,
        Err(_) => false,
    }
}

fn check_timeouts(epoll: i32, clients: &mut Clients, pipes: &mut Pipes) {
    let mut too_slow = Vec::new();
    for (fd, connection) in clients.iter() {
        let idle = connection.last_active.elapsed() > TIMEOUT;
        let too_long = connection.started.is_some_and(|started| started.elapsed() > REQUEST_LIMIT);
        if idle || too_long {
            too_slow.push(*fd);
        }
    }
    for fd in too_slow {
        timeout_client(epoll, fd, clients, pipes);
    }
}

fn timeout_client(epoll: i32, fd: i32, clients: &mut Clients, pipes: &mut Pipes) {
    let connection = match clients.get_mut(&fd) {
        Some(connection) => connection,
        None => return,
    };

    let status = if connection.cgi.is_some() {
        504
    } else if connection.started.is_some() && connection.output.is_empty() {
        408
    } else {
        0
    };
    if status == 0 {
        close_client(epoll, fd, clients, pipes);
        return;
    }

    println!("client {} timed out -> {}", fd, status);
    stop_cgi(epoll, connection, pipes);
    connection.keep_alive = false;
    send(connection, Response::default_error(status));
    if epoll::change(epoll, fd, epoll::WRITE).is_err() {
        close_client(epoll, fd, clients, pipes);
    }
}

fn close_client(epoll: i32, fd: i32, clients: &mut Clients, pipes: &mut Pipes) {
    epoll::remove(epoll, fd);
    if let Some(mut connection) = clients.remove(&fd) {
        stop_cgi(epoll, &mut connection, pipes);
    }
}

fn stop_cgi(epoll: i32, connection: &mut Connection, pipes: &mut Pipes) {
    if let Some(mut job) = connection.cgi.take() {
        let pipe = job.stdout.as_raw_fd();
        epoll::remove(epoll, pipe);
        pipes.remove(&pipe);
        let _ = job.child.kill();
        let _ = job.child.wait();
    }
}
