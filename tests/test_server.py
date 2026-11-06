import os
import socket
import subprocess
import sys
import time

HOST = "127.0.0.1"
SERVER = "./target/release/localhost"
passed = 0
failed = 0


def check(name, ok):
    global passed, failed
    if ok:
        passed += 1
        print("  PASS  " + name)
    else:
        failed += 1
        print("  FAIL  " + name)


def split_response(data):
    head, _, body = data.partition(b"\r\n\r\n")
    lines = head.decode(errors="replace").split("\r\n")
    words = lines[0].split(" ")
    status = int(words[1]) if len(words) > 1 and words[1].isdigit() else 0
    headers = {}
    for line in lines[1:]:
        name, _, value = line.partition(":")
        headers[name.strip().lower()] = value.strip()
    return status, headers, body


def send(raw, port=8080, timeout=15):
    s = socket.create_connection((HOST, port), timeout=timeout)
    s.sendall(raw)
    data = b""
    while True:
        chunk = s.recv(65536)
        if not chunk:
            break
        data += chunk
    s.close()
    return split_response(data)


def request(method, path, port=8080, host="localhost", headers=None, body=b""):
    headers = headers or {}
    lines = [method + " " + path + " HTTP/1.1", "Host: " + host, "Connection: close"]
    for name, value in headers.items():
        lines.append(name + ": " + value)
    if body and "Transfer-Encoding" not in headers:
        lines.append("Content-Length: " + str(len(body)))
    raw = ("\r\n".join(lines) + "\r\n\r\n").encode() + body
    return send(raw, port)


def chunked(data, size=7):
    out = b""
    for start in range(0, len(data), size):
        piece = data[start:start + size]
        out += format(len(piece), "x").encode() + b"\r\n" + piece + b"\r\n"
    return out + b"0\r\n\r\n"


def test_static():
    print("Static files")
    status, headers, body = request("GET", "/")
    check("GET / gives 200 and the index page", status == 200 and b"Welcome" in body)
    check("index page is text/html", headers.get("content-type", "").startswith("text/html"))
    status, headers, _ = request("GET", "/style.css")
    check("GET /style.css is text/css", status == 200 and headers.get("content-type", "").startswith("text/css"))
    status, headers, body = request("GET", "/logo.svg")
    with open("www/main/logo.svg", "rb") as f:
        check("GET /logo.svg is the exact file", status == 200 and body == f.read())
    status, headers, _ = request("GET", "/files")
    check("folder without / gives 301 to /files/", status == 301 and headers.get("location") == "/files/")
    status, _, body = request("GET", "/files/")
    check("directory listing shows hello.txt", status == 200 and b"hello.txt" in body)
    status, _, _ = request("GET", "/cgi/")
    check("folder with listing off gives 403", status == 403)
    status, _, body = request("GET", "/nothing-here")
    check("missing page gives custom 404", status == 404 and b"does not exist" in body)


def test_servers_and_ports():
    print("Servers, ports and host names")
    status, _, body = request("GET", "/", port=8081)
    check("port 8081 serves the localhost site", status == 200 and b"Welcome" in body)
    status, _, body = request("GET", "/", host="test.com")
    check("Host test.com on 8080 gives the test site", status == 200 and b"Test site" in body)
    status, _, body = request("GET", "/", host="unknown.com")
    check("unknown Host gives the default (first) server", status == 200 and b"Welcome" in body)
    status, _, body = request("GET", "/", port=9000)
    check("port 9000 gives the admin site", status == 200 and b"Admin site" in body)
    status, _, body = request("GET", "/nothing", host="test.com")
    check("server without custom pages uses the built-in 404", status == 404 and b"404 Not Found" in body)


def test_methods_and_redirects():
    print("Methods and redirects")
    status, headers, _ = request("DELETE", "/files/hello.txt")
    check("DELETE on a GET-only route gives 405 + Allow", status == 405 and headers.get("allow") == "GET")
    status, _, _ = request("PUT", "/")
    check("unknown method gives 405", status == 405)
    status, headers, _ = request("GET", "/old")
    check("redirect route gives 301 to /", status == 301 and headers.get("location") == "/")


def test_bad_requests():
    print("Wrong requests")
    status, _, _ = send(b"HELLO\r\n\r\n")
    check("nonsense first line gives 400", status == 400)
    status, _, _ = send(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n")
    check("missing Host gives 400", status == 400)
    status, _, _ = send(b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: abc\r\n\r\n")
    check("bad Content-Length gives 400", status == 400)
    status, _, _ = send(b"GET /../secret HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n")
    check("path with .. gives 403", status == 403)
    status, _, _ = request("GET", "/")
    check("server still works after wrong requests", status == 200)


def test_body_limit():
    print("Body size limit")
    status, _, _ = request("POST", "/note.txt", host="test.com", body=b"short")
    check("body under the limit (10) is accepted", status == 201)
    status, _, _ = request("POST", "/note.txt", host="test.com", body=b"BODY with something longer than limit")
    check("body over the limit gives 413", status == 413)
    status, _, _ = request("POST", "/uploads/big.bin", body=os.urandom(2000000))
    check("2 MB on a 1 MB server gives 413", status == 413)


def test_upload_and_delete():
    print("Upload, download, delete")
    data = os.urandom(300000)
    status, _, _ = request("POST", "/uploads/test.bin", body=data)
    check("raw upload gives 201", status == 201)
    status, _, body = request("GET", "/uploads/test.bin")
    check("downloaded file is identical", status == 200 and body == data)
    form = (b"------b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"form.txt\"\r\n"
            b"Content-Type: text/plain\r\n\r\nfrom a form\r\n------b--\r\n")
    status, _, _ = request("POST", "/uploads/", headers={"Content-Type": "multipart/form-data; boundary=----b"}, body=form)
    _, _, body = request("GET", "/uploads/form.txt")
    check("form (multipart) upload is saved correctly", status == 201 and body == b"from a form")
    status, _, _ = request("POST", "/uploads/chunked.txt", headers={"Transfer-Encoding": "chunked"}, body=chunked(b"sent in many small chunks"))
    _, _, body = request("GET", "/uploads/chunked.txt")
    check("chunked upload is joined correctly", status == 201 and body == b"sent in many small chunks")
    status, _, _ = request("DELETE", "/uploads/test.bin")
    check("DELETE gives 200", status == 200)
    status, _, _ = request("DELETE", "/uploads/test.bin")
    check("DELETE again gives 404", status == 404)
    request("DELETE", "/uploads/form.txt")
    request("DELETE", "/uploads/chunked.txt")


def test_cookies():
    print("Cookies and sessions")
    status, headers, body = request("GET", "/session")
    cookie = headers.get("set-cookie", "")
    check("first visit gets a session cookie", status == 200 and cookie.startswith("session_id="))
    pair = cookie.split(";")[0]
    _, headers, body = request("GET", "/session", headers={"Cookie": pair})
    check("second visit with the cookie counts 2", b"2 times" in body and "set-cookie" not in headers)
    _, headers, _ = request("GET", "/session?logout", headers={"Cookie": pair})
    check("logout expires the cookie", "Max-Age=0" in headers.get("set-cookie", ""))


def test_cgi():
    print("CGI")
    status, _, body = request("GET", "/cgi/hello.py?name=shahd")
    check("Python CGI with a query", status == 200 and b"Query: name=shahd" in body)
    check("CGI runs in its own folder", b"Running in folder: " in body and b"/www/main/cgi" in body)
    status, _, body = request("POST", "/cgi/echo.py", body=b"name=shahd&age=25")
    check("CGI gets an unchunked body", status == 200 and b"received 17 bytes" in body)
    status, _, body = request("POST", "/cgi/echo.py", headers={"Transfer-Encoding": "chunked"}, body=chunked(b"this body was sent in chunks"))
    check("CGI gets a chunked body", status == 200 and b"received 28 bytes" in body)
    status, _, body = request("GET", "/cgi/hello.pl")
    check("Perl CGI (second CGI)", status == 200 and b"Perl" in body)
    status, _, _ = request("GET", "/cgi/broken.py")
    check("crashing script gives 500", status == 500)
    status, _, _ = request("GET", "/cgi/nope.py")
    check("missing script gives 404", status == 404)

    slow = socket.create_connection((HOST, 8080), timeout=15)
    slow.sendall(b"GET /cgi/slow.py HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
    started = time.time()
    status, _, _ = request("GET", "/about.html")
    check("a slow script does not block other guests", status == 200 and time.time() - started < 1)
    data = b""
    while True:
        chunk = slow.recv(65536)
        if not chunk:
            break
        data += chunk
    check("the slow script still answers", b"Done after 3 seconds" in data)


def read_one_response(s):
    data = b""
    while b"\r\n\r\n" not in data:
        data += s.recv(65536)
    head, _, body = data.partition(b"\r\n\r\n")
    status, headers, _ = split_response(head + b"\r\n\r\n")
    length = int(headers.get("content-length", "0"))
    while len(body) < length:
        body += s.recv(65536)
    return status, headers, body


def test_keep_alive():
    print("Keep-alive")
    s = socket.create_connection((HOST, 8080), timeout=15)
    s.sendall(b"GET /about.html HTTP/1.1\r\nHost: localhost\r\n\r\n")
    status1, headers, _ = read_one_response(s)
    s.sendall(b"GET /style.css HTTP/1.1\r\nHost: localhost\r\n\r\n")
    status2, _, _ = read_one_response(s)
    s.close()
    check("two requests on one connection", status1 == 200 and status2 == 200)
    check("server says Connection: keep-alive", headers.get("connection") == "keep-alive")
    check("responses carry a Date header", headers.get("date", "").endswith(" GMT"))

    s = socket.create_connection((HOST, 8080), timeout=15)
    s.sendall(b"GET /about.html HTTP/1.1\r\nHost: localhost\r\n\r\n"
              b"GET /style.css HTTP/1.1\r\nHost: localhost\r\n\r\n"
              b"GET /nothing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
    data = b""
    while True:
        chunk = s.recv(65536)
        if not chunk:
            break
        data += chunk
    s.close()
    statuses = []
    while data:
        status, headers, rest = split_response(data)
        statuses.append(status)
        data = rest[int(headers.get("content-length", "0")):]
    check("three pipelined requests in one packet are all answered", statuses == [200, 200, 404])


def test_encoded_paths():
    print("Percent-encoded paths")
    with open("www/main/files/hello world.txt", "wb") as f:
        f.write(b"spaces are fine")
    status, _, body = request("GET", "/files/hello%20world.txt")
    check("GET /files/hello%20world.txt finds 'hello world.txt'", status == 200 and body == b"spaces are fine")
    status, _, body = request("GET", "/files/")
    check("the listing links to the encoded name", status == 200 and b'href="hello%20world.txt"' in body)
    os.remove("www/main/files/hello world.txt")
    status, _, _ = request("GET", "/files/%2e%2e/index.html")
    check("encoded .. is refused with 403", status == 403)
    status, _, _ = send(b"GET /%zz HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n")
    check("broken %-sequence gives 400", status == 400)


def test_config_files():
    print("Configuration files")
    for name, expected in [
        ("bad-syntax.conf", "a '}' is missing"),
        ("bad-same-port.conf", "port 8080 is written twice"),
        ("bad-conflict.conf", "server #2 skipped"),
        ("bad-one-broken.conf", "unknown server setting 'colour'"),
    ]:
        try:
            result = subprocess.run([SERVER, "config/" + name], capture_output=True, text=True, timeout=2)
            output = result.stdout + result.stderr
        except subprocess.TimeoutExpired as e:
            output = (e.stdout or b"").decode() + (e.stderr or b"").decode()
        check(name + " is detected", expected in output)


def server_memory():
    with open("/proc/1/status") as f:
        for line in f:
            if line.startswith("VmRSS"):
                return int(line.split()[1])
    return 0


def test_slow():
    print("Timeouts (about 25 seconds)")
    s = socket.create_connection((HOST, 8080), timeout=30)
    s.sendall(b"GET / HTTP/1.1\r\nHo")
    status, _, _ = split_response(s.recv(1000))
    check("half a request gets 408 after the timeout", status == 408)
    status, _, _ = request("GET", "/cgi/forever.py")
    check("a never-ending script gets 504", status == 504)

    print("Siege, memory and open connections (about 30 seconds)")
    memory_before = server_memory()
    result = subprocess.run(["siege", "-b", "-t", "20s", "http://127.0.0.1:8080/"], capture_output=True, text=True)
    output = result.stdout + result.stderr
    availability = 0.0
    for line in output.splitlines():
        if "availability" in line.lower():
            availability = float(line.split(":")[1].strip().strip(","))
    check("siege availability is at least 99.5% (" + str(availability) + ")", availability >= 99.5)
    time.sleep(2)
    memory_after = server_memory()
    check("memory did not grow (" + str(memory_before) + " kB -> " + str(memory_after) + " kB)", memory_after <= memory_before + 512)
    open_fds = len(os.listdir("/proc/1/fd"))
    check("no hanging connections (" + str(open_fds) + " open fds)", open_fds <= 7)


def main():
    test_static()
    test_servers_and_ports()
    test_methods_and_redirects()
    test_bad_requests()
    test_body_limit()
    test_upload_and_delete()
    test_cookies()
    test_cgi()
    test_keep_alive()
    test_encoded_paths()
    test_config_files()
    if "--slow" in sys.argv:
        test_slow()
    print()
    print(str(passed) + " passed, " + str(failed) + " failed")
    sys.exit(1 if failed else 0)


main()
