print "Content-Type: text/html\n\n";
print "<h1>Hello from Perl CGI</h1>\n";
print "<p>Method: $ENV{REQUEST_METHOD}</p>\n";
print "<p>Query: $ENV{QUERY_STRING}</p>\n";
