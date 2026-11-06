import sys

body = sys.stdin.read()
print("Content-Type: text/plain")
print()
print("The script received " + str(len(body)) + " bytes:")
print(body)
