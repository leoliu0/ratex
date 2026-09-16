#!/usr/bin/env python3
"""Bounded, streaming subprocess and file capture for repository harnesses.

The SHA-256 and byte count always describe the complete byte stream.  The
retained file contains the complete stream when it fits, otherwise a fixed
head and tail separated by an explicit omission marker.
"""

from __future__ import annotations

import hashlib
import os
import signal
import subprocess
import threading
import time
from collections import deque
from pathlib import Path
from typing import BinaryIO, Pattern, Sequence


DEFAULT_MAX_CAPTURE_BYTES = 1 << 20
DEFAULT_HEAD_BYTES = 128 << 10
READ_CHUNK_BYTES = 64 << 10
MAX_SCANNED_LINE_BYTES = 4096


class CaptureBuffer:
    """Consume arbitrary chunks while retaining bounded head/tail evidence."""

    def __init__(
        self,
        max_bytes: int = DEFAULT_MAX_CAPTURE_BYTES,
        *,
        head_bytes: int = DEFAULT_HEAD_BYTES,
        error_pattern: Pattern[str] | None = None,
        error_limit: int = 8,
    ) -> None:
        if max_bytes < 0:
            raise ValueError("max capture bytes must be non-negative")
        self.max_bytes = max_bytes
        # Keep the default 1:7 head/tail balance when a caller requests a
        # smaller cap; larger captures still need no more than a 128 KiB head.
        scaled_head = max(1, max_bytes // 8) if max_bytes else 0
        self.head_limit = min(head_bytes, scaled_head) if max_bytes else 0
        self.tail_limit = max_bytes - self.head_limit if max_bytes else 0
        self.error_pattern = error_pattern
        self.error_limit = error_limit
        self.total_bytes = 0
        self._hash = hashlib.sha256()
        self._head = bytearray()
        self._tail: deque[bytes] = deque()
        self._tail_size = 0
        self._all = bytearray() if max_bytes == 0 else None
        self._line = bytearray()
        self._skip_line = False
        self._errors: list[str] = []
        self._seen_errors: set[str] = set()

    def feed(self, data: bytes) -> None:
        if not data:
            return
        self.total_bytes += len(data)
        self._hash.update(data)
        self._scan_lines(data)
        if self._all is not None:
            self._all.extend(data)
            return
        need = self.head_limit - len(self._head)
        if need > 0:
            self._head.extend(data[:need])
            data = data[need:]
        if data and self.tail_limit:
            self._tail.append(bytes(data))
            self._tail_size += len(data)
            while self._tail and (
                self._tail_size - len(self._tail[0]) >= self.tail_limit
            ):
                self._tail_size -= len(self._tail.popleft())
            overflow = self._tail_size - self.tail_limit
            if overflow > 0:
                first = self._tail.popleft()
                self._tail.appendleft(first[overflow:])
                self._tail_size -= overflow

    def finish(self) -> None:
        if self._line and not self._skip_line:
            self._scan_line(bytes(self._line))
        self._line.clear()
        self._skip_line = False

    def _scan_lines(self, data: bytes) -> None:
        if self.error_pattern is None or len(self._errors) >= self.error_limit:
            return
        pos = 0
        while pos < len(data):
            newline = data.find(b"\n", pos)
            end = len(data) if newline < 0 else newline
            part = data[pos:end]
            if not self._skip_line:
                room = MAX_SCANNED_LINE_BYTES - len(self._line)
                self._line.extend(part[:room])
                if len(part) > room:
                    self._scan_line(bytes(self._line))
                    self._line.clear()
                    self._skip_line = True
            if newline < 0:
                return
            if not self._skip_line:
                self._scan_line(bytes(self._line))
            self._line.clear()
            self._skip_line = False
            pos = newline + 1

    def _scan_line(self, line: bytes) -> None:
        if self.error_pattern is None or len(self._errors) >= self.error_limit:
            return
        text = line.decode("utf-8", "replace").strip()
        if self.error_pattern.match(text):
            text = text[:160]
            if text not in self._seen_errors:
                self._seen_errors.add(text)
                self._errors.append(text)

    @property
    def truncated(self) -> bool:
        return self.max_bytes > 0 and self.total_bytes > self.max_bytes

    @property
    def retained_bytes(self) -> int:
        if self._all is not None:
            return len(self._all)
        return len(self._rendered()[0])

    @property
    def errors(self) -> list[str]:
        return list(self._errors)

    def bytes_for_file(self) -> bytes:
        return self._rendered()[0]

    def _rendered(self) -> tuple[bytes, int, int, int]:
        """Return file bytes plus raw head, raw tail and marker lengths."""
        if self._all is not None:
            data = bytes(self._all)
            return data, len(data), 0, 0
        tail_data = b"".join(self._tail)
        if not self.truncated:
            data = bytes(self._head) + tail_data
            return data, len(self._head), len(tail_data), 0
        marker = b""
        omitted = self.total_bytes - len(self._head) - self._tail_size
        tail = b""
        for _ in range(3):
            marker = (
                f"\n\n[... {omitted} output bytes omitted by bounded capture ...]\n\n"
            ).encode("ascii")
            tail_budget = max(0, self.max_bytes - len(self._head) - len(marker))
            tail = tail_data[-tail_budget:] if tail_budget else b""
            actual_omitted = self.total_bytes - len(self._head) - len(tail)
            if actual_omitted == omitted:
                break
            omitted = actual_omitted
        if len(self._head) + len(marker) > self.max_bytes:
            # Very small custom limits prioritize the exact stream head; the
            # structured metadata still records that truncation occurred.
            data = bytes(self._head[: self.max_bytes])
            return data, len(data), 0, 0
        data = bytes(self._head) + marker + tail
        return data, len(self._head), len(tail), len(marker)

    def metadata(self) -> dict:
        rendered, head_bytes, tail_bytes, marker_bytes = self._rendered()
        return {
            "bytes": self.total_bytes,
            "sha256": self._hash.hexdigest(),
            "retained_bytes": len(rendered),
            "truncated": self.truncated,
            "max_bytes": self.max_bytes,
            "head_bytes": head_bytes,
            "tail_bytes": tail_bytes,
            "marker_bytes": marker_bytes,
            "errors": self.errors,
        }


def write_capture(path: Path, capture: CaptureBuffer) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + f".tmp.{os.getpid()}.{threading.get_ident()}")
    with open(tmp, "wb") as stream:
        stream.write(capture.bytes_for_file())
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(tmp, path)


def capture_file(
    source: Path,
    destination: Path | None,
    *,
    max_bytes: int = DEFAULT_MAX_CAPTURE_BYTES,
    error_pattern: Pattern[str] | None = None,
    error_limit: int = 8,
) -> dict:
    capture = CaptureBuffer(
        max_bytes, error_pattern=error_pattern, error_limit=error_limit
    )
    with open(source, "rb") as stream:
        for chunk in iter(lambda: stream.read(READ_CHUNK_BYTES), b""):
            capture.feed(chunk)
    capture.finish()
    if destination is not None:
        write_capture(destination, capture)
    return capture.metadata()


def _drain(stream: BinaryIO, capture: CaptureBuffer) -> None:
    try:
        for chunk in iter(lambda: stream.read(READ_CHUNK_BYTES), b""):
            capture.feed(chunk)
    except (OSError, ValueError):
        # Closing a pipe held by an escaped grandchild can wake the reader.
        pass


def run_bounded(
    cmd: Sequence[str],
    *,
    cwd: Path,
    output_path: Path | None,
    timeout: float,
    env: dict[str, str] | None = None,
    max_bytes: int = DEFAULT_MAX_CAPTURE_BYTES,
    error_pattern: Pattern[str] | None = None,
    error_limit: int = 8,
    start_new_session: bool = True,
    preexec_fn=None,
) -> dict:
    """Run argv without a shell and drain merged output incrementally."""
    capture = CaptureBuffer(
        max_bytes, error_pattern=error_pattern, error_limit=error_limit
    )
    started = time.perf_counter()
    proc: subprocess.Popen[bytes] | None = None
    spawn_error: str | None = None
    timed_out = False
    returncode: int | None = None
    process_finished: float | None = None
    try:
        proc = subprocess.Popen(
            list(cmd),
            cwd=cwd,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            start_new_session=start_new_session,
            preexec_fn=preexec_fn,
        )
    except OSError as exc:
        spawn_error = str(exc)

    reader: threading.Thread | None = None
    if proc is not None:
        assert proc.stdout is not None
        reader = threading.Thread(
            target=_drain, args=(proc.stdout, capture), daemon=True
        )
        reader.start()
        try:
            returncode = proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            try:
                if start_new_session and hasattr(os, "killpg"):
                    os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
                else:
                    proc.kill()
            except OSError:
                pass
            try:
                returncode = proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
                returncode = proc.wait()
        process_finished = time.perf_counter()
        reader.join(timeout=10)
        if reader.is_alive():
            proc.stdout.close()
            reader.join(timeout=1)
        else:
            proc.stdout.close()

    capture.finish()
    if output_path is not None:
        write_capture(output_path, capture)
    meta = capture.metadata()
    return {
        "returncode": returncode,
        "timed_out": timed_out,
        "spawn_error": spawn_error,
        "elapsed_seconds": (process_finished or time.perf_counter()) - started,
        "capture": meta,
    }
