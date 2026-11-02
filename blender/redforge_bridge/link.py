"""The connection with RedForge: one JSON object per line, over TCP on this computer (RedForge.WinUI/BlenderBridge.cs).

Commands are one short connection each; a listener thread keeps another one open for RedForge's events, which
Blender's main thread picks up from `events`.
"""

import json
import queue
import socket
import threading

HOST = "127.0.0.1"
events = queue.Queue()
_listener = None


class LinkError(Exception):
    pass


def request(port: int, cmd: str, timeout: float = 600, **args) -> dict:
    """Send a command to RedForge and wait for its reply."""
    try:
        with socket.create_connection((HOST, port), timeout=3) as sock:
            sock.settimeout(timeout)
            sock.sendall((json.dumps({"cmd": cmd, **args}) + "\n").encode("utf-8"))
            line = sock.makefile("r", encoding="utf-8").readline()
    except OSError as ex:
        raise LinkError("RedForge isn't running, or its bridge port is different") from ex
    if not line:
        raise LinkError("RedForge closed the connection")
    reply = json.loads(line)
    if not reply.get("ok"):
        raise LinkError(reply.get("error") or "RedForge couldn't do it")
    return reply


def listening():
    """The port the listener uses, None when it isn't running."""
    return None if _listener is None else _listener.port


def connected() -> bool:
    return (_listener is not None) and _listener.connected


def start(port: int):
    global _listener
    stop()
    _listener = _Listener(port)
    _listener.start()


def stop():
    global _listener
    if _listener is not None:
        _listener.stop()
        _listener = None


class _Listener(threading.Thread):
    def __init__(self, port: int):
        super().__init__(name="RedForge link", daemon=True)
        self.port = port
        self.connected = False
        self.stopping = threading.Event()
        self.sock = None

    def run(self):
        while not self.stopping.is_set():
            try:
                with socket.create_connection((HOST, self.port), timeout=3) as sock:
                    self.sock = sock
                    sock.settimeout(None)
                    sock.sendall(b'{"cmd": "listen"}\n')
                    lines = sock.makefile("r", encoding="utf-8")
                    lines.readline()  # (the reply to listen)
                    self.connected = True
                    for line in lines:
                        message = json.loads(line)
                        if "event" in message:
                            events.put(message)
            except (OSError, ValueError):
                pass
            finally:
                self.connected = False
                self.sock = None
            self.stopping.wait(2.0)  # (RedForge not running yet, or closed: try again)

    def stop(self):
        self.stopping.set()
        try:
            if self.sock is not None:
                self.sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
