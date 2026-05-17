"""Minos Python sidecar wrapper.

usage:  python wrapper.py <user_script_path> <unix_socket_path>

The user script must define `filter(packet) -> dict | None`. See
docs/reference/sidecar-protocol.md for the packet schema and verdict
format.
"""
import base64
import json
import socket
import struct
import sys
import traceback


def _read_frame(sock):
    """Read one length-prefixed JSON frame from `sock`. Returns None on EOF."""
    hdr = _recv_exact(sock, 4)
    if hdr is None:
        return None
    (length,) = struct.unpack(">I", hdr)
    body = _recv_exact(sock, length)
    if body is None:
        return None
    return json.loads(body)


def _write_frame(sock, obj):
    body = json.dumps(obj).encode("utf-8")
    sock.sendall(struct.pack(">I", len(body)) + body)


def _recv_exact(sock, n):
    buf = bytearray()
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            return None
        buf.extend(chunk)
    return bytes(buf)


def _decode_packet(req):
    pkt = {
        "direction": req["direction"],
        "kind": req["kind"],
        "bytes": base64.b64decode(req["bytes_b64"]),
    }
    if req.get("http") is not None:
        http = req["http"]
        pkt["http"] = {
            "method": http["method"],
            "path": http["path"],
            "headers": http["headers"],
            "body": base64.b64decode(http["body_b64"]),
        }
    return pkt


def main():
    user_script_path, socket_path = sys.argv[1], sys.argv[2]
    with open(user_script_path, "rb") as f:
        user_source = f.read()
    namespace = {"__name__": "minos_user_filter"}
    exec(compile(user_source, user_script_path, "exec"), namespace)
    user_filter = namespace.get("filter")
    if not callable(user_filter):
        print("user script does not define filter(packet)", file=sys.stderr)
        sys.exit(2)

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(socket_path)

    while True:
        req = _read_frame(sock)
        if req is None:
            return
        req_id = req["id"]
        try:
            pkt = _decode_packet(req)
            verdict = user_filter(pkt)
        except Exception:  # noqa: BLE001 — fail-open on any user-script exception
            traceback.print_exc(file=sys.stderr)
            verdict = None
        if verdict is None:
            _write_frame(sock, {"verdict": "pass", "id": req_id})
        elif verdict.get("verdict") == "block":
            _write_frame(sock, {
                "verdict": "block",
                "id": req_id,
                "reason": verdict.get("reason", "blocked"),
            })
        else:
            _write_frame(sock, {"verdict": "pass", "id": req_id})


if __name__ == "__main__":
    main()
