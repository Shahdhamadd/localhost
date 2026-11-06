use crate::config::Server;
use std::io;
use std::net::TcpListener;

pub struct Listener {
    pub socket: TcpListener,
    pub host: String,
    pub port: u16,
}

pub fn open_all(servers: &[Server]) -> Vec<Listener> {
    let mut listeners: Vec<Listener> = Vec::new();

    for server in servers {
        for port in &server.ports {
            let already_open = listeners
                .iter()
                .any(|l| l.host == server.host && l.port == *port);
            if already_open {
                continue;
            }

            match open(&server.host, *port) {
                Ok(socket) => {
                    println!("listening on {}:{}", server.host, port);
                    listeners.push(Listener {
                        socket,
                        host: server.host.clone(),
                        port: *port,
                    });
                }
                Err(e) => eprintln!("cannot listen on {}:{}: {}", server.host, port, e),
            }
        }
    }
    listeners
}

fn open(host: &str, port: u16) -> io::Result<TcpListener> {
    let socket = TcpListener::bind((host, port))?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}
