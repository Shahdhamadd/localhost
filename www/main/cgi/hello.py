import os

print("Content-Type: text/html")
print()
print("<h1>Hello from Python CGI</h1>")
print("<p>Method: " + os.environ.get("REQUEST_METHOD", "") + "</p>")
print("<p>Query: " + os.environ.get("QUERY_STRING", "") + "</p>")
print("<p>PATH_INFO: " + os.environ.get("PATH_INFO", "") + "</p>")
print("<p>Running in folder: " + os.getcwd() + "</p>")
print('<a href="/">Back home</a>')
