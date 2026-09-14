#!/usr/bin/env python3
"""A deterministic local peer for the Framed TCP protocol demo harness."""

import argparse
import socket
import struct
import threading

INPUT = 0x01
RESIZE = 0x02
OUTPUT = 0x81


def read_exact(connection, length):
    chunks = bytearray()
    while len(chunks) < length:
        chunk = connection.recv(length - len(chunks))
        if not chunk:
            raise RuntimeError("peer closed before its complete frame")
        chunks.extend(chunk)
    return bytes(chunks)


def read_frame(connection):
    body_length = struct.unpack(">I", read_exact(connection, 4))[0]
    if not 1 <= body_length <= 32 * 1024:
        raise RuntimeError(f"invalid frame body length: {body_length}")
    body = read_exact(connection, body_length)
    return body[0], body[1:]


def write_split(connection, frame):
    # TCP writes intentionally cut through both the length and payload so the
    # Wasm parser must retain partial state across Core Resource Data events.
    for offset in (2, 5):
        if offset < len(frame):
            connection.sendall(frame[:offset])
            frame = frame[offset:]
    connection.sendall(frame)


def output_frame(payload):
    body = bytes([OUTPUT]) + payload
    return struct.pack(">I", len(body)) + body


def serve_connection(connection, sequence):
    with connection:
        frame_type, payload = read_frame(connection)
        if frame_type != INPUT:
            raise RuntimeError(f"connection {sequence}: expected input frame")
        expected = b"first input\n" if sequence == 1 else b"second input\n"
        if payload != expected:
            raise RuntimeError(f"connection {sequence}: unexpected input bytes")
        if sequence == 1:
            frame_type, payload = read_frame(connection)
            if frame_type != RESIZE or payload != struct.pack(">HH", 33, 101):
                raise RuntimeError("connection 1: resize did not retain big-endian rows and cols")
            write_split(connection, output_frame(b"\x1b[32mfirst: \xe4\xb8\x96\xe7\x95\x8c\x1b[0m\r\n"))
        else:
            write_split(connection, output_frame(b"\x1b[35msecond: \xe6\xad\xa3\xe5\xb8\xb8\x1b[0m\r\n"))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=0)
    args = parser.parse_args()
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", args.port))
    listener.listen(2)
    print(f'{{"port":{listener.getsockname()[1]}}}', flush=True)
    errors = []
    threads = []
    try:
        for sequence in (1, 2):
            connection, _ = listener.accept()
            thread = threading.Thread(
                target=_record,
                args=(errors, serve_connection, connection, sequence),
                daemon=False,
            )
            thread.start()
            threads.append(thread)
        for thread in threads:
            thread.join()
        if errors:
            raise errors[0]
    finally:
        listener.close()


def _record(errors, callback, *args):
    try:
        callback(*args)
    except Exception as error:
        errors.append(error)


if __name__ == "__main__":
    main()
