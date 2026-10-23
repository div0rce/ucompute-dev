"""Offline check of cdp.mjs against a SYNTHETIC DevTools endpoint (a stdlib HTTP + WebSocket stub on loopback): no
app, WebView2 or laptop is involved, so nothing here is evidence of the app. A key press the page refused must not
be printed as done.
Run: python3 -m unittest discover -s tests/distributed/two_machine -v
"""
import base64
import hashlib
import json
import os
import shutil
import socketserver
import struct
import subprocess
import threading
import unittest
from pathlib import Path

CDP = Path(__file__).with_name("cdp.mjs")


class Stub(socketserver.StreamRequestHandler):
    """/json/list names one page; its WebSocket answers every call, with an error for the methods in `fail`."""

    def handle(self):
        head = b""
        while not head.endswith(b"\r\n\r\n"):
            head += self.rfile.read(1)
        lines = head.decode().split("\r\n")
        hdr = {k.lower(): v.strip() for k, _, v in (x.partition(":") for x in lines[1:] if x)}
        if "sec-websocket-key" not in hdr:
            port = self.server.server_address[1]
            body = json.dumps([{"type": "page", "webSocketDebuggerUrl": f"ws://127.0.0.1:{port}/devtools/page/1"}]).encode()
            self.wfile.write(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n"
                             b"Content-Length: %d\r\n\r\n%s" % (len(body), body))
            return
        accept = base64.b64encode(hashlib.sha1((hdr["sec-websocket-key"] + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest())
        self.wfile.write(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                         b"Sec-WebSocket-Accept: " + accept + b"\r\n\r\n")
        while len(two := self.rfile.read(2)) == 2:  # until the client goes away
            b1, b2 = two
            n = b2 & 0x7F
            n = struct.unpack(">H", self.rfile.read(2))[0] if n == 126 else struct.unpack(">Q", self.rfile.read(8))[0] if n == 127 else n
            mask = self.rfile.read(4)
            data = bytes(c ^ mask[i % 4] for i, c in enumerate(self.rfile.read(n)))
            if b1 & 0x0F == 8:  # close
                return
            msg = json.loads(data)
            out = {"id": msg["id"], **({"error": {"code": -32000, "message": "refused"}} if msg["method"] in self.server.fail
                                       else {"result": {"result": {"value": {"tag": "BUTTON"}}}})}
            raw = json.dumps(out).encode()
            self.wfile.write(bytes([0x81, len(raw)]) + raw if len(raw) < 126 else bytes([0x81, 126]) + struct.pack(">H", len(raw)) + raw)


@unittest.skipUnless(shutil.which("node"), "node is not installed")
class SyntheticCdpKeyTests(unittest.TestCase):
    def press(self, fail):
        srv = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Stub)
        srv.daemon_threads, srv.fail = True, fail
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        try:
            return subprocess.run(["node", str(CDP), "key", "Enter", "1", "0"], capture_output=True, text=True, timeout=30,
                                  env={**os.environ, "CDP_PORT": str(srv.server_address[1])})
        finally:
            srv.shutdown()
            srv.server_close()

    def test_a_press_the_page_got_is_printed(self):
        r = self.press(set())
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(json.loads(r.stdout)["tag"], "BUTTON")

    def test_a_refused_press_is_an_error_not_a_line(self):
        r = self.press({"Input.dispatchKeyEvent"})
        self.assertEqual((r.returncode, r.stdout), (1, ""))
        self.assertIn("Input.dispatchKeyEvent", r.stderr)


if __name__ == "__main__":
    unittest.main()
