# requests in a cell (change `add-pip-packages`): one HTTP GET with the
# `requests` package (and `urllib3`, `idna`, `charset_normalizer`,
# `certifi`, as pip installs them) to the host's fake model server, given
# as `http://ADDR:PORT`, through the cell's sockets. Prints what it got:
# the status, the content type and the tool call of the JSON body.
import sys

import requests

reply = requests.get(sys.argv[1] + "/v1/models", timeout=30)
call = reply.json()["choices"][0]["message"]["tool_calls"][0]["function"]
print("requests", requests.__version__, "status", reply.status_code)
print("type", reply.headers["Content-Type"], "encoding", reply.apparent_encoding)
print("call", call["name"], call["arguments"])
