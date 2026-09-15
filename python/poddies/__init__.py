"""Poddies Python plugin SDK.

A Poddies plugin is a process that speaks newline-delimited JSON on stdio. This
module implements that loop for you: subclass :class:`Plugin`, implement
:meth:`Plugin.on_request`, and call :func:`run`.

    from poddies import Plugin, run

    class Hello(Plugin):
        id = "dev.example.hello"
        name = "Hello"
        panels = [{"id": "main", "title": "Hello"}]

        def on_request(self, method, params):
            if method == "ui/panel":
                return {"panel_id": "main", "widgets": [
                    {"type": "heading", "text": "Hello"}]}
            raise UnsupportedMethod(method)

    run(Hello())

Python plugins run in the same sandboxed worker process as native ones, so a
raising handler can never take the host down.
"""

import json
import sys

__all__ = [
    "PROTOCOL_VERSION",
    "Plugin",
    "UnsupportedMethod",
    "call_host",
    "log",
    "run",
]

PROTOCOL_VERSION = "1.0"


class UnsupportedMethod(Exception):
    """Raised by a handler for a method the plugin does not implement."""

    def __init__(self, method):
        super().__init__("plugin does not handle '%s'" % method)
        self.method = method


class Plugin:
    """Base class for a Python plugin."""

    id = "dev.example.plugin"
    name = "Plugin"
    version = "0.1.0"
    #: Panels to advertise: [{"id": ..., "title": ...}]
    panels = []

    def on_request(self, method, params):
        """Handle a request and return its result payload."""
        raise UnsupportedMethod(method)

    def on_event(self, method, params):
        """Handle a notification (e.g. playback events). Default: ignore."""

    def on_shutdown(self):
        """Flush state before the worker exits. Default: nothing to do."""


def _write(payload):
    sys.stdout.write(json.dumps(payload) + "\n")
    sys.stdout.flush()


def log(level, message):
    """Send a log line to the host. Fire and forget."""
    _write({"method": "host/log", "params": {"level": level, "message": message}})


def _describe(plugin):
    return {
        "id": plugin.id,
        "name": plugin.name,
        "version": plugin.version,
        "protocol": PROTOCOL_VERSION,
        "ui_panels": list(plugin.panels),
    }


def call_host(method, params=None):
    """Make a synchronous request back to the host, e.g. ``host/library/shows``.

    Returns the result payload, or ``None`` if the host pipe closed.
    """
    request_id = _next_host_id()
    _write({"id": request_id, "method": method, "params": params or {}})

    while True:
        line = sys.stdin.readline()
        if not line:
            return None
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if "method" in message:
            continue
        if message.get("id") == request_id:
            if message.get("error"):
                return None
            return message.get("result")


_host_id = [1_000_000]


def _next_host_id():
    _host_id[0] += 1
    return _host_id[0]


def run(plugin):
    """Run the plugin loop until the host closes stdin."""
    for raw in sys.stdin:
        raw = raw.strip()
        if not raw:
            continue

        try:
            message = json.loads(raw)
        except ValueError:
            # Malformed input is ignored: a broken request must not be fatal.
            continue

        if "method" not in message:
            # A reply to one of our host calls; consumed above, ignore here.
            continue

        method = message["method"]
        message_id = message.get("id")
        params = message.get("params") or {}

        if message_id is None:
            if method == "shutdown":
                plugin.on_shutdown()
                return
            plugin.on_event(method, params)
            continue

        if method == "describe":
            _write({"id": message_id, "result": _describe(plugin)})
            continue

        if method == "shutdown":
            plugin.on_shutdown()
            _write({"id": message_id, "result": None})
            return

        try:
            _write({"id": message_id, "result": plugin.on_request(method, params)})
        except UnsupportedMethod as error:
            _write(
                {
                    "id": message_id,
                    "error": {"code": "unsupported_method", "message": str(error)},
                }
            )
        except Exception as error:  # noqa: BLE001 - never let a handler kill the pipe
            _write(
                {
                    "id": message_id,
                    "error": {"code": "plugin_error", "message": str(error)},
                }
            )
