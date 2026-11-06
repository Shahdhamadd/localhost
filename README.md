# localhost

An HTTP/1.1 web server written in Rust from scratch: one thread, non-blocking sockets and one `epoll` loop. The only crate is `libc`.

## Features

- GET, POST, DELETE
- several servers and ports, chosen by the `Host` header
- static files, index files, directory listing, redirects, custom error pages
- file uploads, downloads and deletes, with a body size limit
- chunked request bodies
- cookies and sessions
- CGI (Python and Perl)
- keep-alive and timeouts

## Run

`epoll` only exists on Linux, so the server runs in Docker.

```bash
docker compose up --build
```

Then open http://localhost:8080, http://localhost:8081 or http://localhost:9000.

To use another config file:

```bash
docker compose run --rm --service-ports localhost ./target/release/localhost config/default.conf
```

## Configuration

```
server {
    host 0.0.0.0
    ports 8080 8081
    server_name localhost
    client_max_body_size 1000000
    error_page 404 www/errors/404.html

    route / {
        root www/main
        methods GET POST DELETE
        index index.html
        listing off
    }

    route /old {
        redirect /
    }

    route /cgi {
        root www/main/cgi
        methods GET POST
        cgi .py python3
        cgi .pl perl
    }
}
```

| Setting | Meaning |
|---|---|
| `host` | address to listen on |
| `ports` | one or more ports |
| `server_name` | names matched against the `Host` header; the first server on a port is the default |
| `client_max_body_size` | biggest accepted body in bytes (default 1000000) |
| `error_page CODE FILE` | custom error page |
| `root` | folder the route serves |
| `methods` | allowed methods (default GET) |
| `index` | default file for a folder |
| `listing on/off` | directory listing |
| `redirect URL` | answer 301 to that URL |
| `cgi .EXT PROGRAM` | run files with that extension through the program |

Config mistakes are reported at startup. A broken server block is skipped and the other servers keep working. Examples are in `config/bad-*.conf`.

## Tests

Unit tests:

```bash
docker compose run --rm localhost cargo test
```

Integration tests, with the server running:

```bash
docker compose exec localhost python3 tests/test_server.py
```

Add `--slow` to also test timeouts, siege, memory and open connections.
