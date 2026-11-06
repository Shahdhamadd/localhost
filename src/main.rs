mod cgi;
mod config;
mod epoll;
mod files;
mod http;
mod listen;
mod router;
mod server;
mod sessions;

use std::env;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();
    let path = if args.len() > 1 {
        args[1].clone()
    } else {
        String::from("config/default.conf")
    };

    let servers = match config::load(&path) {
        Ok(servers) => servers,
        Err(problem) => {
            eprintln!("config error: {}", problem);
            process::exit(1);
        }
    };

    let listeners = listen::open_all(&servers);
    if listeners.is_empty() {
        eprintln!("no port could be opened");
        process::exit(1);
    }

    server::run(&servers, listeners);
}
